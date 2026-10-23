//! In-process AHEAD agent connection.
//!
//! This follows Zed's native-agent split: the built-in agent owns its model and
//! tool loop in-process, while the editor consumes the same normalized events
//! used for external ACP agents. ACP is not involved in this path.

use std::{
    collections::hash_map::DefaultHasher,
    collections::{HashMap, HashSet},
    fs::File,
    hash::Hash,
    hash::Hasher,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ahead_agent_skills::{HostSkillsLoadInput, HostSkillsSnapshot};
use ahead_core::search::WorkspaceFileIndex;
use ahead_rpc::ahead::{
    AgentBufferSnapshot, AgentBufferSymbol, AgentPresentationAction,
    AgentReasoningEffort, AgentReasoningOptions, AgentSkill, AgentSkillCatalog,
    AgentSkillSource, validate_agent_buffer_snapshots,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use codex_config::types::OtelExporterKind;
use codex_core::{CodexThread, StartThreadOptions, ThreadManager};
use codex_exec_server::LOCAL_FS;
use codex_features::Feature;
use codex_models_manager::manager::RefreshStrategy;
use codex_protocol::{
    ThreadId,
    approvals::{ElicitationAction, ElicitationRequest, ElicitationRequestEvent},
    config_types::SandboxMode,
    dynamic_tools::{
        DynamicToolCallOutputContentItem, DynamicToolFunctionSpec,
        DynamicToolResponse, DynamicToolSpec,
    },
    items::{SubAgentActivityItem, TurnItem},
    mcp::RequestId,
    mcp_approval_meta::{
        APPROVAL_KIND_KEY, APPROVAL_KIND_MCP_TOOL_CALL, PERSIST_KEY,
        PERSIST_SESSION, TOOL_PARAMS_KEY,
    },
    openai_models::ReasoningEffort,
    plan_tool::StepStatus,
    protocol::{
        AskForApproval, EventMsg, FileChange, Op, ReviewDecision, SessionSource,
        SkillScope, SubAgentActivityKind, SubAgentSource, ThreadMemoryMode,
        ThreadSettingsOverrides,
    },
    request_permissions::{
        PermissionGrantScope, RequestPermissionProfile, RequestPermissionsResponse,
    },
    request_user_input::{RequestUserInputAnswer, RequestUserInputResponse},
    turn_input::{StartIfIdleSubmission, TurnInputRequest},
    user_input::UserInput,
};
use codex_rollout::{RolloutItem, decode_rollout_line, persisted_rollout_items};
use codex_thread_store::{
    CreateThreadParams, ReadThreadParams, ThreadPersistenceMetadata, ThreadStore,
};
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::stream::{FuturesOrdered, StreamExt};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use crate::editor_tools::{
    AHEAD_CLEAR_PRESENTATION_TOOL, AHEAD_MOVE_CODE_POINTER_TOOL,
    AHEAD_PRESENT_CODE_TOOL, AHEAD_READ_EDITOR_BUFFER_TOOL, AHEAD_SPEAK_TEXT_TOOL,
    AHEAD_STOP_SPEAKING_TOOL, format_read_editor_buffer, parse_presentation_action,
    presentation_tool_definitions, read_editor_buffer_path,
};
use crate::instructions::EmptyUserInstructionsProvider;
use crate::mcp_form::McpForm;
use crate::{
    events::{
        HarnessEvent, HarnessFileChange, HarnessPatchPreview, HarnessPlanEntry,
        HarnessSink, HarnessUserInputOption, HarnessUserInputQuestion,
    },
    runtime_support::{
        AgentScope, McpServerPolicy, file_change_allowed, mcp_servers_for_workspace,
        path_is_allowed, permission_profile, prepare_runtime_home,
        runtime_config_from_sources,
    },
    store::{HarnessStore, LegacyNativeThreadImport, ProjectInstructionSource},
    turso_agent_graph_store::TursoAgentGraphStore,
    turso_thread_store::TursoThreadStore,
};
use codex_utils_path_uri::PathUri;

const MAX_LEGACY_ROLLOUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LEGACY_ROLLOUT_RECORDS: usize = 200_000;
const MAX_LEGACY_ROLLOUT_WINDOW_LOG: u32 = 26;

fn validate_legacy_rollout_directories(root: &Path, path: &Path) -> Result<()> {
    let relative_path = path
        .strip_prefix(root)
        .context("legacy rollout is outside the runtime session root")?;
    let parent = relative_path
        .parent()
        .context("legacy rollout has no session directory")?;
    let mut directory = root.to_path_buf();
    for component in parent.components() {
        directory.push(component);
        let metadata = std::fs::symlink_metadata(&directory)
            .context("inspecting legacy AHEAD session directory")?;
        anyhow::ensure!(
            metadata.file_type().is_dir(),
            "legacy AHEAD session directories must not be symlinks"
        );
    }
    Ok(())
}

fn legacy_rollout_candidates(
    runtime_root: &Path,
    thread_id: &str,
) -> Result<Vec<(PathBuf, bool)>> {
    let mut candidates = Vec::new();
    for (directory, archived) in [("sessions", false), ("archived_sessions", true)] {
        let directory = runtime_root.join(directory);
        let metadata = match std::fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).context("inspecting legacy rollout directory");
            }
        };
        anyhow::ensure!(
            metadata.file_type().is_dir(),
            "legacy AHEAD rollout directories must not be symlinks"
        );
        for entry in walkdir::WalkDir::new(&directory).follow_links(false) {
            let entry = entry.context("searching legacy AHEAD rollouts")?;
            let name = entry.file_name().to_string_lossy();
            if !name.contains(thread_id)
                || !(name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
            {
                continue;
            }
            let file_type = entry.file_type();
            if file_type.is_file() || file_type.is_symlink() {
                candidates.push((entry.into_path(), archived));
            }
        }
    }
    Ok(candidates)
}

fn read_legacy_rollout_items(path: &Path, file: File) -> Result<Vec<RolloutItem>> {
    let reader: Box<dyn Read> = if path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.ends_with(".jsonl.zst"))
    {
        let mut decoder = zstd::stream::read::Decoder::new(file)
            .context("opening compressed legacy AHEAD rollout")?;
        decoder
            .window_log_max(MAX_LEGACY_ROLLOUT_WINDOW_LOG)
            .context("limiting legacy AHEAD rollout decompression window")?;
        Box::new(decoder)
    } else {
        Box::new(file)
    };
    let mut decompressed = Vec::new();
    reader
        .take(MAX_LEGACY_ROLLOUT_BYTES + 1)
        .read_to_end(&mut decompressed)
        .context("reading legacy AHEAD rollout")?;
    anyhow::ensure!(
        decompressed.len() as u64 <= MAX_LEGACY_ROLLOUT_BYTES,
        "legacy AHEAD rollout exceeds the {} MiB decompressed import limit",
        MAX_LEGACY_ROLLOUT_BYTES / (1024 * 1024)
    );

    let mut items = Vec::new();
    for (index, line) in decompressed.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        anyhow::ensure!(
            items.len() < MAX_LEGACY_ROLLOUT_RECORDS,
            "legacy AHEAD rollout exceeds the {MAX_LEGACY_ROLLOUT_RECORDS} record import limit"
        );
        let mut value: Value = serde_json::from_slice(line).with_context(|| {
            format!("parsing legacy AHEAD rollout record {}", index + 1)
        })?;
        if strip_legacy_ghost_snapshot_rollout_line(&mut value) {
            continue;
        }
        let line = decode_rollout_line(value).with_context(|| {
            format!("decoding legacy AHEAD rollout record {}", index + 1)
        })?;
        items.push(line.item);
    }
    anyhow::ensure!(!items.is_empty(), "legacy AHEAD rollout is empty");
    Ok(items)
}

fn strip_legacy_ghost_snapshot_rollout_line(value: &mut Value) -> bool {
    match value.get("type").and_then(Value::as_str) {
        Some("response_item") => {
            value
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(Value::as_str)
                == Some("ghost_snapshot")
        }
        Some("compacted") => {
            let Some(payload) =
                value.get_mut("payload").and_then(Value::as_object_mut)
            else {
                return false;
            };
            let Some(replacement_history) =
                payload.get("replacement_history").and_then(Value::as_array)
            else {
                return false;
            };
            let remove = replacement_history
                .iter()
                .map(|item| {
                    item.get("type").and_then(Value::as_str)
                        == Some("ghost_snapshot")
                })
                .collect::<Vec<_>>();
            if !remove.contains(&true) {
                return false;
            }
            match payload.get("replacement_history_metadata") {
                None => {}
                Some(Value::Array(metadata)) if metadata.len() == remove.len() => {}
                Some(_) => return false,
            }
            let Some(replacement_history) = payload
                .get_mut("replacement_history")
                .and_then(Value::as_array_mut)
            else {
                return false;
            };
            retain_unmarked_entries(replacement_history, &remove);
            if let Some(metadata) = payload
                .get_mut("replacement_history_metadata")
                .and_then(Value::as_array_mut)
            {
                retain_unmarked_entries(metadata, &remove);
            }
            false
        }
        _ => false,
    }
}

fn retain_unmarked_entries(entries: &mut Vec<Value>, remove: &[bool]) {
    let mut index = 0;
    entries.retain(|_| {
        let retain = !remove[index];
        index += 1;
        retain
    });
}

#[derive(Debug, Clone)]
pub struct NativeClientConfig {
    pub cwd: PathBuf,
    pub file_index: Option<Arc<WorkspaceFileIndex>>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
}

impl NativeClientConfig {
    pub fn ahead(cwd: PathBuf) -> Self {
        Self {
            cwd,
            file_index: None,
            model: None,
            model_provider: None,
        }
    }

    pub fn with_model_selection(
        mut self,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Self {
        self.model = model.map(str::to_string);
        self.model_provider = model_provider.map(str::to_string);
        self
    }

    pub fn with_file_index(mut self, file_index: Arc<WorkspaceFileIndex>) -> Self {
        self.file_index = Some(file_index);
        self
    }
}

#[derive(Debug, Clone)]
struct SessionSettings {
    cwd: PathBuf,
    mode_id: String,
    model: Option<String>,
    model_provider: Option<String>,
    loaded_provider_fingerprint: [u8; 32],
}

fn provider_fingerprint(config: &codex_core::config::Config) -> Result<[u8; 32]> {
    let provider = serde_json::to_vec(&config.model_provider)
        .context("failed to inspect the AHEAD model connection")?;
    let mut digest = Sha256::new();
    digest.update(config.model_provider_id.as_bytes());
    digest.update([0]);
    digest.update(provider);
    Ok(digest.finalize().into())
}

fn local_project_instruction_sources(
    sources: Vec<(PathUri, String)>,
) -> Result<Vec<ProjectInstructionSource>> {
    sources
        .into_iter()
        .map(|(path, content_sha256)| {
            Ok(ProjectInstructionSource {
                path: path
                    .to_abs_path()
                    .context("project instructions must belong to this host")?
                    .to_path_buf(),
                content_sha256,
            })
        })
        .collect()
}

type UserInputAnswers = HashMap<String, Vec<String>>;
struct PendingUserInput {
    sender: oneshot::Sender<UserInputAnswers>,
    kind: PendingUserInputKind,
}

enum PendingUserInputKind {
    AgentQuestion,
    McpForm(McpForm),
    McpUrl,
}
// Child runtime threads share the parent work-session queue, so model call IDs
// cannot identify pending host requests by themselves.
type PendingUserInputs = HashMap<String, HashMap<String, PendingUserInput>>;

struct TurnPendingInputs<'a> {
    pending: &'a Mutex<PendingUserInputs>,
    session_id: &'a str,
    request_ids: Vec<String>,
    sink: HarnessSink,
}

impl Drop for TurnPendingInputs<'_> {
    fn drop(&mut self) {
        let mut pending = self.pending.lock();
        let mut cancelled = Vec::new();
        let remove_session = if let Some(requests) = pending.get_mut(self.session_id)
        {
            for request_id in &self.request_ids {
                if requests.remove(request_id).is_some() {
                    cancelled.push(request_id.clone());
                }
            }
            requests.is_empty()
        } else {
            false
        };
        if remove_session {
            pending.remove(self.session_id);
        }
        drop(pending);
        for request_id in cancelled {
            (self.sink)(HarnessEvent::UserInputCancelled {
                acp_session_id: self.session_id.to_string(),
                request_id,
            });
        }
    }
}

type PendingBufferSnapshots = HashMap<
    (String, String),
    oneshot::Sender<std::result::Result<Vec<AgentBufferSnapshot>, String>>,
>;
type PendingEditorPresentations =
    HashMap<(String, String), oneshot::Sender<(bool, String)>>;

fn validate_mcp_approval_answers(answers: &UserInputAnswers) -> Result<()> {
    anyhow::ensure!(
        answers.iter().all(|(question_id, choices)| {
            !question_id.starts_with("mcp_tool_call_approval_") || choices.len() == 1
        }),
        "AHEAD MCP approval requires exactly one choice"
    );
    Ok(())
}

fn mcp_url_action(answers: &UserInputAnswers) -> Result<ElicitationAction> {
    anyhow::ensure!(answers.len() == 1, "invalid MCP URL response");
    match answers
        .get(ahead_rpc::ahead::MCP_FORM_ACTION_KEY)
        .map(Vec::as_slice)
    {
        Some([action]) if action == "accept" => Ok(ElicitationAction::Accept),
        Some([action]) if action == "decline" => Ok(ElicitationAction::Decline),
        Some([action]) if action == "cancel" => Ok(ElicitationAction::Cancel),
        _ => anyhow::bail!("invalid MCP URL response"),
    }
}

fn mcp_approval_elicitation_question(
    event: &ElicitationRequestEvent,
) -> Option<(String, HarnessUserInputQuestion)> {
    let RequestId::String(request_id) = &event.id else {
        return None;
    };
    let ElicitationRequest::Form {
        meta: Some(meta),
        message,
        requested_schema,
    } = &event.request
    else {
        return None;
    };
    if !request_id.starts_with("mcp_tool_call_approval_")
        || meta.get(APPROVAL_KIND_KEY).and_then(Value::as_str)
            != Some(APPROVAL_KIND_MCP_TOOL_CALL)
        || requested_schema != &json!({"type": "object", "properties": {}})
    {
        return None;
    }
    let mut options = vec![HarnessUserInputOption {
        value: "Allow".to_string(),
        label: "Allow".to_string(),
        description: "Run the tool and continue.".to_string(),
    }];
    if meta.get(PERSIST_KEY).and_then(Value::as_str) == Some(PERSIST_SESSION) {
        options.push(HarnessUserInputOption {
            value: "Allow for this session".to_string(),
            label: "Allow for this session".to_string(),
            description: "Run the tool and remember this choice for this session."
                .to_string(),
        });
    }
    options.push(HarnessUserInputOption {
        value: "Cancel".to_string(),
        label: "Cancel".to_string(),
        description: "Cancel this tool call.".to_string(),
    });
    let question = match meta.get(TOOL_PARAMS_KEY) {
        Some(arguments) => format!("{message}\nArguments: {arguments}"),
        None => message.clone(),
    };
    Some((
        request_id.clone(),
        HarnessUserInputQuestion {
            id: request_id.clone(),
            header: "Approve MCP tool?".to_string(),
            question,
            external_url: None,
            options,
            default_answers: Vec::new(),
            allows_other: false,
            is_secret: false,
        },
    ))
}

fn mcp_url_elicitation_question(
    event: &ElicitationRequestEvent,
) -> Option<HarnessUserInputQuestion> {
    let ElicitationRequest::Url {
        message,
        url,
        elicitation_id,
        ..
    } = &event.request
    else {
        return None;
    };
    if event.server_name.is_empty()
        || event.server_name.len() > 256
        || event.server_name.chars().any(char::is_control)
        || message.trim().is_empty()
        || message.len() > 4096
        || message.chars().any(|character| {
            character.is_control() && character != '\n' && character != '\t'
        })
        || elicitation_id.is_empty()
        || elicitation_id.len() > 256
        || elicitation_id.chars().any(char::is_control)
    {
        return None;
    }
    let url = ahead_rpc::ahead::validated_mcp_url(url)?;
    Some(HarnessUserInputQuestion {
        id: "mcp_url".to_string(),
        header: format!("MCP server: {} · External site", event.server_name),
        question: format!(
            "{message}\nVerify the destination before opening it in your browser."
        ),
        external_url: Some(url.to_string()),
        options: Vec::new(),
        default_answers: Vec::new(),
        allows_other: false,
        is_secret: false,
    })
}

fn mcp_server_policy(mode_id: &str, read_only: bool) -> McpServerPolicy {
    if mode_id == "agent" && !read_only {
        McpServerPolicy::PromptEveryCall
    } else {
        McpServerPolicy::Disabled
    }
}

#[cfg(any(test, feature = "test-support"))]
fn test_sandbox_helper_executable(current_exe: PathBuf) -> Result<PathBuf> {
    // Dependent-crate tests do not compile this crate with cfg(test).
    // Cargo test binaries cannot reenter AHEAD's sandbox helper modes.
    let Some(dependencies) = current_exe.parent().filter(|directory| {
        directory.file_name() == Some(std::ffi::OsStr::new("deps"))
    }) else {
        return Ok(current_exe);
    };
    let profile = dependencies
        .parent()
        .context("Cargo test executable has no profile directory")?;
    let helper = profile.join(format!("ahead{}", std::env::consts::EXE_SUFFIX));
    ensure!(
        helper.is_file(),
        "AHEAD sandbox helper is missing at {}. Run `cargo build -p ahead --bin ahead` before native-agent tests",
        helper.display()
    );
    Ok(helper)
}

/// A direct connection to the hard-forked agent loop.
pub struct NativeClient {
    runtime: Runtime,
    manager: Arc<ThreadManager>,
    model_manager_provider_fingerprint: [u8; 32],
    store: Arc<dyn HarnessStore>,
    thread_store: Arc<TursoThreadStore>,
    runtime_home: PathBuf,
    file_index: Option<Arc<WorkspaceFileIndex>>,
    threads: Mutex<HashMap<String, Arc<CodexThread>>>,
    thread_depths: Mutex<HashMap<String, u8>>,
    settings: Mutex<HashMap<String, SessionSettings>>,
    scopes: Mutex<HashMap<String, AgentScope>>,
    pending_inputs: Mutex<PendingUserInputs>,
    pending_buffer_snapshots: Mutex<PendingBufferSnapshots>,
    pending_editor_presentations: Mutex<PendingEditorPresentations>,
    sink: HarnessSink,
    thread_admission: tokio::sync::RwLock<()>,
    running: AtomicBool,
}

impl NativeClient {
    pub fn spawn(
        config: &NativeClientConfig,
        store: Arc<dyn HarnessStore>,
        sink: HarnessSink,
    ) -> Result<Self> {
        let runtime_home = prepare_runtime_home(&config.cwd)?;
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("ahead-agent")
            .thread_stack_size(8 * 1024 * 1024)
            .build()
            .context("failed to start the AHEAD agent runtime")?;
        let initial_config = runtime.block_on(Self::build_config(
            &runtime_home,
            &config.cwd,
            config.model.as_deref(),
            config.model_provider.as_deref(),
            McpServerPolicy::Disabled,
        ))?;
        let model_manager_provider_fingerprint =
            provider_fingerprint(&initial_config)?;
        let thread_store = Arc::new(TursoThreadStore::new(store.clone()));
        let manager = Arc::new(
            runtime
                .block_on(ThreadManager::new_for_ahead_with_instructions_and_store(
                    &initial_config,
                    Arc::new(EmptyUserInstructionsProvider),
                    thread_store.clone(),
                    Arc::new(TursoAgentGraphStore::new(store.clone())),
                ))
                .map_err(|error| {
                    anyhow!("failed to initialize AHEAD agent core: {error}")
                })?,
        );
        Ok(Self {
            runtime,
            manager,
            model_manager_provider_fingerprint,
            store,
            thread_store,
            runtime_home,
            file_index: config.file_index.clone(),
            threads: Mutex::new(HashMap::new()),
            thread_depths: Mutex::new(HashMap::new()),
            settings: Mutex::new(HashMap::new()),
            scopes: Mutex::new(HashMap::new()),
            pending_inputs: Mutex::new(HashMap::new()),
            pending_buffer_snapshots: Mutex::new(HashMap::new()),
            pending_editor_presentations: Mutex::new(HashMap::new()),
            sink,
            thread_admission: tokio::sync::RwLock::new(()),
            running: AtomicBool::new(true),
        })
    }

    async fn build_config(
        runtime_home: &Path,
        cwd: &Path,
        model: Option<&str>,
        model_provider: Option<&str>,
        mcp_server_policy: McpServerPolicy,
    ) -> Result<codex_core::config::Config> {
        let user_home = crate::instructions::user_home();
        let provider_overrides =
            runtime_config_from_sources(cwd, user_home.as_deref())?
                .unwrap_or_default()
                .into_iter()
                .collect();
        let ahead_self_exe = std::env::current_exe()
            .context("failed to resolve the AHEAD proxy executable")?;
        #[cfg(any(test, feature = "test-support"))]
        let ahead_self_exe = test_sandbox_helper_executable(ahead_self_exe)?;
        let overrides = codex_core::config::ConfigOverrides {
            model: model
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string),
            cwd: Some(cwd.to_path_buf()),
            approval_policy: Some(AskForApproval::OnRequest),
            // Startup is always read-only. Assist receives its exact AHEAD
            // artifact scope as a per-turn permission profile.
            sandbox_mode: Some(SandboxMode::ReadOnly),
            model_provider: model_provider
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string),
            // The proxy binary exposes only Core's two sandbox helper modes;
            // it does not restore the deleted Codex CLI surface.
            ahead_self_exe: Some(ahead_self_exe),
            ..Default::default()
        };
        let mut config = codex_core::config::ConfigBuilder::default()
            .codex_home(runtime_home.to_path_buf())
            .cli_overrides(provider_overrides)
            .harness_overrides(overrides)
            .loader_overrides(codex_core::config::LoaderOverrides {
                ignore_managed_requirements: true,
                // AHEAD provider settings arrive in memory; never reload a stale runtime file.
                ignore_user_config: true,
                ignore_user_exec_policy_rules: true,
                ..Default::default()
            })
            .build()
            .await
            .context("failed to load the AHEAD native agent configuration")?;
        // MCP servers are selected from AHEAD's workspace config only after
        // explicit local opt-in. Their tools always use the human-review path.
        let mcp_servers = mcp_servers_for_workspace(cwd, mcp_server_policy)?;
        config
            .mcp_servers
            .set(mcp_servers)
            .context("failed to apply AHEAD MCP server policy")?;
        for feature in AHEAD_DISABLED_FEATURES {
            config.features.disable(*feature).map_err(|error| {
                anyhow!(
                    "AHEAD native runtime cannot disable Codex product feature {feature:?}: {error}"
                )
            })?;
        }
        config
            .features
            .enable(Feature::Mcp20260728)
            .context("AHEAD could not enable the current MCP protocol")?;
        config
            .features
            .enable(Feature::ApplyPatchStreamingEvents)
            .context("AHEAD could not enable streamed patch previews")?;
        config.memories.generate_memories = false;
        config.memories.use_memories = false;
        config.memories.dedicated_tools = false;
        // Runtime-home config cannot opt the editor's managed agent into telemetry.
        config.otel.exporter = OtelExporterKind::None;
        config.otel.trace_exporter = OtelExporterKind::None;
        config.otel.metrics_exporter = OtelExporterKind::None;
        config.otel.log_user_prompt = false;
        let mut developer_instructions = config
            .developer_instructions
            .take()
            .into_iter()
            .collect::<Vec<_>>();
        developer_instructions
            .push(AHEAD_MANAGED_DEVELOPER_INSTRUCTIONS.to_string());
        config.developer_instructions = Some(developer_instructions.join("\n\n"));
        Ok(config)
    }

    pub fn initialize(&self) -> Result<Value> {
        Ok(json!({
            "name": "ahead-native-agent",
            "version": env!("CARGO_PKG_VERSION"),
            "transport": "in-process"
        }))
    }

    fn skill_snapshot(
        &self,
        cwd: &Path,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<HostSkillsSnapshot> {
        self.runtime.block_on(self.load_skill_snapshot(
            cwd,
            model,
            model_provider,
            true,
        ))
    }

    async fn load_skill_snapshot(
        &self,
        cwd: &Path,
        model: Option<&str>,
        model_provider: Option<&str>,
        force_reload: bool,
    ) -> Result<HostSkillsSnapshot> {
        let config = Self::build_config(
            &self.runtime_home,
            cwd,
            model,
            model_provider,
            McpServerPolicy::Disabled,
        )
        .await?;
        let cwd = AbsolutePathBuf::from_absolute_path(cwd)
            .context("AHEAD skill discovery requires an absolute workspace path")?;
        let input = HostSkillsLoadInput::new(cwd, config.config_layer_stack);
        let skills_service = self.manager.skills_service();
        let skills_request = skills_service.for_request();
        Ok(skills_request
            .snapshot_for_cwd(&input, force_reload, Some(Arc::clone(&LOCAL_FS)))
            .await)
    }

    pub fn available_skills(
        &self,
        cwd: &Path,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<AgentSkillCatalog> {
        let snapshot = self.skill_snapshot(cwd, model, model_provider)?;
        let outcome = snapshot.outcome();
        let skills = outcome
            .skills
            .iter()
            .filter(|skill| {
                outcome.is_skill_enabled(skill)
                    && !is_native_slash_command(&skill.name)
            })
            .map(|skill| AgentSkill {
                name: skill.name.clone(),
                description: skill
                    .short_description
                    .as_ref()
                    .filter(|description| !description.trim().is_empty())
                    .unwrap_or(&skill.description)
                    .clone(),
                source: agent_skill_source(skill.scope),
            })
            .collect();
        Ok(AgentSkillCatalog {
            skills,
            skipped_count: outcome.errors.len(),
        })
    }

    pub fn reasoning_options(
        &self,
        cwd: &Path,
        model: &str,
        model_provider: Option<&str>,
    ) -> Result<AgentReasoningOptions> {
        let config = self.runtime.block_on(Self::build_config(
            &self.runtime_home,
            cwd,
            Some(model),
            model_provider,
            McpServerPolicy::Disabled,
        ))?;
        let models_manager = self.manager.get_models_manager();
        let models_config = config.to_models_manager_config();
        let model_info = self.runtime.block_on(async {
            models_manager
                .raw_model_catalog(
                    RefreshStrategy::OnlineIfUncached,
                    config.http_client_factory(),
                )
                .await;
            models_manager.get_model_info(model, &models_config).await
        });

        Ok(AgentReasoningOptions {
            default_effort: model_info
                .default_reasoning_level
                .map(|effort| effort.as_str().to_string()),
            supported_efforts: model_info
                .supported_reasoning_levels
                .into_iter()
                .map(|preset| AgentReasoningEffort {
                    value: preset.effort.as_str().to_string(),
                    description: preset.description,
                })
                .collect(),
        })
    }

    fn prompt_user_input(
        &self,
        settings: &SessionSettings,
        text: &str,
    ) -> Result<Vec<UserInput>> {
        let plain_text = || {
            vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }]
        };
        let Some((skill_name, skill_request)) = slash_skill_invocation(text) else {
            return Ok(plain_text());
        };
        if is_native_slash_command(skill_name) {
            return Ok(plain_text());
        }
        let snapshot = self.skill_snapshot(
            &settings.cwd,
            settings.model.as_deref(),
            settings.model_provider.as_deref(),
        )?;
        let outcome = snapshot.outcome();
        let skill = resolve_slash_skill(
            outcome
                .skills
                .iter()
                .filter(|skill| outcome.is_skill_enabled(skill))
                .map(|skill| {
                    (skill.name.as_str(), agent_skill_source(skill.scope), skill)
                }),
            skill_name,
        )?;
        let mut inputs = vec![UserInput::Skill {
            name: skill.name.clone(),
            path: skill.path_to_skills_md.to_path_buf(),
        }];
        if !skill_request.is_empty() {
            inputs.push(UserInput::Text {
                text: skill_request.to_string(),
                text_elements: Vec::new(),
            });
        }
        Ok(inputs)
    }

    pub fn new_session(
        &self,
        cwd: &Path,
        mode_id: &str,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<String> {
        let _admission = self.runtime.block_on(self.thread_admission.read());
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
        let config = self.runtime.block_on(Self::build_config(
            &self.runtime_home,
            cwd,
            model,
            model_provider,
            mcp_server_policy(mode_id, false),
        ))?;
        let loaded_provider_fingerprint = provider_fingerprint(&config)?;
        let mut options = StartThreadOptions::new(config);
        options.dynamic_tools = ahead_dynamic_tools(true);
        let new_thread = self
            .runtime
            .block_on(self.manager.start_thread(options))
            .map_err(|error| {
            anyhow!("failed to start AHEAD agent thread: {error}")
        })?;
        let thread_id = new_thread.thread_id.to_string();
        self.threads
            .lock()
            .insert(thread_id.clone(), new_thread.thread);
        self.thread_depths.lock().insert(thread_id.clone(), 0);
        self.settings.lock().insert(
            thread_id.clone(),
            SessionSettings {
                cwd: cwd.to_path_buf(),
                mode_id: mode_id.to_string(),
                model: model.map(str::to_string),
                model_provider: model_provider.map(str::to_string),
                loaded_provider_fingerprint,
            },
        );
        Ok(thread_id)
    }

    pub fn load_session(
        &self,
        thread_id: &str,
        cwd: &Path,
        mode_id: &str,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<()> {
        let _admission = self.runtime.block_on(self.thread_admission.read());
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
        if self.threads.lock().contains_key(thread_id) {
            let mut settings = self.settings.lock();
            let current = settings.get_mut(thread_id).with_context(|| {
                format!("AHEAD agent settings for `{thread_id}` are missing")
            })?;
            current.cwd = cwd.to_path_buf();
            current.mode_id = mode_id.to_string();
            current.model = model.map(str::to_string);
            current.model_provider = model_provider.map(str::to_string);
            return Ok(());
        }
        let parsed = ThreadId::from_string(thread_id).map_err(|error| {
            anyhow!("invalid AHEAD thread id `{thread_id}`: {error}")
        })?;
        let config = self.runtime.block_on(Self::build_config(
            &self.runtime_home,
            cwd,
            model,
            model_provider,
            mcp_server_policy(mode_id, false),
        ))?;
        let loaded_provider_fingerprint = provider_fingerprint(&config)?;
        self.import_legacy_thread_if_missing(
            parsed,
            cwd,
            &config.model_provider_id,
        )?;
        let (is_subagent, dynamic_tools) = self.dynamic_tools_for_resume(parsed)?;
        let resumed = self
            .runtime
            .block_on(self.manager.resume_thread_by_id_with_tools(
                parsed,
                config,
                dynamic_tools,
            ))
            .map_err(|error| {
                anyhow!("failed to resume AHEAD agent thread: {error}")
            })?;
        self.threads
            .lock()
            .insert(thread_id.to_string(), resumed.thread);
        self.thread_depths
            .lock()
            .insert(thread_id.to_string(), u8::from(is_subagent));
        self.settings.lock().insert(
            thread_id.to_string(),
            SessionSettings {
                cwd: cwd.to_path_buf(),
                mode_id: mode_id.to_string(),
                model: model.map(str::to_string),
                model_provider: model_provider.map(str::to_string),
                loaded_provider_fingerprint,
            },
        );
        Ok(())
    }

    fn import_legacy_thread_if_missing(
        &self,
        thread_id: ThreadId,
        cwd: &Path,
        fallback_model_provider: &str,
    ) -> Result<()> {
        let thread_id_text = thread_id.to_string();
        if self
            .store
            .load_native_thread_header(&thread_id_text)?
            .is_some()
        {
            return Ok(());
        }

        let runtime_root = self
            .runtime_home
            .canonicalize()
            .context("resolving AHEAD runtime session root")?;
        let mut candidates =
            legacy_rollout_candidates(&runtime_root, &thread_id_text)?;
        anyhow::ensure!(
            candidates.len() <= 1,
            "legacy AHEAD thread {thread_id} has multiple rollout files"
        );
        let Some((path, is_archived)) = candidates.pop() else {
            return Ok(());
        };
        validate_legacy_rollout_directories(&runtime_root, &path)?;
        let before = std::fs::symlink_metadata(&path)
            .context("inspecting legacy AHEAD rollout")?;
        anyhow::ensure!(
            before.file_type().is_file(),
            "legacy AHEAD rollout must be a regular file"
        );
        let canonical_path = path
            .canonicalize()
            .context("resolving legacy AHEAD rollout path")?;
        anyhow::ensure!(
            canonical_path.starts_with(&runtime_root),
            "legacy AHEAD rollout is outside the runtime session root"
        );
        let filename = canonical_path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .context("legacy AHEAD rollout filename is not valid UTF-8")?;
        anyhow::ensure!(
            filename.ends_with(".jsonl") || filename.ends_with(".jsonl.zst"),
            "legacy AHEAD rollout must be JSONL"
        );
        anyhow::ensure!(
            before.len() <= MAX_LEGACY_ROLLOUT_BYTES,
            "legacy AHEAD rollout exceeds the {} MiB import limit",
            MAX_LEGACY_ROLLOUT_BYTES / (1024 * 1024)
        );
        let before_modified = before
            .modified()
            .context("reading legacy AHEAD rollout modification time")?;
        #[cfg(unix)]
        let opened_file =
            ahead_core::secure_fs::open_canonical_regular_file(&canonical_path)
                .context("opening legacy AHEAD rollout without following links")?;
        #[cfg(not(unix))]
        let opened_file =
            File::open(&canonical_path).context("opening legacy AHEAD rollout")?;
        let opened_before = opened_file
            .metadata()
            .context("inspecting opened legacy AHEAD rollout")?;
        anyhow::ensure!(
            opened_before.is_file()
                && opened_before.len() == before.len()
                && opened_before.modified().ok() == Some(before_modified),
            "legacy AHEAD rollout changed before it could be opened"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                opened_before.dev() == before.dev()
                    && opened_before.ino() == before.ino(),
                "legacy AHEAD rollout changed before it could be opened"
            );
        }
        let opened_for_recheck = opened_file
            .try_clone()
            .context("retaining legacy AHEAD rollout for verification")?;
        let items = read_legacy_rollout_items(&canonical_path, opened_file)?;
        let Some(meta) = items.iter().find_map(|item| match item {
            RolloutItem::SessionMeta(line) => Some(&line.meta),
            _ => None,
        }) else {
            bail!("legacy AHEAD rollout has no session metadata")
        };
        anyhow::ensure!(
            meta.id == thread_id,
            "legacy AHEAD session metadata does not match thread {thread_id}"
        );
        anyhow::ensure!(
            meta.cwd.is_absolute(),
            "legacy AHEAD workspace metadata must be an absolute path"
        );
        let expected_cwd = cwd
            .canonicalize()
            .context("resolving requested AHEAD workspace")?;
        let rollout_cwd = meta
            .cwd
            .canonicalize()
            .context("resolving legacy AHEAD workspace metadata")?;
        anyhow::ensure!(
            expected_cwd == rollout_cwd,
            "legacy AHEAD rollout belongs to a different workspace"
        );

        validate_legacy_rollout_directories(&runtime_root, &path)?;
        let after = std::fs::symlink_metadata(&path)
            .context("rechecking legacy AHEAD rollout")?;
        let opened_after = opened_for_recheck
            .metadata()
            .context("rechecking opened legacy AHEAD rollout")?;
        anyhow::ensure!(
            after.file_type().is_file()
                && after.len() == before.len()
                && after.modified().ok() == Some(before_modified)
                && opened_after.len() == before.len()
                && opened_after.modified().ok() == Some(before_modified)
                && path.canonicalize().ok().as_deref()
                    == Some(canonical_path.as_path()),
            "legacy AHEAD rollout changed while it was being imported"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            anyhow::ensure!(
                after.dev() == opened_before.dev()
                    && after.ino() == opened_before.ino(),
                "legacy AHEAD rollout changed while it was being imported"
            );
        }

        let created_at = chrono::DateTime::parse_from_rfc3339(&meta.timestamp)
            .context("parsing legacy AHEAD session creation time")?
            .with_timezone(&chrono::Utc);
        let updated_at =
            chrono::DateTime::<chrono::Utc>::from(before_modified).max(created_at);
        let memory_mode = match meta.memory_mode.as_deref() {
            None | Some("enabled") => ThreadMemoryMode::Enabled,
            Some("disabled") => ThreadMemoryMode::Disabled,
            Some(value) => bail!("unsupported legacy AHEAD memory mode `{value}`"),
        };
        let create_params = CreateThreadParams {
            session_id: meta.session_id,
            thread_id,
            forked_from_id: meta.forked_from_id,
            parent_thread_id: meta.parent_thread_id,
            source: meta.source.clone(),
            thread_source: meta.thread_source.clone(),
            originator: meta.originator.clone(),
            base_instructions: meta.base_instructions.clone().unwrap_or_default(),
            dynamic_tools: meta.dynamic_tools.clone().unwrap_or_default(),
            multi_agent_version: meta.multi_agent_version,
            history_mode: meta.history_mode,
            history_base: meta.history_base,
            subagent_history_start_ordinal: meta.subagent_history_start_ordinal,
            initial_window_id: meta
                .context_window
                .as_ref()
                .map(|window| window.window_id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            metadata: ThreadPersistenceMetadata {
                cwd: Some(meta.cwd.clone()),
                model_provider: meta
                    .model_provider
                    .clone()
                    .filter(|provider| !provider.trim().is_empty())
                    .unwrap_or_else(|| fallback_model_provider.to_string()),
                memory_mode,
            },
        };
        let rollout_items = persisted_rollout_items(&items, meta.history_mode)
            .into_iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        self.store
            .import_legacy_native_thread(LegacyNativeThreadImport {
                thread_id: thread_id_text,
                create_params: serde_json::to_value(create_params)?,
                rollout_items,
                created_at,
                updated_at,
                archived_at: is_archived.then_some(updated_at),
            })?;
        Ok(())
    }

    fn dynamic_tools_for_resume(
        &self,
        thread_id: ThreadId,
    ) -> Result<(bool, Vec<DynamicToolSpec>)> {
        let thread = self
            .runtime
            .block_on(self.thread_store.read_thread(ReadThreadParams {
                thread_id,
                include_archived: true,
                include_history: false,
            }))
            .map_err(|error| {
                anyhow!("failed to inspect AHEAD thread {thread_id} before resume: {error}")
            })?;
        let is_subagent = thread.parent_thread_id.is_some();
        Ok((is_subagent, ahead_dynamic_tools(!is_subagent)))
    }

    pub fn set_session_mode(&self, thread_id: &str, mode_id: &str) -> Result<()> {
        let _admission = self.runtime.block_on(self.thread_admission.read());
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
        let mut next_settings = self
            .settings
            .lock()
            .get(thread_id)
            .cloned()
            .with_context(|| {
                format!("AHEAD agent thread `{thread_id}` is not loaded")
            })?;
        next_settings.mode_id = mode_id.to_string();
        let read_only = self
            .scopes
            .lock()
            .get(thread_id)
            .is_some_and(|scope| scope.read_only);
        let config = self.runtime.block_on(Self::build_config(
            &self.runtime_home,
            &next_settings.cwd,
            next_settings.model.as_deref(),
            next_settings.model_provider.as_deref(),
            mcp_server_policy(&next_settings.mode_id, read_only),
        ))?;
        let thread =
            self.threads
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent thread `{thread_id}` is not loaded")
                })?;
        self.runtime.block_on(thread.refresh_mcp_config(config));
        self.settings
            .lock()
            .insert(thread_id.to_string(), next_settings);
        (self.sink)(HarnessEvent::ModeChanged {
            acp_session_id: thread_id.to_string(),
            mode_id: mode_id.to_string(),
        });
        Ok(())
    }

    pub fn set_scope(
        &self,
        thread_id: &str,
        workspace: &Path,
        allowed_paths: &[String],
        read_only: bool,
    ) -> Result<()> {
        let _admission = self.runtime.block_on(self.thread_admission.read());
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
        let settings =
            self.settings
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent settings for `{thread_id}` are missing")
                })?;
        self.scopes.lock().insert(
            thread_id.to_string(),
            AgentScope {
                workspace: workspace.to_path_buf(),
                allowed_paths: allowed_paths.to_vec(),
                read_only,
            },
        );
        let config = self.runtime.block_on(Self::build_config(
            &self.runtime_home,
            workspace,
            settings.model.as_deref(),
            settings.model_provider.as_deref(),
            mcp_server_policy(&settings.mode_id, read_only),
        ))?;
        let thread =
            self.threads
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent thread `{thread_id}` is not loaded")
                })?;
        self.runtime.block_on(thread.refresh_mcp_config(config));
        Ok(())
    }

    pub fn prompt(&self, thread_id: &str, text: &str) -> Result<String> {
        self.prompt_inner(thread_id, text, None, None, None)
    }

    pub fn prompt_for_ahead_turn(
        &self,
        thread_id: &str,
        text: &str,
        ahead_turn_id: &str,
        model_context_window: Option<u32>,
    ) -> Result<String> {
        self.prompt_inner(
            thread_id,
            text,
            Some(ahead_turn_id),
            Some(model_context_window),
            None,
        )
    }

    pub fn prompt_for_ahead_turn_with_reasoning(
        &self,
        thread_id: &str,
        text: &str,
        ahead_turn_id: &str,
        model_context_window: Option<u32>,
        reasoning_effort: Option<&str>,
    ) -> Result<String> {
        self.prompt_inner(
            thread_id,
            text,
            Some(ahead_turn_id),
            Some(model_context_window),
            reasoning_effort,
        )
    }

    fn prompt_inner(
        &self,
        thread_id: &str,
        text: &str,
        ahead_turn_id: Option<&str>,
        model_context_window: Option<Option<u32>>,
        reasoning_effort: Option<&str>,
    ) -> Result<String> {
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
        self.ensure_thread_history_healthy(thread_id)?;
        let thread =
            self.threads
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent thread `{thread_id}` is not loaded")
                })?;
        let settings =
            self.settings
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent settings for `{thread_id}` are missing")
                })?;
        ensure!(
            !self.provider_changed(
                thread_id,
                &settings.cwd,
                &settings.mode_id,
                settings.model.as_deref(),
                settings.model_provider.as_deref(),
            )?,
            "AHEAD model connection changed before this turn began; retry after other managed turns finish"
        );
        let user_input = self.prompt_user_input(&settings, text)?;
        let scope = self.scopes.lock().get(thread_id).cloned();
        let permission_profile =
            permission_profile(&settings.mode_id, &settings.cwd, scope.as_ref())?;
        let effort = reasoning_effort
            .map(|effort| {
                effort.parse::<ReasoningEffort>().map_err(|error| {
                    anyhow!("invalid reasoning effort `{effort}`: {error}")
                })
            })
            .transpose()?;
        let thread_settings = ThreadSettingsOverrides {
            approval_policy: Some(AskForApproval::OnRequest),
            permission_profile: Some(permission_profile),
            model: settings.model.clone(),
            model_context_window: model_context_window.map(|context_window| {
                context_window.and_then(std::num::NonZeroU32::new)
            }),
            // The composer always sends its selection. An unset selection
            // explicitly clears a stale override and follows model defaults.
            effort: Some(effort),
            ..Default::default()
        };
        let request = TurnInputRequest::user_input(user_input)
            .with_thread_settings(thread_settings);
        self.runtime.block_on(async {
            let core_turn_id = match thread.start_turn_if_idle(request).await? {
                StartIfIdleSubmission::Started { turn_id } => turn_id,
                StartIfIdleSubmission::NotSubmitted { reason } => {
                    bail!("AHEAD agent did not start the turn: {reason:?}");
                }
            };
            // Preserve the exact instructions used by this turn before a slow
            // model request can be interrupted by an app or process restart.
            let instruction_snapshot = ahead_turn_id.map(|ahead_turn_id| {
                let ahead_turn_id = ahead_turn_id.to_string();
                let core_turn_id = core_turn_id.clone();
                let thread = thread.clone();
                let store = self.store.clone();
                tokio::spawn(async move {
                    loop {
                        if let Some(sources) = thread
                            .project_instruction_source_hashes_for_turn(
                                &core_turn_id,
                            )
                            .await
                        {
                            let sources =
                                local_project_instruction_sources(sources)?;
                            store.set_project_instruction_sources(
                                &ahead_turn_id,
                                &sources,
                            )?;
                            return Ok::<_, anyhow::Error>(sources);
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
            });
            let result = Box::pin(self.consume_turn(
                thread_id,
                thread_id,
                &thread,
                &settings.mode_id,
                false,
                ahead_turn_id,
            ))
            .await;
            let captured_instruction_sources =
                if let Some(instruction_snapshot) = instruction_snapshot {
                    if !instruction_snapshot.is_finished() {
                        instruction_snapshot.abort();
                    }
                    match instruction_snapshot.await {
                        Ok(sources) => Some(sources?),
                        Err(error) if error.is_cancelled() => None,
                        Err(error) => return Err(error.into()),
                    }
                } else {
                    None
                };
            if let Some(ahead_turn_id) = ahead_turn_id {
                if captured_instruction_sources.is_none() {
                    match thread
                        .project_instruction_source_hashes_for_turn(&core_turn_id)
                        .await
                    {
                        Some(sources) => {
                            self.store.set_project_instruction_sources(
                                ahead_turn_id,
                                &local_project_instruction_sources(sources)?,
                            )?;
                        }
                        None if result.is_ok() => {
                            bail!("managed turn completed without an AGENTS.md provenance snapshot")
                        }
                        None => {}
                    }
                }
            }
            result
        })
    }

    pub fn current_project_instruction_sources(
        &self,
        thread_id: &str,
    ) -> Result<Vec<ProjectInstructionSource>> {
        let thread =
            self.threads
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent thread `{thread_id}` is not loaded")
                })?;
        let sources = self
            .runtime
            .block_on(thread.refresh_project_instruction_source_hashes())?;
        local_project_instruction_sources(sources)
    }

    /// Detects whether a loaded thread's next selection needs a fresh runtime.
    /// An unloaded thread will read current settings when it is restored.
    pub fn provider_changed(
        &self,
        thread_id: &str,
        cwd: &Path,
        mode_id: &str,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<bool> {
        let Some(settings) = self.settings.lock().get(thread_id).cloned() else {
            return Ok(false);
        };
        let current_config = self.runtime.block_on(Self::build_config(
            &self.runtime_home,
            cwd,
            model,
            model_provider,
            mcp_server_policy(mode_id, false),
        ))?;
        Ok(provider_fingerprint(&current_config)?
            != settings.loaded_provider_fingerprint)
    }

    /// Whether this runtime's shared model catalog was initialized for the requested provider.
    pub fn configured_for_provider(
        &self,
        cwd: &Path,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<bool> {
        let current_config = self.runtime.block_on(Self::build_config(
            &self.runtime_home,
            cwd,
            model,
            model_provider,
            McpServerPolicy::Disabled,
        ))?;
        Ok(provider_fingerprint(&current_config)?
            == self.model_manager_provider_fingerprint)
    }

    async fn consume_turn(
        &self,
        acp_session_id: &str,
        runtime_thread_id: &str,
        thread: &Arc<CodexThread>,
        mode_id: &str,
        return_agent_message: bool,
        ahead_turn_id: Option<&str>,
    ) -> Result<String> {
        let thread_id = acp_session_id;
        let is_child_thread = runtime_thread_id != acp_session_id;
        let mut terminal_error = None;
        let mut saw_agent_delta = false;
        let mut saw_reasoning_delta = false;
        let mut agent_message = String::new();
        let mut agent_message_truncated = false;
        let mut pending_input_cleanup = TurnPendingInputs {
            pending: &self.pending_inputs,
            session_id: thread_id,
            request_ids: Vec::new(),
            sink: self.sink.clone(),
        };
        let mut input_responses = JoinSet::<Result<()>>::new();
        let mut dynamic_tool_calls = FuturesOrdered::new();
        let mut turn_skill_resource_ids = HashSet::new();
        loop {
            let event = tokio::select! {
                event = thread.next_event() => event?,
                completed = input_responses.join_next(), if !input_responses.is_empty() => {
                    if let Some(completed) = completed {
                        completed.context("AHEAD agent input responder failed")??;
                    }
                    continue;
                }
                completed = dynamic_tool_calls.next(), if !dynamic_tool_calls.is_empty() => {
                    let (call_id, tool, is_subagent_tool, result):
                        (String, String, bool, Option<Result<String>>) = completed
                            .context("AHEAD dynamic tool queue ended unexpectedly")??;
                    let success = result.as_ref().is_some_and(|result| result.is_ok());
                    let response_text = match result {
                        Some(Ok(text)) => text,
                        Some(Err(error)) => error.to_string(),
                        None => "AHEAD has no registered dynamic client tool for this request".to_string(),
                    };
                    if !is_subagent_tool {
                        self.tool_event(
                            thread_id,
                            call_id.clone(),
                            tool,
                            if success { "completed" } else { "failed" },
                            "dynamicToolCall",
                        );
                    }
                    thread
                        .submit(Op::DynamicToolResponse {
                            id: call_id,
                            response: DynamicToolResponse {
                                content_items: vec![
                                    DynamicToolCallOutputContentItem::InputText {
                                        text: response_text,
                                    },
                                ],
                                success,
                            },
                        })
                        .await?;
                    continue;
                }
            };
            if let Err(error) = self.ensure_thread_history_healthy(runtime_thread_id)
            {
                if !matches!(
                    &event.msg,
                    EventMsg::TurnComplete(_)
                        | EventMsg::TurnAborted(_)
                        | EventMsg::ShutdownComplete
                ) && let Err(interrupt_error) = thread.submit(Op::Interrupt).await
                {
                    tracing::warn!(
                        "failed to interrupt unsafe AHEAD agent turn after a history write failure: {interrupt_error}"
                    );
                }
                return Err(error);
            }
            match event.msg {
                EventMsg::AgentMessageContentDelta(event) => {
                    saw_agent_delta = true;
                    if return_agent_message {
                        agent_message_truncated |= append_bounded(
                            &mut agent_message,
                            &event.delta,
                            MAX_SUBAGENT_OUTPUT_BYTES,
                        );
                    }
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentDelta {
                            acp_session_id: acp_session_id.to_string(),
                            text: event.delta,
                        });
                    }
                }
                EventMsg::ReasoningContentDelta(event) => {
                    saw_reasoning_delta = true;
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: acp_session_id.to_string(),
                            text: event.delta,
                        });
                    }
                }
                EventMsg::AgentMessage(event) => {
                    if return_agent_message {
                        agent_message.clear();
                        agent_message_truncated = append_bounded(
                            &mut agent_message,
                            &event.message,
                            MAX_SUBAGENT_OUTPUT_BYTES,
                        );
                    }
                    if !is_child_thread && !saw_agent_delta {
                        (self.sink)(HarnessEvent::AgentDelta {
                            acp_session_id: acp_session_id.to_string(),
                            text: event.message,
                        });
                    }
                }
                EventMsg::AgentReasoning(event) if !saw_reasoning_delta => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: acp_session_id.to_string(),
                            text: event.text,
                        });
                    }
                }
                EventMsg::Warning(event) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::Warning {
                            acp_session_id: thread_id.to_string(),
                            message: event.message,
                        });
                    }
                }
                EventMsg::SkillContextLoaded(event) => {
                    turn_skill_resource_ids.extend(
                        event.skills.iter().map(|skill| skill.skill_id.clone()),
                    );
                    if let Some(ahead_turn_id) = ahead_turn_id {
                        let sources = event
                            .skills
                            .into_iter()
                            .map(|skill| crate::TurnSkillSource {
                                skill_id: skill.skill_id,
                                name: skill.name,
                                scope: skill.scope,
                                source_sha256: skill.source_sha256,
                                prompt_sha256: skill.prompt_sha256,
                                selected_by: skill.selected_by,
                            })
                            .collect::<Vec<_>>();
                        if let Err(error) = self
                            .store
                            .set_turn_skill_sources(ahead_turn_id, &sources)
                        {
                            if let Err(interrupt_error) =
                                thread.submit(Op::Interrupt).await
                            {
                                tracing::warn!(
                                    %interrupt_error,
                                    "failed to interrupt managed turn after skill provenance persistence failed"
                                );
                            }
                            return Err(error.context(
                                "persisting managed-turn skill provenance",
                            ));
                        }
                    }
                }
                EventMsg::AuthRecoveryStarted(event)
                | EventMsg::AuthRecoveryCompleted(event) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: thread_id.to_string(),
                            text: format!(
                                "\n{}: {}\n",
                                event.provider, event.message
                            ),
                        });
                    }
                }
                EventMsg::ModelMismatch(event) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: thread_id.to_string(),
                            text: format!(
                                "\nModel endpoint reported {} for a request configured as {}.\n",
                                event.reported_model, event.requested_model
                            ),
                        });
                    }
                }
                EventMsg::ModelVerification(event) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: thread_id.to_string(),
                            text: format!(
                                "\nModel verification required: {:?}.\n",
                                event.verifications
                            ),
                        });
                    }
                }
                EventMsg::SafetyBuffering(event) if event.show_buffering_ui => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: thread_id.to_string(),
                            text: format!(
                                "\nSafety review in progress for {}.\n",
                                event.model
                            ),
                        });
                    }
                }
                EventMsg::DeprecationNotice(event) => {
                    let details = event
                        .details
                        .map(|details| format!(" {details}"))
                        .unwrap_or_default();
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: thread_id.to_string(),
                            text: format!(
                                "\nDeprecated: {}{details}\n",
                                event.summary
                            ),
                        });
                    }
                }
                EventMsg::StreamError(event) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: thread_id.to_string(),
                            text: format!("\nStream retry: {}\n", event.message),
                        });
                    }
                }
                // `PlanDelta` is partial prose, not a complete plan snapshot.
                // Zed's thread model updates structured entries by identity;
                // AHEAD likewise waits for `PlanUpdate` rather than replacing
                // the visible plan with each streamed fragment.
                EventMsg::PlanDelta(_) => {}
                EventMsg::ItemStarted(event) => {
                    if let TurnItem::SubAgentActivity(activity) = event.item {
                        (self.sink)(Self::subagent_tool_call(thread_id, activity));
                    }
                }
                EventMsg::ItemCompleted(event)
                    if matches!(event.item, TurnItem::SubAgentActivity(_)) => {}
                EventMsg::PlanUpdate(update) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::Plan {
                            acp_session_id: thread_id.to_string(),
                            entries: update
                                .plan
                                .into_iter()
                                .map(|entry| HarnessPlanEntry {
                                    content: entry.step,
                                    status: match entry.status {
                                        StepStatus::Pending => "pending",
                                        StepStatus::InProgress => "in_progress",
                                        StepStatus::Completed => "completed",
                                    }
                                    .to_string(),
                                    priority: "normal".to_string(),
                                })
                                .collect(),
                        });
                    }
                }
                EventMsg::TokenCount(event) => {
                    if !is_child_thread && let Some(info) = event.info {
                        (self.sink)(HarnessEvent::Usage {
                            acp_session_id: thread_id.to_string(),
                            total_tokens: u64::try_from(
                                info.total_token_usage.total_tokens.max(0),
                            )
                            .unwrap_or(u64::MAX),
                            context_window: info
                                .model_context_window
                                .and_then(|tokens| u64::try_from(tokens).ok()),
                        });
                    }
                }
                EventMsg::ExecCommandBegin(event) => {
                    self.tool_event(
                        thread_id,
                        event.call_id,
                        event.command.join(" "),
                        "in_progress",
                        "commandExecution",
                    );
                }
                EventMsg::ExecCommandEnd(event) => {
                    let status = match event.status {
                        codex_protocol::protocol::ExecCommandStatus::Completed => {
                            "completed"
                        }
                        codex_protocol::protocol::ExecCommandStatus::Failed => {
                            "failed"
                        }
                        codex_protocol::protocol::ExecCommandStatus::Declined => {
                            "declined"
                        }
                    };
                    self.tool_event(
                        thread_id,
                        event.call_id,
                        event.command.join(" "),
                        status,
                        "commandExecution",
                    );
                }
                EventMsg::PatchApplyBegin(event) => {
                    self.tool_event(
                        thread_id,
                        event.call_id,
                        "File changes".to_string(),
                        "in_progress",
                        "fileChange",
                    );
                }
                EventMsg::PatchApplyUpdated(event) => {
                    let preview =
                        bounded_patch_preview(event.call_id, event.changes);
                    if !preview.changes.is_empty() {
                        (self.sink)(HarnessEvent::PatchApplyProgress {
                            acp_session_id: thread_id.to_string(),
                            preview,
                        });
                    }
                }
                EventMsg::PatchApplyEnd(event) => {
                    let status = if event.success { "completed" } else { "failed" };
                    self.tool_event(
                        thread_id,
                        event.call_id,
                        "File changes".to_string(),
                        status,
                        "fileChange",
                    );
                    if event.success {
                        (self.sink)(HarnessEvent::FileChange {
                            acp_session_id: thread_id.to_string(),
                            status: status.to_string(),
                            changes: file_changes(event.changes),
                        });
                    }
                }
                EventMsg::McpToolCallBegin(event) => {
                    self.tool_event(
                        thread_id,
                        event.call_id,
                        format!(
                            "{} / {}",
                            event.invocation.server, event.invocation.tool
                        ),
                        "in_progress",
                        "mcpToolCall",
                    );
                }
                EventMsg::McpToolCallEnd(event) => {
                    let status = if event.is_success() {
                        "completed"
                    } else {
                        "failed"
                    };
                    self.tool_event(
                        thread_id,
                        event.call_id,
                        format!(
                            "{} / {}",
                            event.invocation.server, event.invocation.tool
                        ),
                        status,
                        "mcpToolCall",
                    );
                }
                EventMsg::ExecApprovalRequest(event) => {
                    thread
                        .submit(Op::ExecApproval {
                            id: event.effective_approval_id(),
                            turn_id: Some(event.turn_id),
                            decision: ReviewDecision::denied(
                                "AHEAD does not allow unscoped shell escalation",
                            ),
                        })
                        .await?;
                }
                EventMsg::ApplyPatchApprovalRequest(event) => {
                    let paths = event
                        .changes
                        .keys()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect::<Vec<_>>();
                    let scope = self.scopes.lock().get(runtime_thread_id).cloned();
                    let accepted =
                        file_change_allowed(mode_id, &paths, scope.as_ref());
                    thread
                        .submit(Op::PatchApproval {
                            id: event.call_id,
                            decision: if accepted {
                                ReviewDecision::Approved
                            } else {
                                ReviewDecision::denied(
                                    "AHEAD Learn mode or edit scope denied this change",
                                )
                            },
                        })
                        .await?;
                }
                EventMsg::RequestUserInput(event) => {
                    let call_id = event.call_id;
                    let host_request_id = uuid::Uuid::new_v4().to_string();
                    let is_blocking = event.is_blocking;
                    let (sender, receiver) = oneshot::channel();
                    {
                        let mut pending = self.pending_inputs.lock();
                        anyhow::ensure!(
                            self.is_running(),
                            "AHEAD agent is shutting down"
                        );
                        pending.entry(thread_id.to_string()).or_default().insert(
                            host_request_id.clone(),
                            PendingUserInput {
                                sender,
                                kind: PendingUserInputKind::AgentQuestion,
                            },
                        );
                    }
                    pending_input_cleanup
                        .request_ids
                        .push(host_request_id.clone());
                    (self.sink)(HarnessEvent::UserInputRequested {
                        acp_session_id: thread_id.to_string(),
                        request_id: host_request_id,
                        is_blocking,
                        questions: event
                            .questions
                            .into_iter()
                            .map(|question| HarnessUserInputQuestion {
                                id: question.id,
                                header: question.header,
                                question: question.question,
                                external_url: None,
                                options: question
                                    .options
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|option| HarnessUserInputOption {
                                        value: option.label.clone(),
                                        label: option.label,
                                        description: option.description,
                                    })
                                    .collect(),
                                default_answers: Vec::new(),
                                allows_other: question.is_other,
                                is_secret: question.is_secret,
                            })
                            .collect(),
                    });
                    let thread = Arc::clone(thread);
                    input_responses.spawn(async move {
                        let Ok(answers) = receiver.await else {
                            return Ok(());
                        };
                        thread
                            .submit(Op::UserInputAnswer {
                                id: call_id,
                                response: RequestUserInputResponse {
                                    answers: answers
                                        .into_iter()
                                        .map(|(id, answers)| {
                                            (id, RequestUserInputAnswer { answers })
                                        })
                                        .collect(),
                                },
                            })
                            .await?;
                        Ok(())
                    });
                }
                EventMsg::ContextCompacted(_) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::ContextCompacted {
                            acp_session_id: thread_id.to_string(),
                        });
                    }
                }
                EventMsg::DynamicToolCallRequest(event) => {
                    let call_id = event.call_id;
                    let tool = event.tool;
                    let is_subagent_tool = tool == AHEAD_SPAWN_AGENT_TOOL;
                    let arguments = event.arguments;
                    let turn_skill_resource_ids = turn_skill_resource_ids.clone();
                    dynamic_tool_calls.push_back(Box::pin(async move {
                        let result = match tool.as_str() {
                            AHEAD_READ_EDITOR_BUFFER_TOOL => Some(
                                self.read_editor_buffer(
                                    runtime_thread_id,
                                    thread_id,
                                    arguments,
                                )
                                .await,
                            ),
                            AHEAD_FILE_SEARCH_TOOL => {
                                let settings = self
                                    .settings
                                    .lock()
                                    .get(runtime_thread_id)
                                    .cloned()
                                    .with_context(|| {
                                        format!("AHEAD agent settings for `{runtime_thread_id}` are missing")
                                    })?;
                                let scope =
                                    self.scopes.lock().get(runtime_thread_id).cloned();
                                Some(
                                    self.file_search(
                                        thread_id,
                                        &settings,
                                        scope.as_ref(),
                                        arguments,
                                    )
                                    .await,
                                )
                            }
                            AHEAD_SKILL_RESOURCE_READ_TOOL => {
                                let settings = self
                                    .settings
                                    .lock()
                                    .get(runtime_thread_id)
                                    .cloned()
                                    .with_context(|| {
                                        format!("AHEAD agent settings for `{runtime_thread_id}` are missing")
                                    })?;
                                Some(
                                    Box::pin(self.read_skill_resource(
                                        &settings,
                                        ahead_turn_id,
                                        arguments,
                                        &turn_skill_resource_ids,
                                    ))
                                    .await,
                                )
                            }
                            AHEAD_PRESENT_CODE_TOOL
                            | AHEAD_MOVE_CODE_POINTER_TOOL
                            | AHEAD_CLEAR_PRESENTATION_TOOL
                            | AHEAD_SPEAK_TEXT_TOOL
                            | AHEAD_STOP_SPEAKING_TOOL => {
                                match self.presentation_action(
                                    runtime_thread_id,
                                    &tool,
                                    arguments,
                                ) {
                                    Ok(action) => Some(
                                        self.request_editor_presentation(
                                            thread_id, action,
                                        )
                                        .await,
                                    ),
                                    Err(error) => Some(Err(error)),
                                }
                            }
                            AHEAD_SPAWN_AGENT_TOOL => Some(
                                Box::pin(self.handle_spawn_agent_tool(
                                    runtime_thread_id,
                                    thread_id,
                                    mode_id,
                                    call_id.clone(),
                                    arguments,
                                ))
                                .await,
                            ),
                            _ => None,
                        };
                        Ok::<_, anyhow::Error>((call_id, tool, is_subagent_tool, result))
                    }));
                }
                EventMsg::RequestPermissions(event) => {
                    thread
                        .submit(Op::RequestPermissionsResponse {
                            id: event.call_id,
                            response: RequestPermissionsResponse {
                                permissions: RequestPermissionProfile::default(),
                                scope: PermissionGrantScope::Turn,
                            },
                        })
                        .await?;
                }
                EventMsg::ElicitationRequest(event) => {
                    let approval = mcp_approval_elicitation_question(&event);
                    let form = if approval.is_none() {
                        match &event.request {
                            ElicitationRequest::Form {
                                message,
                                requested_schema,
                                ..
                            } => {
                                match McpForm::new(
                                    &event.server_name,
                                    message,
                                    requested_schema,
                                ) {
                                    Ok(form) => Some(form),
                                    Err(error) => {
                                        tracing::warn!(%error, "unsupported MCP elicitation form");
                                        None
                                    }
                                }
                            }
                            ElicitationRequest::Url { .. } => None,
                        }
                    } else {
                        None
                    };
                    let url_question = if approval.is_none() && form.is_none() {
                        mcp_url_elicitation_question(&event)
                    } else {
                        None
                    };
                    let questions = if let Some((_, question)) = &approval {
                        vec![question.clone()]
                    } else if let Some((_, questions)) = &form {
                        questions.clone()
                    } else if let Some(question) = &url_question {
                        vec![question.clone()]
                    } else {
                        thread
                            .submit(Op::ResolveElicitation {
                                server_name: event.server_name,
                                request_id: event.id,
                                decision: ElicitationAction::Decline,
                                content: None,
                                meta: None,
                            })
                            .await?;
                        continue;
                    };
                    let is_url = url_question.is_some();
                    let (sender, receiver) = oneshot::channel();
                    let host_request_id = if form.is_some() {
                        format!("mcp-form-{}", uuid::Uuid::new_v4())
                    } else if is_url {
                        format!("mcp-url-{}", uuid::Uuid::new_v4())
                    } else {
                        uuid::Uuid::new_v4().to_string()
                    };
                    let allow_session = questions.iter().any(|question| {
                        question
                            .options
                            .iter()
                            .any(|option| option.label == "Allow for this session")
                    });
                    {
                        let mut pending = self.pending_inputs.lock();
                        anyhow::ensure!(
                            self.is_running(),
                            "AHEAD agent is shutting down"
                        );
                        pending.entry(thread_id.to_string()).or_default().insert(
                            host_request_id.clone(),
                            PendingUserInput {
                                sender,
                                kind: if let Some((form, _)) = &form {
                                    PendingUserInputKind::McpForm(form.clone())
                                } else if is_url {
                                    PendingUserInputKind::McpUrl
                                } else {
                                    PendingUserInputKind::AgentQuestion
                                },
                            },
                        );
                    }
                    pending_input_cleanup
                        .request_ids
                        .push(host_request_id.clone());
                    (self.sink)(HarnessEvent::UserInputRequested {
                        acp_session_id: thread_id.to_string(),
                        request_id: host_request_id,
                        is_blocking: true,
                        questions,
                    });
                    let thread = Arc::clone(thread);
                    input_responses.spawn(async move {
                        let (decision, content, meta) = match (receiver.await, form, is_url) {
                            (Ok(answers), Some((form, _)), _) => match form.response(&answers) {
                                Ok((decision, content)) => (decision, content, None),
                                Err(error) => {
                                    tracing::warn!(%error, "invalid MCP form response");
                                    (ElicitationAction::Decline, None, None)
                                }
                            },
                            (Ok(answers), None, true) => match mcp_url_action(&answers) {
                                Ok(decision) => (decision, None, None),
                                Err(error) => {
                                    tracing::warn!(%error, "invalid MCP URL response");
                                    (ElicitationAction::Decline, None, None)
                                }
                            },
                            (Ok(answers), None, false) => {
                                match approval
                                    .as_ref()
                                    .and_then(|(request_id, _)| answers.get(request_id))
                                    .map(Vec::as_slice)
                                {
                                    Some([answer]) if answer == "Allow" => {
                                        (ElicitationAction::Accept, None, None)
                                    }
                                    Some([answer])
                                        if allow_session
                                            && answer == "Allow for this session" =>
                                    {
                                        (
                                            ElicitationAction::Accept,
                                            None,
                                            Some(json!({ "persist": PERSIST_SESSION })),
                                        )
                                    }
                                    Some([answer]) if answer == "Cancel" => {
                                        (ElicitationAction::Cancel, None, None)
                                    }
                                    _ => (ElicitationAction::Decline, None, None),
                                }
                            }
                            (Err(_), _, _) => (ElicitationAction::Decline, None, None),
                        };
                        thread
                            .submit(Op::ResolveElicitation {
                                server_name: event.server_name,
                                request_id: event.id,
                                decision,
                                content,
                                meta,
                            })
                            .await?;
                        Ok(())
                    });
                }
                EventMsg::Error(event) => {
                    terminal_error = Some(event.message);
                }
                EventMsg::TurnComplete(event) => {
                    anyhow::ensure!(
                        dynamic_tool_calls.is_empty(),
                        "AHEAD agent turn completed with pending dynamic client tools"
                    );
                    if let Some(error) = event.error {
                        bail!("{}", error.message);
                    }
                    if let Some(error) = terminal_error {
                        bail!("{error}");
                    }
                    if return_agent_message && agent_message_truncated {
                        append_bounded_output_marker(
                            &mut agent_message,
                            MAX_SUBAGENT_OUTPUT_BYTES,
                        );
                    }
                    return Ok(if return_agent_message {
                        agent_message
                    } else {
                        "completed".to_string()
                    });
                }
                EventMsg::TurnAborted(event) => {
                    while let Some(result) = dynamic_tool_calls.next().await {
                        if let Err(error) = result {
                            tracing::warn!(%error, "AHEAD dynamic tool failed during interrupted-turn cleanup");
                        }
                    }
                    bail!("turn aborted: {:?}", event.reason);
                }
                EventMsg::ShutdownComplete => {
                    bail!("AHEAD agent shut down during the turn");
                }
                // These events are already represented by their streaming
                // counterparts above, or are intentionally not projected into
                // AHEAD's normalized event model.
                EventMsg::AgentReasoning(_) | EventMsg::SafetyBuffering(_) => {}
                other => {
                    tracing::warn!(
                        event_kind = %other,
                        "native runtime event has no AHEAD presentation mapping; event was not projected"
                    );
                }
            }
        }
    }

    fn ensure_thread_history_healthy(&self, thread_id: &str) -> Result<()> {
        if let Some(error) = self.thread_store.persistence_error(thread_id) {
            bail!(
                "AHEAD agent history was not saved; restart AHEAD before retrying and review any tools that ran: {error}"
            );
        }
        Ok(())
    }

    async fn handle_spawn_agent_tool(
        &self,
        parent_thread_id: &str,
        acp_session_id: &str,
        mode_id: &str,
        call_id: String,
        arguments: Value,
    ) -> Result<String> {
        let label = arguments
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty() && label.len() <= 80)
            .unwrap_or("Spawn agent")
            .to_string();
        self.tool_event(
            acp_session_id,
            call_id.clone(),
            label.clone(),
            "in_progress",
            "subagent",
        );
        let result = match serde_json::from_value::<SpawnAgentToolInput>(arguments) {
            Ok(input) => {
                Box::pin(self.run_subagent_turn(
                    parent_thread_id,
                    acp_session_id,
                    mode_id,
                    input,
                ))
                .await
            }
            Err(error) => {
                Err(anyhow!("invalid AHEAD spawn_agent arguments: {error}"))
            }
        };
        self.tool_event(
            acp_session_id,
            call_id,
            label,
            if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            "subagent",
        );
        result
    }

    async fn run_subagent_turn(
        &self,
        parent_thread_id: &str,
        acp_session_id: &str,
        mode_id: &str,
        input: SpawnAgentToolInput,
    ) -> Result<String> {
        let SpawnAgentToolInput {
            label,
            message,
            session_id,
        } = input;
        let admission = self.thread_admission.read().await;
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
        validate_spawn_agent_input(&label, &message)?;
        let label = label.trim();
        let parent_id = ThreadId::from_string(parent_thread_id)
            .map_err(|error| anyhow!("invalid AHEAD parent thread ID: {error}"))?;
        let parent_depth = self
            .thread_depths
            .lock()
            .get(parent_thread_id)
            .copied()
            .context("AHEAD parent thread depth is unavailable")?;
        anyhow::ensure!(
            parent_depth < MAX_SUBAGENT_DEPTH,
            "AHEAD subagent depth limit reached"
        );
        let child_depth = parent_depth + 1;
        let settings = self
            .settings
            .lock()
            .get(parent_thread_id)
            .cloned()
            .context("AHEAD parent thread settings are unavailable")?;
        let scope = self.scopes.lock().get(parent_thread_id).cloned();
        let config = Self::build_config(
            &self.runtime_home,
            &settings.cwd,
            settings.model.as_deref(),
            settings.model_provider.as_deref(),
            mcp_server_policy(
                &settings.mode_id,
                scope.as_ref().is_some_and(|scope| scope.read_only),
            ),
        )
        .await?;

        let is_resuming = session_id.is_some();
        let child_session_id = match session_id {
            Some(child_session_id) => {
                let children = self
                    .store
                    .list_native_agent_children(parent_thread_id, None)
                    .map_err(|error| {
                        subagent_tool_error(&child_session_id, error)
                    })?;
                anyhow::ensure!(
                    children.iter().any(|child| child == &child_session_id),
                    "AHEAD can resume only a direct child of the current thread"
                );
                let parsed_child_id = ThreadId::from_string(&child_session_id)
                    .map_err(|error| {
                        anyhow!("invalid AHEAD child thread ID: {error}")
                    })?;
                let child = if let Some(child) =
                    self.threads.lock().get(&child_session_id).cloned()
                {
                    child
                } else {
                    Box::pin(self.manager.resume_thread_by_id_with_tools(
                        parsed_child_id,
                        config,
                        ahead_dynamic_tools(false),
                    ))
                    .await
                    .map_err(|error| {
                        anyhow!("failed to resume AHEAD subagent: {error}")
                    })?
                    .thread
                };
                self.threads.lock().insert(child_session_id.clone(), child);
                child_session_id
            }
            None => {
                let options = StartThreadOptions {
                    session_source: Some(SessionSource::SubAgent(
                        SubAgentSource::ThreadSpawn {
                            parent_thread_id: parent_id,
                            depth: i32::from(child_depth),
                            agent_path: None,
                            agent_nickname: Some(label.to_string()),
                        },
                    )),
                    dynamic_tools: ahead_dynamic_tools(false),
                    ..StartThreadOptions::new(config)
                };
                let child = Box::pin(
                    self.manager.spawn_subagent_session(parent_id, options),
                )
                .await
                .map_err(|error| {
                    anyhow!("failed to create AHEAD subagent thread: {error}")
                })?;
                let child_session_id = child.thread_id.to_string();
                self.threads
                    .lock()
                    .insert(child_session_id.clone(), child.thread);
                child_session_id
            }
        };

        self.settings
            .lock()
            .insert(child_session_id.clone(), settings.clone());
        self.thread_depths
            .lock()
            .insert(child_session_id.clone(), child_depth);
        if let Some(scope) = scope {
            self.scopes.lock().insert(child_session_id.clone(), scope);
        } else {
            self.scopes.lock().remove(&child_session_id);
        }
        let child = self
            .threads
            .lock()
            .get(&child_session_id)
            .cloned()
            .ok_or_else(|| {
                subagent_tool_error(
                    &child_session_id,
                    "AHEAD child thread was not registered",
                )
            })?;
        self.ensure_thread_history_healthy(&child_session_id)
            .map_err(|error| subagent_tool_error(&child_session_id, error))?;
        if is_resuming {
            self.store
                .set_native_agent_edge_status(
                    &child_session_id,
                    crate::NativeAgentEdgeStatus::Open,
                )
                .map_err(|error| subagent_tool_error(&child_session_id, error))?;
        }
        drop(admission);
        let turn_result = async {
            let child_scope = self.scopes.lock().get(&child_session_id).cloned();
            let permission_profile =
                permission_profile(mode_id, &settings.cwd, child_scope.as_ref())?;
            let request = TurnInputRequest::user_input(vec![UserInput::Text {
                text: message.to_string(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                approval_policy: Some(AskForApproval::OnRequest),
                permission_profile: Some(permission_profile),
                model: settings.model.clone(),
                ..Default::default()
            });
            match child.start_turn_if_idle(request).await? {
                StartIfIdleSubmission::Started { .. } => {}
                StartIfIdleSubmission::NotSubmitted { reason } => {
                    bail!("AHEAD subagent did not start its turn: {reason:?}");
                }
            }

            Box::pin(self.consume_turn(
                acp_session_id,
                &child_session_id,
                &child,
                mode_id,
                true,
                None,
            ))
            .await
        }
        .await;
        if let Err(error) = self.store.set_native_agent_edge_status(
            &child_session_id,
            crate::NativeAgentEdgeStatus::Closed,
        ) {
            return match turn_result {
                Ok(_) => Err(subagent_tool_error(
                    &child_session_id,
                    format!(
                        "AHEAD subagent finished but its Turso edge could not be closed: {error}"
                    ),
                )),
                Err(turn_error) => Err(subagent_tool_error(
                    &child_session_id,
                    format!(
                        "AHEAD subagent turn failed: {turn_error}; its Turso edge also could not be closed: {error}"
                    ),
                )),
            };
        }
        let output = turn_result.map_err(|error| {
            subagent_tool_error(
                &child_session_id,
                format!("AHEAD subagent turn failed: {error}"),
            )
        })?;
        Ok(json!({ "session_id": child_session_id, "output": output }).to_string())
    }

    fn tool_event(
        &self,
        thread_id: &str,
        call_id: String,
        title: String,
        status: &str,
        kind: &str,
    ) {
        (self.sink)(HarnessEvent::ToolCall {
            acp_session_id: thread_id.to_string(),
            call_id,
            title,
            status: status.to_string(),
            kind: kind.to_string(),
        });
    }

    fn subagent_tool_call(
        parent_thread_id: &str,
        activity: SubAgentActivityItem,
    ) -> HarnessEvent {
        let status = match activity.kind {
            SubAgentActivityKind::Started | SubAgentActivityKind::Interacted => {
                "in_progress"
            }
            SubAgentActivityKind::Interrupted => "failed",
            SubAgentActivityKind::Completed => "completed",
        };
        HarnessEvent::ToolCall {
            acp_session_id: parent_thread_id.to_string(),
            call_id: format!("subagent-{}", activity.agent_thread_id),
            title: format!("Subagent {}", activity.agent_path.name()),
            status: status.to_string(),
            kind: "subagent".to_string(),
        }
    }

    pub fn cancel(&self, thread_id: &str) -> Result<()> {
        self.pending_inputs.lock().remove(thread_id);
        self.pending_buffer_snapshots
            .lock()
            .retain(|(pending_thread, _), _| pending_thread != thread_id);
        self.pending_editor_presentations
            .lock()
            .retain(|(pending_thread, _), _| pending_thread != thread_id);
        let parent_thread = self
            .threads
            .lock()
            .get(thread_id)
            .cloned()
            .with_context(|| {
                format!("AHEAD agent thread `{thread_id}` is not loaded")
            })?;

        let parent_id = ThreadId::from_string(thread_id);
        let mut first_error = None;
        let mut descendant_ids = match parent_id {
            Ok(parent_id) => match self
                .runtime
                .block_on(self.manager.list_agent_subtree_thread_ids(parent_id))
            {
                Ok(thread_ids) => thread_ids
                    .into_iter()
                    .filter(|candidate| *candidate != parent_id)
                    .rev()
                    .collect::<Vec<_>>(),
                Err(error) => {
                    first_error = Some(anyhow!(
                        "failed to enumerate AHEAD subagents before cancellation: {error}"
                    ));
                    Vec::new()
                }
            },
            Err(error) => {
                first_error = Some(anyhow!(
                    "invalid AHEAD thread ID while cancelling subagents: {error}"
                ));
                Vec::new()
            }
        };

        let loaded_threads = self.threads.lock();
        let mut threads = descendant_ids
            .drain(..)
            .filter_map(|descendant_id| {
                loaded_threads.get(&descendant_id.to_string()).cloned()
            })
            .collect::<Vec<_>>();
        drop(loaded_threads);
        threads.push(parent_thread);

        for thread in threads {
            if let Err(error) = self.runtime.block_on(thread.submit(Op::Interrupt))
                && first_error.is_none()
            {
                first_error = Some(anyhow!(
                    "failed to interrupt an AHEAD agent thread during cancellation: {error}"
                ));
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub fn compact(&self, thread_id: &str) -> Result<String> {
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
        self.ensure_thread_history_healthy(thread_id)?;
        let thread =
            self.threads
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent thread `{thread_id}` is not loaded")
                })?;
        let mode_id = self
            .settings
            .lock()
            .get(thread_id)
            .map(|settings| settings.mode_id.clone())
            .with_context(|| {
                format!("AHEAD agent settings for `{thread_id}` are missing")
            })?;
        self.runtime.block_on(async {
            thread.submit(Op::Compact).await?;
            Box::pin(
                self.consume_turn(
                    thread_id, thread_id, &thread, &mode_id, false, None,
                ),
            )
            .await
        })
    }

    pub fn answer_user_input(
        &self,
        thread_id: &str,
        request_id: &str,
        answers: HashMap<String, Vec<String>>,
    ) -> Result<()> {
        let mut pending = self.pending_inputs.lock();
        let requests = pending.get_mut(thread_id).with_context(|| {
            format!("AHEAD agent input request `{request_id}` is no longer pending")
        })?;
        let request = requests.get(request_id).with_context(|| {
            format!("AHEAD agent input request `{request_id}` is no longer pending")
        })?;
        match &request.kind {
            PendingUserInputKind::McpForm(form) => {
                form.response(&answers)?;
            }
            PendingUserInputKind::McpUrl => {
                mcp_url_action(&answers)?;
            }
            PendingUserInputKind::AgentQuestion => {
                validate_mcp_approval_answers(&answers)?;
            }
        }
        let request = requests
            .remove(request_id)
            .context("validated input disappeared")?;
        request.sender.send(answers).map_err(|_| {
            anyhow!("AHEAD agent input request `{request_id}` was cancelled")
        })
    }

    pub fn answer_buffer_snapshots(
        &self,
        thread_id: &str,
        request_id: &str,
        buffers: Vec<AgentBufferSnapshot>,
        error: Option<String>,
    ) -> Result<()> {
        let sender = self
            .pending_buffer_snapshots
            .lock()
            .remove(&(thread_id.to_string(), request_id.to_string()))
            .with_context(|| {
                format!(
                    "AHEAD buffer snapshot request `{request_id}` is no longer pending"
                )
            })?;
        let response = match error {
            Some(error) => Err(error),
            None => validate_agent_buffer_snapshots(&buffers).map(|()| buffers),
        };
        sender.send(response).map_err(|_| {
            anyhow!("AHEAD buffer snapshot request `{request_id}` was cancelled")
        })
    }

    pub fn answer_editor_presentation(
        &self,
        thread_id: &str,
        request_id: &str,
        applied: bool,
        message: String,
    ) -> Result<()> {
        let sender = self
            .pending_editor_presentations
            .lock()
            .remove(&(thread_id.to_string(), request_id.to_string()))
            .with_context(|| {
                format!(
                    "AHEAD editor presentation `{request_id}` is no longer pending"
                )
            })?;
        sender.send((applied, message)).map_err(|_| {
            anyhow!("AHEAD editor presentation `{request_id}` was cancelled")
        })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn shutdown(&self) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }
        self.pending_inputs.lock().clear();
        self.pending_buffer_snapshots.lock().clear();
        self.pending_editor_presentations.lock().clear();
        self.runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let result = tokio::time::timeout_at(deadline, async {
                // Include creations already in flight, and fence new root/child
                // admissions before taking the manager's shutdown snapshot.
                let _admission = self.thread_admission.write().await;
                self.manager
                    .shutdown_all_threads_bounded(
                        deadline
                            .saturating_duration_since(tokio::time::Instant::now()),
                    )
                    .await
            })
            .await;
            match result {
                Ok(report)
                    if report.submit_failed.is_empty()
                        && report.timed_out.is_empty() => {}
                Ok(report) => tracing::error!(
                    ?report,
                    "AHEAD agent threads did not finish shutdown"
                ),
                Err(error) => {
                    tracing::error!(%error, "AHEAD agent shutdown deadline elapsed");
                }
            }
        });
    }

    async fn current_buffer_snapshots(
        &self,
        thread_id: &str,
        include_document_symbols: bool,
    ) -> Result<Vec<AgentBufferSnapshot>> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let key = (thread_id.to_string(), request_id.clone());
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending_buffer_snapshots.lock();
            anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
            pending.insert(key.clone(), sender);
        }
        (self.sink)(HarnessEvent::BufferSnapshotsRequested {
            acp_session_id: thread_id.to_string(),
            request_id,
            include_document_symbols,
        });
        let result = tokio::time::timeout(Duration::from_secs(10), receiver).await;
        self.pending_buffer_snapshots.lock().remove(&key);
        match result {
            Ok(Ok(Ok(buffers))) => Ok(buffers),
            Ok(Ok(Err(error))) => bail!(error),
            Ok(Err(_)) => bail!("AHEAD editor buffer request was cancelled"),
            Err(_) => bail!("AHEAD editor did not answer the buffer request"),
        }
    }

    async fn read_editor_buffer(
        &self,
        thread_id: &str,
        acp_session_id: &str,
        arguments: Value,
    ) -> Result<String> {
        let settings =
            self.settings
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent settings for `{thread_id}` are missing")
                })?;
        let scope = self.scopes.lock().get(thread_id).cloned();
        let workspace = scope
            .as_ref()
            .map(|scope| scope.workspace.as_path())
            .unwrap_or(settings.cwd.as_path());
        let relative = read_editor_buffer_path(
            &arguments,
            workspace,
            scope.as_ref().map(|scope| scope.allowed_paths.as_slice()),
        )?;
        let buffers = self.current_buffer_snapshots(acp_session_id, false).await?;
        format_read_editor_buffer(&relative, &buffers)
    }

    fn presentation_action(
        &self,
        thread_id: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<AgentPresentationAction> {
        let settings =
            self.settings
                .lock()
                .get(thread_id)
                .cloned()
                .with_context(|| {
                    format!("AHEAD agent settings for `{thread_id}` are missing")
                })?;
        let scope = self.scopes.lock().get(thread_id).cloned();
        let workspace = scope
            .as_ref()
            .map(|scope| scope.workspace.as_path())
            .unwrap_or(settings.cwd.as_path());
        parse_presentation_action(
            tool,
            arguments,
            workspace,
            scope.as_ref().map(|scope| scope.allowed_paths.as_slice()),
        )
    }

    async fn request_editor_presentation(
        &self,
        thread_id: &str,
        action: AgentPresentationAction,
    ) -> Result<String> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let key = (thread_id.to_string(), request_id.clone());
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending_editor_presentations.lock();
            anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
            pending.insert(key.clone(), sender);
        }
        (self.sink)(HarnessEvent::EditorPresentationRequested {
            acp_session_id: thread_id.to_string(),
            request_id,
            action,
        });
        let result = tokio::time::timeout(Duration::from_secs(15), receiver).await;
        self.pending_editor_presentations.lock().remove(&key);
        match result {
            Ok(Ok((true, message))) => Ok(message),
            Ok(Ok((false, message))) => bail!(message),
            Ok(Err(_)) => bail!("AHEAD editor presentation was cancelled"),
            Err(_) => bail!("AHEAD editor did not acknowledge the presentation"),
        }
    }

    async fn file_search(
        &self,
        acp_session_id: &str,
        settings: &SessionSettings,
        scope: Option<&AgentScope>,
        arguments: Value,
    ) -> Result<String> {
        let pattern = arguments
            .get("pattern")
            .and_then(Value::as_str)
            .filter(|pattern| !pattern.is_empty())
            .context("AHEAD file_search requires a non-empty pattern")?
            .to_string();
        let cursor = arguments
            .get("cursor")
            .map(|value| {
                parse_file_search_cursor(
                    value
                        .as_str()
                        .context("AHEAD file_search cursor must be a string")?,
                )
            })
            .transpose()?;
        ensure!(
            arguments.get("offset").is_none(),
            "AHEAD file_search accepts a cursor, not an offset"
        );
        let offset = cursor.as_ref().map_or(0, |cursor| cursor.offset);
        if offset > MAX_AGENT_SEARCH_OFFSET {
            bail!("AHEAD file_search offset exceeds {MAX_AGENT_SEARCH_OFFSET}");
        }
        let options = ahead_core::search::FileSearchOptions {
            pattern,
            case_sensitive: arguments
                .get("case_sensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            whole_word: arguments
                .get("whole_word")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            is_regex: arguments
                .get("is_regex")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            max_results: offset + AGENT_SEARCH_PAGE_SIZE + 1,
        };
        let workspace = scope
            .map(|scope| scope.workspace.clone())
            .unwrap_or_else(|| settings.cwd.clone());
        let include_pattern = arguments
            .get("include_pattern")
            .map(|value| {
                value
                    .as_str()
                    .context("AHEAD file_search include_pattern must be a string")
            })
            .transpose()?;
        let exclude_pattern = arguments
            .get("exclude_pattern")
            .map(|value| {
                value
                    .as_str()
                    .context("AHEAD file_search exclude_pattern must be a string")
            })
            .transpose()?;
        let path_filter = ahead_core::search::WorkspacePathFilter::new(
            &workspace,
            include_pattern,
            exclude_pattern,
        )?;
        let allowed_paths = scope.map(|scope| scope.allowed_paths.clone());
        let snapshots = self.current_buffer_snapshots(acp_session_id, true).await?;
        let (overrides, symbol_context) =
            open_buffer_overrides(&workspace, allowed_paths.as_deref(), snapshots);
        let query_fingerprint = file_search_query_fingerprint(
            &workspace,
            &options,
            include_pattern,
            exclude_pattern,
            allowed_paths.as_deref(),
            &overrides,
        );
        if let Some(cursor) = &cursor {
            ensure!(
                cursor.query_fingerprint == query_fingerprint,
                "AHEAD file_search cursor belongs to a different query; restart without a cursor"
            );
        }
        let search_index = self
            .file_index
            .clone()
            .filter(|index| index.workspace() == workspace);
        let display_workspace = workspace.clone();
        let keep_running = Arc::new(AtomicBool::new(true));
        let snapshot_keep_running = keep_running.clone();
        let cancellation = SearchCancellation(keep_running.clone());
        let (search_revision, results) = tokio::task::spawn_blocking(move || {
            let (search_revision, indexed_files) = match search_index.as_ref() {
                Some(index) => {
                    let (search_revision, files) = index
                        .snapshot_with_search_revision_while(move || {
                            snapshot_keep_running.load(Ordering::Relaxed)
                        })
                        .map(|(_, revision, files)| (revision, files))
                        .ok_or(ahead_core::search::FileSearchError::Cancelled)?;
                    (search_revision, Some(files))
                }
                None => (0, None),
            };
            let disk_paths: Box<dyn Iterator<Item = PathBuf> + '_> =
                if let Some(files) = indexed_files.as_ref() {
                    Box::new(
                        files
                            .iter()
                            .filter(|path| {
                                ahead_core::search::is_agent_visible_path(
                                    &workspace,
                                    path,
                                )
                            })
                            .cloned(),
                    )
                } else {
                    Box::new(ahead_core::search::agent_workspace_paths(&workspace))
                };
            let paths =
                ahead_core::search::new_open_buffer_paths(&workspace, &overrides)
                    .into_iter()
                    .chain(disk_paths.filter(|path| {
                        allowed_paths.as_ref().is_none_or(|allowed| {
                            path.to_str().is_some_and(|path| {
                                path_is_allowed(path, allowed, &workspace)
                            })
                        })
                    }))
                    .filter(|path| path_filter.matches(path));
            let results = ahead_core::search::search_paths_with_overrides(
                ahead_core::search::SearchScope::Workspace(&workspace),
                paths,
                &overrides,
                &options,
                move || keep_running.load(Ordering::Relaxed),
            )?;
            ensure!(
                search_index
                    .as_ref()
                    .is_none_or(|index| index.search_revision() == search_revision),
                "AHEAD workspace changed during file_search; restart from the first page"
            );
            Ok((search_revision, results))
        })
        .await
        .context("AHEAD file_search worker failed")??;
        drop(cancellation);

        if let Some(cursor) = &cursor {
            validate_file_search_cursor(
                cursor,
                search_revision,
                &query_fingerprint,
                &results,
            )?;
        }

        format_file_search_results(
            &display_workspace,
            results,
            offset,
            search_revision,
            &query_fingerprint,
            &symbol_context,
        )
    }

    async fn read_skill_resource(
        &self,
        settings: &SessionSettings,
        ahead_turn_id: Option<&str>,
        arguments: Value,
        turn_skill_resource_ids: &HashSet<String>,
    ) -> Result<String> {
        let package = arguments
            .get("package")
            .and_then(Value::as_str)
            .context("skill resource package must be a string")?;
        validate_skill_handle("package", package)?;
        ensure!(
            turn_skill_resource_ids.contains(package),
            "host skill resource was not selected for this turn"
        );
        let resource = match arguments.get("resource") {
            Some(value) => {
                let resource =
                    value.as_str().context("skill resource must be a string")?;
                validate_skill_handle("resource", resource)?;
                Some(resource)
            }
            None => None,
        };
        let cursor = match arguments.get("cursor") {
            Some(value) => {
                let cursor = value
                    .as_str()
                    .context("skill resource cursor must be a string")?;
                ensure!(cursor.len() <= 32, "skill resource cursor is invalid");
                Some(cursor)
            }
            None => None,
        };

        let snapshot = Box::pin(self.load_skill_snapshot(
            &settings.cwd,
            settings.model.as_deref(),
            settings.model_provider.as_deref(),
            false,
        ))
        .await?;
        let read = snapshot
            .read_package_resource(package, resource)
            .await
            .map_err(|_| anyhow!("host skill resource could not be read"))?;
        let response = page_skill_resource_result(
            resource.unwrap_or(package),
            &read.contents,
            cursor,
        )?;
        if let (Some(ahead_turn_id), Some(resource_path)) =
            (ahead_turn_id, read.relative_path.as_deref())
        {
            self.store.append_turn_skill_reference(
                ahead_turn_id,
                &crate::TurnSkillReference {
                    skill_id: read.skill_id,
                    skill_name: read.skill_name,
                    scope: read.scope,
                    resource_path: resource_path.to_string(),
                    source_sha256: format!(
                        "{:x}",
                        Sha256::digest(read.contents.as_bytes())
                    ),
                    response_sha256: format!(
                        "{:x}",
                        Sha256::digest(response.as_bytes())
                    ),
                    loaded_by: "agent".to_string(),
                },
            )?;
        }
        Ok(response)
    }
}

struct SearchCancellation(Arc<AtomicBool>);

impl Drop for SearchCancellation {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

const AHEAD_FILE_SEARCH_TOOL: &str = "file_search";
const AHEAD_SKILL_RESOURCE_READ_TOOL: &str = "skill_resource_read";
const AHEAD_SPAWN_AGENT_TOOL: &str = "spawn_agent";
const AGENT_SEARCH_PAGE_SIZE: usize = 20;
const MAX_AGENT_SEARCH_OFFSET: usize = 5_000;
const MAX_AGENT_SEARCH_CURSOR_BYTES: usize = 192;
// ponytail: cap repeated LSP ancestry at eight levels to bound result tokens; raise only if real projects need deeper context.
const MAX_AGENT_SEARCH_SYMBOL_CONTEXT_DEPTH: usize = 8;
const MAX_SKILL_HANDLE_BYTES: usize = 2_048;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnAgentToolInput {
    label: String,
    message: String,
    #[serde(default, deserialize_with = "deserialize_agent_session_id")]
    session_id: Option<String>,
}

fn validate_spawn_agent_input(label: &str, message: &str) -> Result<()> {
    anyhow::ensure!(
        !label.trim().is_empty() && label.chars().count() <= 80,
        "AHEAD spawn_agent requires a label of at most 80 characters"
    );
    anyhow::ensure!(
        !message.trim().is_empty() && message.chars().count() <= 65_536,
        "AHEAD spawn_agent requires a message of at most 65,536 characters"
    );
    Ok(())
}

fn deserialize_agent_session_id<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if value
        .as_str()
        .is_some_and(|session_id| session_id.trim().is_empty())
    {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

fn subagent_tool_error(
    session_id: &str,
    error: impl std::fmt::Display,
) -> anyhow::Error {
    anyhow!(
        "{}",
        json!({"session_id": session_id, "error": error.to_string()})
    )
}

const MAX_SKILL_RESOURCE_PAGE_BYTES: usize = 48 * 1024;
const MAX_SUBAGENT_DEPTH: u8 = 1;
const MAX_SUBAGENT_OUTPUT_BYTES: usize = 32_768;

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentFileSearchCursor {
    offset: usize,
    search_revision: u64,
    query_fingerprint: String,
    prefix_fingerprint: String,
}

fn update_search_hash(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn file_search_query_fingerprint(
    workspace: &Path,
    options: &ahead_core::search::FileSearchOptions,
    include_pattern: Option<&str>,
    exclude_pattern: Option<&str>,
    allowed_paths: Option<&[String]>,
    overrides: &HashMap<PathBuf, String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"AHEAD file_search query v2");
    update_search_hash(&mut hasher, workspace.to_string_lossy().as_bytes());
    update_search_hash(&mut hasher, options.pattern.as_bytes());
    hasher.update([
        u8::from(options.case_sensitive),
        u8::from(options.whole_word),
        u8::from(options.is_regex),
    ]);
    match include_pattern {
        Some(pattern) => {
            hasher.update([1]);
            update_search_hash(&mut hasher, pattern.as_bytes());
        }
        None => hasher.update([0]),
    }
    match exclude_pattern {
        Some(pattern) => {
            hasher.update([1]);
            update_search_hash(&mut hasher, pattern.as_bytes());
        }
        None => hasher.update([0]),
    }
    if let Some(allowed_paths) = allowed_paths {
        hasher.update([1]);
        let mut allowed_paths =
            allowed_paths.iter().map(String::as_str).collect::<Vec<_>>();
        allowed_paths.sort_unstable();
        hasher.update((allowed_paths.len() as u64).to_be_bytes());
        for path in allowed_paths {
            update_search_hash(&mut hasher, path.as_bytes());
        }
    } else {
        hasher.update([0]);
    }
    let mut buffers = overrides.iter().collect::<Vec<_>>();
    buffers.sort_unstable_by_key(|(left, _)| *left);
    for (path, contents) in buffers {
        update_search_hash(&mut hasher, path.to_string_lossy().as_bytes());
        update_search_hash(&mut hasher, contents.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn file_search_prefix_fingerprint(
    results: &[(PathBuf, Vec<ahead_core::search::FileSearchMatch>)],
    count: usize,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"AHEAD file_search prefix v1");
    let mut remaining = count;
    'paths: for (path, matches) in results {
        let path = path.to_string_lossy();
        for line_match in matches {
            if remaining == 0 {
                break 'paths;
            }
            update_search_hash(&mut hasher, path.as_bytes());
            hasher.update((line_match.line as u64).to_be_bytes());
            hasher.update((line_match.start as u64).to_be_bytes());
            hasher.update((line_match.end_line as u64).to_be_bytes());
            hasher.update((line_match.end as u64).to_be_bytes());
            update_search_hash(
                &mut hasher,
                line_match.line_content.trim_end().as_bytes(),
            );
            remaining -= 1;
        }
    }
    format!("{:x}", hasher.finalize())
}

fn encode_file_search_cursor(
    offset: usize,
    search_revision: u64,
    query_fingerprint: &str,
    prefix_fingerprint: &str,
) -> String {
    format!("v1:{offset}:{search_revision}:{query_fingerprint}:{prefix_fingerprint}")
}

fn parse_file_search_cursor(value: &str) -> Result<AgentFileSearchCursor> {
    ensure!(
        value.len() <= MAX_AGENT_SEARCH_CURSOR_BYTES,
        "AHEAD file_search cursor is too long"
    );
    let mut parts = value.split(':');
    ensure!(
        parts.next() == Some("v1"),
        "AHEAD file_search cursor is invalid"
    );
    let offset = parts
        .next()
        .context("AHEAD file_search cursor is invalid")?
        .parse::<usize>()
        .context("AHEAD file_search cursor is invalid")?;
    ensure!(
        offset > 0 && offset <= MAX_AGENT_SEARCH_OFFSET,
        "AHEAD file_search cursor is invalid"
    );
    let search_revision = parts
        .next()
        .context("AHEAD file_search cursor is invalid")?
        .parse::<u64>()
        .context("AHEAD file_search cursor is invalid")?;
    let query_fingerprint = parts
        .next()
        .filter(|fingerprint| is_sha256_fingerprint(fingerprint))
        .context("AHEAD file_search cursor is invalid")?
        .to_string();
    let prefix_fingerprint = parts
        .next()
        .filter(|fingerprint| is_sha256_fingerprint(fingerprint))
        .context("AHEAD file_search cursor is invalid")?
        .to_string();
    ensure!(
        parts.next().is_none(),
        "AHEAD file_search cursor is invalid"
    );
    Ok(AgentFileSearchCursor {
        offset,
        search_revision,
        query_fingerprint,
        prefix_fingerprint,
    })
}

fn is_sha256_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_file_search_cursor(
    cursor: &AgentFileSearchCursor,
    search_revision: u64,
    query_fingerprint: &str,
    results: &[(PathBuf, Vec<ahead_core::search::FileSearchMatch>)],
) -> Result<()> {
    ensure!(
        cursor.query_fingerprint == query_fingerprint,
        "AHEAD file_search cursor belongs to a different query; restart without a cursor"
    );
    ensure!(
        cursor.search_revision == search_revision,
        "AHEAD file_search cursor is stale because workspace files changed; restart without a cursor"
    );
    ensure!(
        cursor.prefix_fingerprint
            == file_search_prefix_fingerprint(results, cursor.offset),
        "AHEAD file_search cursor is stale because earlier results changed; restart without a cursor"
    );
    Ok(())
}

fn validate_skill_handle(name: &str, value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= MAX_SKILL_HANDLE_BYTES
            && !value.chars().any(char::is_control),
        "skill {name} must be non-empty, contain no control characters, and be at most {MAX_SKILL_HANDLE_BYTES} bytes"
    );
    Ok(())
}

fn skill_resource_fingerprint(resource: &str, contents: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    (resource, contents).hash(&mut hasher);
    hasher.finish()
}

fn skill_resource_cursor(resource: &str, contents: &str, offset: usize) -> String {
    format!(
        "{:016x}:{offset}",
        skill_resource_fingerprint(resource, contents)
    )
}

fn skill_resource_cursor_offset(
    resource: &str,
    contents: &str,
    cursor: Option<&str>,
) -> Result<usize> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let Some((fingerprint, offset)) = cursor.split_once(':') else {
        bail!("skill resource cursor is invalid");
    };
    ensure!(
        u64::from_str_radix(fingerprint, 16).ok()
            == Some(skill_resource_fingerprint(resource, contents)),
        "skill resource cursor is stale; restart from the first page"
    );
    let offset = offset
        .parse::<usize>()
        .context("skill resource cursor is invalid")?;
    ensure!(
        offset <= contents.len() && contents.is_char_boundary(offset),
        "skill resource cursor is invalid"
    );
    Ok(offset)
}

fn page_skill_resource_result(
    resource: &str,
    contents: &str,
    cursor: Option<&str>,
) -> Result<String> {
    let start = skill_resource_cursor_offset(resource, contents, cursor)?;
    let response = |end, next_cursor| {
        json!({
            "resource": resource,
            "contents": &contents[start..end],
            "next_cursor": next_cursor,
        })
        .to_string()
    };
    let complete = response(contents.len(), None::<String>);
    if complete.len() <= MAX_SKILL_RESOURCE_PAGE_BYTES {
        return Ok(complete);
    }

    let mut lower = start;
    let mut upper = contents.len();
    let mut best = None;
    while lower < upper {
        let mut end = lower.midpoint(upper).saturating_add(1).min(contents.len());
        while !contents.is_char_boundary(end) {
            end += 1;
        }
        let candidate =
            response(end, Some(skill_resource_cursor(resource, contents, end)));
        if candidate.len() <= MAX_SKILL_RESOURCE_PAGE_BYTES {
            lower = end;
            best = Some(candidate);
        } else {
            upper = end.saturating_sub(1);
            while !contents.is_char_boundary(upper) {
                upper -= 1;
            }
        }
    }
    best.context("skill resource response budget leaves no room for contents")
}

fn append_bounded(output: &mut String, input: &str, max_bytes: usize) -> bool {
    let available = max_bytes.saturating_sub(output.len());
    if input.len() <= available {
        output.push_str(input);
        return false;
    }

    let mut end = available.min(input.len());
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&input[..end]);
    true
}

fn append_bounded_output_marker(output: &mut String, max_bytes: usize) {
    const MARKER: &str = "\n[Subagent output truncated]";
    if MARKER.len() > max_bytes {
        let mut end = max_bytes.min(output.len());
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        return;
    }

    let content_limit = max_bytes - MARKER.len();
    let mut end = content_limit.min(output.len());
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    output.truncate(end);
    output.push_str(MARKER);
}

fn slash_skill_invocation(text: &str) -> Option<(&str, &str)> {
    let command = text.trim_start().strip_prefix('/')?;
    let (name, request) = command
        .split_once(char::is_whitespace)
        .unwrap_or((command, ""));
    (!name.is_empty()).then_some((name, request))
}

fn agent_skill_source(scope: SkillScope) -> AgentSkillSource {
    match scope {
        SkillScope::User => AgentSkillSource::User,
        SkillScope::Repo => AgentSkillSource::Project,
        SkillScope::System => AgentSkillSource::System,
        SkillScope::Admin => AgentSkillSource::Admin,
    }
}

fn skill_matches_slash_invocation(
    skill_name: &str,
    source: AgentSkillSource,
    invocation: &str,
) -> bool {
    match invocation.rsplit_once(':') {
        Some((scope, name)) => skill_name == name && source.slash_prefix() == scope,
        None => skill_name == invocation,
    }
}

fn resolve_slash_skill<'a, T>(
    skills: impl Iterator<Item = (&'a str, AgentSkillSource, &'a T)>,
    invocation: &str,
) -> Result<&'a T> {
    let mut matches = skills.filter(|(name, source, _)| {
        skill_matches_slash_invocation(name, *source, invocation)
    });
    let matched_skill = matches.next().with_context(|| {
        format!("Skill /{invocation} is unavailable; choose a current skill from the / menu")
    })?;
    ensure!(
        matches.next().is_none(),
        "Skill /{invocation} is ambiguous; choose a source-qualified skill from the / menu"
    );
    Ok(matched_skill.2)
}

fn is_native_slash_command(name: &str) -> bool {
    matches!(
        name,
        "plan"
            | "context"
            | "checkpoint"
            | "compact"
            | "settings"
            | "review-project-memory"
            | "review-user-memory"
    )
}

fn open_buffer_overrides(
    workspace: &Path,
    allowed_paths: Option<&[String]>,
    snapshots: Vec<AgentBufferSnapshot>,
) -> (
    HashMap<PathBuf, String>,
    HashMap<PathBuf, Vec<AgentBufferSymbol>>,
) {
    let mut contents = HashMap::new();
    let mut symbols_by_path = HashMap::new();
    for snapshot in snapshots {
        let relative = Path::new(&snapshot.path);
        if ahead_core::search::is_private_file(relative)
            || allowed_paths.is_some_and(|allowed| {
                !path_is_allowed(&snapshot.path, allowed, workspace)
            })
        {
            continue;
        }
        let Some(path) =
            ahead_core::search::resolve_open_buffer_path(workspace, relative)
        else {
            continue;
        };
        contents.insert(path.clone(), snapshot.content);
        if !snapshot.symbols.is_empty() {
            symbols_by_path.insert(path, snapshot.symbols);
        } else {
            symbols_by_path.remove(&path);
        }
    }
    (contents, symbols_by_path)
}

fn format_file_search_results(
    workspace: &Path,
    results: Vec<(PathBuf, Vec<ahead_core::search::FileSearchMatch>)>,
    offset: usize,
    search_revision: u64,
    query_fingerprint: &str,
    symbol_context: &HashMap<PathBuf, Vec<AgentBufferSymbol>>,
) -> Result<String> {
    let mut output = String::new();
    let mut match_index = 0;
    let mut shown = 0;
    let mut has_more = false;
    for (path, matches) in &results {
        let mut wrote_path = false;
        for line_match in matches {
            if match_index < offset {
                match_index += 1;
                continue;
            }
            if shown == AGENT_SEARCH_PAGE_SIZE {
                has_more = true;
                break;
            }
            if !wrote_path {
                let relative = path.strip_prefix(workspace).context(
                    "AHEAD file_search result was outside the active workspace",
                )?;
                ensure!(
                    relative.components().all(|component| matches!(
                        component,
                        std::path::Component::Normal(_)
                    )),
                    "AHEAD file_search result was outside the active workspace"
                );
                output.push_str(&format!("\n{}", relative.display()));
                wrote_path = true;
            }
            let location = if line_match.end_line == line_match.line {
                format!("{}:{}", line_match.line + 1, line_match.start)
            } else {
                format!(
                    "{}:{}-{}:{}",
                    line_match.line + 1,
                    line_match.start,
                    line_match.end_line + 1,
                    line_match.end
                )
            };
            let mut symbols = u32::try_from(line_match.line)
                .ok()
                .and_then(|line| {
                    symbol_context.get(path).map(|symbols| {
                        symbols
                            .iter()
                            .filter(|symbol| {
                                symbol.start_line <= line && line < symbol.end_line
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default();
            symbols.sort_by_key(|symbol| {
                (
                    symbol.depth,
                    symbol.end_line.saturating_sub(symbol.start_line),
                )
            });
            symbols.dedup_by_key(|symbol| symbol.depth);
            let omitted_symbols = symbols
                .len()
                .saturating_sub(MAX_AGENT_SEARCH_SYMBOL_CONTEXT_DEPTH);
            symbols.drain(..omitted_symbols);
            if symbols.is_empty() {
                output.push_str(&format!(
                    "\n  {location} {}",
                    line_match.line_content.trim_end()
                ));
            } else {
                output.push_str(&format!(
                    "\n  {location} {}  (in ",
                    line_match.line_content.trim_end()
                ));
                if omitted_symbols > 0 {
                    output.push_str("… › ");
                }
                for (index, symbol) in symbols.iter().enumerate() {
                    if index > 0 {
                        output.push_str(" › ");
                    }
                    output.push_str(&symbol.name);
                }
                output.push(')');
            }
            match_index += 1;
            shown += 1;
        }
        if has_more {
            break;
        }
    }
    if shown == 0 {
        return Ok(if offset == 0 {
            "No matches found".to_string()
        } else {
            format!("No matches at offset {offset}")
        });
    }
    if has_more {
        let next_offset = offset + AGENT_SEARCH_PAGE_SIZE;
        if next_offset <= MAX_AGENT_SEARCH_OFFSET {
            let cursor = encode_file_search_cursor(
                next_offset,
                search_revision,
                query_fingerprint,
                &file_search_prefix_fingerprint(&results, next_offset),
            );
            output.push_str(&format!(
                "\nMore matches available; call file_search with cursor `{cursor}`."
            ));
        } else {
            output.push_str(
                "\nSearch result limit reached; refine the query to see later matches.",
            );
        }
    }
    Ok(output)
}

const AHEAD_DISABLED_FEATURES: &[Feature] = &[
    Feature::CodeMode,
    Feature::CodeModeInterrupt,
    Feature::CodeModeOnly,
    Feature::CodeModePrewarm,
    Feature::Collab,
    Feature::MemoryTool,
    Feature::MultiAgentV2,
    Feature::RespectSystemProxy,
    Feature::WebSearchCached,
    Feature::WebSearchRequest,
];

const AHEAD_MANAGED_DEVELOPER_INSTRUCTIONS: &str = concat!(
    "For concrete task target paths, inspect applicable `AGENTS.md` files from the active workspace root through each target's parent directory (or the target directory itself when it is a directory) before choosing additional project skills or taking side-effecting actions. ",
    "Read only those ancestor files, outer to inner, and apply the nearest file when instructions conflict. Do not recursively scan unrelated subtrees. If targets become clear only through exploration, keep that exploration read-only and check its instruction ancestry before editing. ",
    "For any requested project-file change, call the `apply_patch` tool so AHEAD can attribute it. Shell writes are blocked and cannot be attributed; do not try them. Use AHEAD file tools to inspect source, and reserve `exec_command` for requested builds, tests, and other terminal commands that do not edit project files. Do not use MCP filesystem tools to bypass patch attribution. ",
    "This model guidance does not replace AHEAD's host-enforced workspace and permission boundaries."
);

fn ahead_dynamic_tools(include_spawn_agent: bool) -> Vec<DynamicToolSpec> {
    let mut tools = vec![
        DynamicToolSpec::Function(DynamicToolFunctionSpec {
            name: AHEAD_FILE_SEARCH_TOOL.to_string(),
            description: "Search file contents and live unsaved editor buffers in the current AHEAD worktree with editor ignore and private-file rules. Result paths are worktree-relative and can be passed to read_editor_buffer. Use include_pattern and exclude_pattern for optional comma-separated path globs, for example include src/**/*.rs and exclude src/generated/**; do not put paths in pattern. Results are paginated with 20 matches per page; continue with the exact cursor returned by the previous page. If the cursor is stale, restart without a cursor."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "include_pattern": { "type": "string", "description": "Optional comma-separated globs for worktree-relative paths, e.g. src/**/*.rs or docs/**, or paths prefixed by the worktree root name." },
                    "exclude_pattern": { "type": "string", "description": "Optional comma-separated globs for worktree-relative paths to exclude, e.g. src/generated/**, target/**. Exclusions take precedence over include_pattern." },
                    "case_sensitive": { "type": "boolean", "default": false },
                    "whole_word": { "type": "boolean", "default": false },
                    "is_regex": { "type": "boolean", "default": false },
                    "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_AGENT_SEARCH_CURSOR_BYTES }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
            supports_parallel_tool_calls: true,
            defer_loading: false,
        }),
        DynamicToolSpec::Function(DynamicToolFunctionSpec {
            name: AHEAD_SKILL_RESOURCE_READ_TOOL.to_string(),
            description: "Read a host skill selected for the current turn. Use the opaque package handle from that skill's <resource_access>; omit resource for its SKILL.md, or pass <package>/<relative-path>. Skills not loaded into this turn, absolute paths, and paths outside the skill package are rejected. Continue with next_cursor when a page is returned.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": { "type": "string", "minLength": 1, "maxLength": MAX_SKILL_HANDLE_BYTES },
                    "resource": { "type": "string", "minLength": 1, "maxLength": MAX_SKILL_HANDLE_BYTES },
                    "cursor": { "type": "string", "maxLength": 32 }
                },
                "required": ["package"],
                "additionalProperties": false
            }),
            supports_parallel_tool_calls: true,
            defer_loading: false,
        }),
    ];
    if include_spawn_agent {
        tools.push(DynamicToolSpec::Function(DynamicToolFunctionSpec {
            name: AHEAD_SPAWN_AGENT_TOOL.to_string(),
            description: "Delegate a well-scoped task to a separate AHEAD-managed agent thread. The child does not inherit the conversation; include all relevant paths, requirements and constraints in message. Avoid trivial or overlapping delegation. Use session_id to continue a previous child and send only new instructions; its prior context is retained. AHEAD keeps the child within the same workspace, permission profile and editor context.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "label": { "type": "string", "minLength": 1, "maxLength": 80 },
                    "message": { "type": "string", "minLength": 1, "maxLength": 65536 },
                    "session_id": { "type": ["string", "null"] }
                },
                "required": ["label", "message"],
                "additionalProperties": false
            }),
            supports_parallel_tool_calls: true,
            defer_loading: false,
        }));
    }
    tools.extend(presentation_tool_definitions().into_iter().map(
        |(name, description, input_schema)| {
            DynamicToolSpec::Function(DynamicToolFunctionSpec {
                name,
                description,
                input_schema,
                supports_parallel_tool_calls: false,
                defer_loading: false,
            })
        },
    ));
    tools
}

fn file_changes(changes: HashMap<PathBuf, FileChange>) -> Vec<HarnessFileChange> {
    changes
        .into_iter()
        .map(|(path, change)| {
            let (path, diff) = match change {
                FileChange::Update {
                    unified_diff,
                    move_path,
                } => (move_path.unwrap_or(path), unified_diff),
                FileChange::Add { content } => {
                    let line_count = content.lines().count().max(1);
                    let added = content
                        .lines()
                        .map(|line| format!("+{line}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    (path, format!("@@ -0,0 +1,{line_count} @@\n{added}"))
                }
                FileChange::Delete { content } => {
                    let line_count = content.lines().count().max(1);
                    let deleted = content
                        .lines()
                        .map(|line| format!("-{line}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    (path, format!("@@ -1,{line_count} +0,0 @@\n{deleted}"))
                }
            };
            HarnessFileChange {
                path: path.to_string_lossy().into_owned(),
                diff,
            }
        })
        .collect()
}

fn bounded_patch_preview(
    call_id: String,
    changes: HashMap<PathBuf, FileChange>,
) -> HarnessPatchPreview {
    const MAX_FILES: usize = 8;
    const MAX_PATH_CHARS: usize = 256;
    const MAX_DIFF_CHARS_PER_FILE: usize = 2_000;

    fn take_chars(value: &str, limit: usize) -> (String, bool) {
        let mut chars = value.chars();
        let value = chars.by_ref().take(limit).collect();
        (value, chars.next().is_some())
    }

    fn add_content_diff(
        content: &str,
        prefix: &str,
        header: String,
        limit: usize,
    ) -> (String, bool) {
        let (mut diff, header_truncated) = take_chars(&header, limit);
        let mut remaining = limit.saturating_sub(diff.chars().count());
        let mut truncated = header_truncated;
        let mut lines = content.lines().peekable();
        while let Some(line) = lines.next() {
            let (bounded_prefix, prefix_truncated) = take_chars(prefix, remaining);
            remaining -= bounded_prefix.chars().count();
            diff.push_str(&bounded_prefix);
            truncated |= prefix_truncated;
            if truncated {
                break;
            }
            let (bounded_line, line_truncated) = take_chars(line, remaining);
            remaining -= bounded_line.chars().count();
            diff.push_str(&bounded_line);
            truncated |= line_truncated;
            if truncated {
                break;
            }
            if lines.peek().is_some() {
                if remaining == 0 {
                    truncated = true;
                    break;
                }
                diff.push('\n');
                remaining -= 1;
            }
        }
        truncated |= lines.peek().is_some();
        (diff, truncated)
    }

    fn bounded_diff(change: &FileChange, limit: usize) -> (String, bool) {
        match change {
            FileChange::Update { unified_diff, .. } => {
                take_chars(unified_diff, limit)
            }
            FileChange::Add { content } => add_content_diff(
                content,
                "+",
                format!("@@ -0,0 +1,{} @@\n", content.lines().count().max(1)),
                limit,
            ),
            FileChange::Delete { content } => add_content_diff(
                content,
                "-",
                format!("@@ -1,{} +0,0 @@\n", content.lines().count().max(1)),
                limit,
            ),
        }
    }

    let mut changes = changes.into_iter().collect::<Vec<_>>();
    changes.sort_by(|left, right| left.0.cmp(&right.0));
    let mut truncated = changes.len() > MAX_FILES;
    let changes = changes
        .into_iter()
        .take(MAX_FILES)
        .map(|(path, change)| {
            let path = match &change {
                FileChange::Update { move_path, .. } => {
                    move_path.as_ref().unwrap_or(&path)
                }
                FileChange::Add { .. } | FileChange::Delete { .. } => &path,
            };
            let (path, path_truncated) =
                take_chars(&path.to_string_lossy(), MAX_PATH_CHARS);
            let (diff, diff_truncated) =
                bounded_diff(&change, MAX_DIFF_CHARS_PER_FILE);
            truncated |= path_truncated || diff_truncated;
            HarnessFileChange { path, diff }
        })
        .collect();
    HarnessPatchPreview {
        call_id,
        changes,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_rollout_reader_accepts_plain_and_compressed_jsonl() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let item =
            RolloutItem::SessionMeta(codex_protocol::protocol::SessionMetaLine {
                meta: Default::default(),
                git: None,
            });
        let line = codex_rollout::RolloutLine {
            timestamp: "2025-01-03T12:00:00Z".to_string(),
            ordinal: None,
            item,
        };
        let content = format!("{}\n", serde_json::to_string(&line)?);
        let plain_path = temporary.path().join("rollout-plain.jsonl");
        std::fs::write(&plain_path, &content)?;
        assert!(matches!(
            read_legacy_rollout_items(&plain_path, File::open(&plain_path)?,)?
                .as_slice(),
            [RolloutItem::SessionMeta(_)]
        ));

        let compressed_path = temporary.path().join("rollout-compressed.jsonl.zst");
        let compressed =
            zstd::stream::encode_all(std::io::Cursor::new(content.as_bytes()), 1)?;
        std::fs::write(&compressed_path, compressed)?;
        assert!(matches!(
            read_legacy_rollout_items(
                &compressed_path,
                File::open(&compressed_path)?,
            )?
            .as_slice(),
            [RolloutItem::SessionMeta(_)]
        ));
        Ok(())
    }

    #[test]
    fn legacy_rollout_import_checks_workspace_and_keeps_turso_authoritative()
    -> Result<()> {
        let workspace = tempfile::tempdir()?;
        let other_workspace = tempfile::tempdir()?;
        let thread_id_text = "9fcb24bb-b6b1-4a4d-ae16-3a8f93e5d401";
        let thread_id = ThreadId::from_string(thread_id_text)?;
        let session_directory =
            workspace.path().join(".ahead/runtime/sessions/2025/01/03");
        std::fs::create_dir_all(&session_directory)?;
        let rollout_path =
            session_directory.join(format!("rollout-{thread_id_text}.jsonl"));
        let mut meta = codex_protocol::protocol::SessionMeta::default();
        meta.id = thread_id;
        meta.session_id = thread_id.into();
        meta.timestamp = "2025-01-03T12:00:00Z".to_string();
        meta.cwd = workspace.path().to_path_buf();
        meta.model_provider = Some("mock".to_string());
        let line = codex_rollout::RolloutLine {
            timestamp: meta.timestamp.clone(),
            ordinal: Some(0),
            item: RolloutItem::SessionMeta(
                codex_protocol::protocol::SessionMetaLine { meta, git: None },
            ),
        };
        let source = format!("{}\n", serde_json::to_string(&line)?);
        std::fs::write(&rollout_path, &source)?;

        let store = Arc::new(TestHarnessStore::default());
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(workspace.path().to_path_buf()),
            store.clone(),
            Arc::new(|_| {}),
        )?;
        assert!(
            client
                .import_legacy_thread_if_missing(
                    thread_id,
                    other_workspace.path(),
                    "mock",
                )
                .is_err()
        );
        assert!(store.load_native_thread(thread_id_text)?.is_none());
        assert_eq!(std::fs::read_to_string(&rollout_path)?, source);

        client.import_legacy_thread_if_missing(
            thread_id,
            workspace.path(),
            "mock",
        )?;
        let imported = store
            .load_native_thread(thread_id_text)?
            .context("legacy thread was not imported")?;
        assert_eq!(imported.create_params["thread_id"], thread_id_text);
        assert_eq!(imported.rollout_items.len(), 1);
        assert!(!imported.archived);
        assert_eq!(std::fs::read_to_string(&rollout_path)?, source);
        client.shutdown();
        Ok(())
    }

    #[test]
    fn legacy_rollout_lookup_requires_one_active_or_archived_candidate() -> Result<()>
    {
        let temporary = tempfile::tempdir()?;
        let thread_id = "9fcb24bb-b6b1-4a4d-ae16-3a8f93e5d401";
        let active = temporary
            .path()
            .join("sessions/2025/01/03")
            .join(format!("rollout-{thread_id}.jsonl"));
        std::fs::create_dir_all(active.parent().context("active parent")?)?;
        std::fs::write(&active, "")?;
        let candidates = legacy_rollout_candidates(temporary.path(), thread_id)?;
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], (active, false));

        let archived = temporary
            .path()
            .join("archived_sessions")
            .join(format!("rollout-{thread_id}.jsonl.zst"));
        std::fs::create_dir_all(archived.parent().context("archive parent")?)?;
        std::fs::write(&archived, "")?;
        assert_eq!(
            legacy_rollout_candidates(temporary.path(), thread_id)?.len(),
            2,
            "active/archive ambiguity must not select a source implicitly"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn legacy_rollout_lookup_rejects_a_symlinked_session_root() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir()?;
        let target = temporary.path().join("real-sessions");
        std::fs::create_dir(&target)?;
        symlink(&target, temporary.path().join("sessions"))?;
        assert!(
            legacy_rollout_candidates(
                temporary.path(),
                "9fcb24bb-b6b1-4a4d-ae16-3a8f93e5d401"
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn legacy_rollout_reader_strips_ghost_snapshots_without_shifting_metadata() {
        let mut response_item = json!({
            "type": "response_item",
            "payload": {"type": "ghost_snapshot"}
        });
        assert!(strip_legacy_ghost_snapshot_rollout_line(&mut response_item));

        let mut compacted = json!({
            "type": "compacted",
            "payload": {
                "replacement_history": [
                    {"type": "ghost_snapshot"},
                    {"type": "message"}
                ],
                "replacement_history_metadata": [
                    {"source": "ghost"},
                    {"source": "message"}
                ]
            }
        });
        assert!(!strip_legacy_ghost_snapshot_rollout_line(&mut compacted));
        assert_eq!(
            compacted["payload"]["replacement_history"],
            json!([{"type": "message"}])
        );
        assert_eq!(
            compacted["payload"]["replacement_history_metadata"],
            json!([{"source": "message"}])
        );
    }

    #[test]
    fn completed_turn_removes_only_its_own_pending_inputs() {
        let (own_sender, _) = oneshot::channel();
        let (other_sender, _) = oneshot::channel();
        let pending = Mutex::new(HashMap::from([(
            "session".to_string(),
            HashMap::from([
                (
                    "own".to_string(),
                    PendingUserInput {
                        sender: own_sender,
                        kind: PendingUserInputKind::AgentQuestion,
                    },
                ),
                (
                    "other".to_string(),
                    PendingUserInput {
                        sender: other_sender,
                        kind: PendingUserInputKind::AgentQuestion,
                    },
                ),
            ]),
        )]));
        let (event_sender, event_receiver) = std::sync::mpsc::channel();
        {
            let _cleanup = TurnPendingInputs {
                pending: &pending,
                session_id: "session",
                request_ids: vec!["own".to_string()],
                sink: Arc::new(move |event| {
                    event_sender.send(event).expect("record cancellation");
                }),
            };
        }
        let requests = pending.lock();
        assert!(!requests["session"].contains_key("own"));
        assert!(requests["session"].contains_key("other"));
        assert!(matches!(
            event_receiver.try_recv(),
            Ok(HarnessEvent::UserInputCancelled {
                acp_session_id,
                request_id,
            }) if acp_session_id == "session" && request_id == "own"
        ));
        assert!(event_receiver.try_recv().is_err());
    }

    #[test]
    fn managed_mcp_form_keeps_pending_request_after_invalid_answer() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(workspace.path().to_path_buf()),
            Arc::new(TestHarnessStore::default()),
            Arc::new(|_| {}),
        )
        .expect("start managed agent");
        let schema = json!({"type":"object","properties":{"count":{"type":"integer","minimum":1}},"required":["count"]});
        let (form, _) = McpForm::new("sample", "Count", &schema).expect("form");
        let (sender, receiver) = oneshot::channel();
        client
            .pending_inputs
            .lock()
            .entry("thread".to_string())
            .or_default()
            .insert(
                "mcp-form-test".to_string(),
                PendingUserInput {
                    sender,
                    kind: PendingUserInputKind::McpForm(form),
                },
            );
        let invalid =
            HashMap::from([("mcp_field_0".to_string(), vec!["0".to_string()])]);
        assert!(
            client
                .answer_user_input("thread", "mcp-form-test", invalid)
                .is_err()
        );
        assert!(
            client.pending_inputs.lock()["thread"].contains_key("mcp-form-test")
        );
        let valid =
            HashMap::from([("mcp_field_0".to_string(), vec!["2".to_string()])]);
        client
            .answer_user_input("thread", "mcp-form-test", valid.clone())
            .expect("valid answer");
        assert_eq!(client.runtime.block_on(receiver).expect("delivered"), valid);
        assert!(
            !client.pending_inputs.lock()["thread"].contains_key("mcp-form-test")
        );
    }

    #[test]
    fn managed_mcp_url_requires_explicit_https_consent() {
        let mut event = ElicitationRequestEvent {
            turn_id: None,
            server_name: "sample".to_string(),
            id: RequestId::String("url-request".to_string()),
            request: ElicitationRequest::Url {
                meta: None,
                message: "Connect your account".to_string(),
                url: "https://example.com/connect?state=abc".to_string(),
                elicitation_id: "flow-1".to_string(),
            },
        };
        let question =
            mcp_url_elicitation_question(&event).expect("safe URL question");
        assert_eq!(
            question.external_url.as_deref(),
            Some("https://example.com/connect?state=abc")
        );
        if let ElicitationRequest::Url { url, .. } = &mut event.request {
            *url = "http://example.com/connect".to_string();
        }
        assert!(mcp_url_elicitation_question(&event).is_none());

        let workspace = tempfile::tempdir().expect("workspace");
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(workspace.path().to_path_buf()),
            Arc::new(TestHarnessStore::default()),
            Arc::new(|_| {}),
        )
        .expect("start managed agent");
        let (sender, receiver) = oneshot::channel();
        client
            .pending_inputs
            .lock()
            .entry("thread".to_string())
            .or_default()
            .insert(
                "mcp-url-test".to_string(),
                PendingUserInput {
                    sender,
                    kind: PendingUserInputKind::McpUrl,
                },
            );
        assert!(
            client
                .answer_user_input("thread", "mcp-url-test", HashMap::new())
                .is_err()
        );
        assert!(client.pending_inputs.lock()["thread"].contains_key("mcp-url-test"));
        let accepted = HashMap::from([(
            ahead_rpc::ahead::MCP_FORM_ACTION_KEY.to_string(),
            vec!["accept".to_string()],
        )]);
        client
            .answer_user_input("thread", "mcp-url-test", accepted.clone())
            .expect("consent");
        assert_eq!(
            client.runtime.block_on(receiver).expect("delivered"),
            accepted
        );
        assert_eq!(
            mcp_url_action(&accepted).expect("action"),
            ElicitationAction::Accept
        );
        for (answer, action) in [
            ("decline", ElicitationAction::Decline),
            ("cancel", ElicitationAction::Cancel),
        ] {
            let answers = HashMap::from([(
                ahead_rpc::ahead::MCP_FORM_ACTION_KEY.to_string(),
                vec![answer.to_string()],
            )]);
            assert_eq!(mcp_url_action(&answers).expect("action"), action);
        }
        let mut extra = accepted;
        extra.insert("unrelated".to_string(), vec!["accept".to_string()]);
        assert!(mcp_url_action(&extra).is_err());
    }

    #[test]
    fn test_helper_selection_requires_built_ahead_and_preserves_app_path()
    -> Result<()> {
        let directory = tempfile::tempdir()?;
        let profile = directory.path().join("debug");
        std::fs::create_dir_all(profile.join("deps"))?;
        let test_executable = profile.join("deps/ahead_proxy-test");
        let error = test_sandbox_helper_executable(test_executable.clone())
            .expect_err("missing helper must not fall back to the test harness");
        assert!(
            error
                .to_string()
                .contains("cargo build -p ahead --bin ahead")
        );

        let app = profile.join(format!("ahead{}", std::env::consts::EXE_SUFFIX));
        std::fs::File::create(&app)?;
        assert_eq!(test_sandbox_helper_executable(test_executable)?, app);
        assert_eq!(test_sandbox_helper_executable(app.clone())?, app);
        Ok(())
    }

    #[test]
    fn mcp_approval_rejects_conflicting_answers() {
        let question = "mcp_tool_call_approval_call-1".to_string();
        assert!(
            validate_mcp_approval_answers(&HashMap::from([(
                question.clone(),
                vec!["Allow".to_string(), "Cancel".to_string()],
            )]))
            .is_err()
        );
        assert!(
            validate_mcp_approval_answers(&HashMap::from([(
                question,
                vec!["Cancel".to_string()],
            )]))
            .is_ok()
        );
    }

    #[test]
    fn managed_mcp_elicitation_only_surfaces_tool_approvals() {
        let mut event = ElicitationRequestEvent {
            turn_id: None,
            server_name: "echo".to_string(),
            id: RequestId::String("mcp_tool_call_approval_call-1".to_string()),
            request: ElicitationRequest::Form {
                meta: Some(json!({
                    APPROVAL_KIND_KEY: APPROVAL_KIND_MCP_TOOL_CALL,
                    PERSIST_KEY: PERSIST_SESSION,
                    TOOL_PARAMS_KEY: {"text": "example"}
                })),
                message: "Allow the echo MCP tool?".to_string(),
                requested_schema: json!({"type": "object", "properties": {}}),
            },
        };
        let (_, question) =
            mcp_approval_elicitation_question(&event).expect("tool approval");
        assert!(question.question.contains("\"text\":\"example\""));
        assert!(
            question
                .options
                .iter()
                .any(|option| option.label == "Allow for this session")
        );
        if let ElicitationRequest::Form { meta, .. } = &mut event.request {
            *meta = None;
        }
        assert!(mcp_approval_elicitation_question(&event).is_none());
        for mode in ["openai/form", "openaiForm"] {
            assert!(
                serde_json::from_value::<ElicitationRequest>(json!({
                    "mode": mode,
                    "message": "Unsupported proprietary form",
                    "requested_schema": {"type": "object", "properties": {}},
                }))
                .is_err(),
                "{mode} must not be accepted as an MCP form"
            );
        }
    }

    #[test]
    fn managed_mcp_item_preserves_standard_metadata_in_legacy_events() {
        use codex_protocol::items::McpToolCallError;
        use codex_protocol::items::McpToolCallItem;
        use codex_protocol::items::McpToolCallStatus;
        use codex_protocol::protocol::EventMsg;

        let item = McpToolCallItem {
            id: "call-1".to_string(),
            server: "calendar_server".to_string(),
            tool: "create_event".to_string(),
            arguments: serde_json::json!({"title": "Review"}),
            mcp_app_resource_uri: Some("ui://calendar/event".to_string()),
            read_only_hint: Some(false),
            status: McpToolCallStatus::Failed,
            result: None,
            error: Some(McpToolCallError {
                message: "approval denied".to_string(),
            }),
            duration: Some(std::time::Duration::from_millis(5)),
        };

        let EventMsg::McpToolCallBegin(begin) = item.as_legacy_begin_event() else {
            panic!("expected MCP begin event");
        };
        assert_eq!(begin.invocation.server, "calendar_server");
        assert_eq!(begin.invocation.tool, "create_event");
        assert_eq!(
            begin.invocation.arguments,
            Some(serde_json::json!({"title": "Review"}))
        );
        assert_eq!(
            begin.mcp_app_resource_uri.as_deref(),
            Some("ui://calendar/event")
        );
        assert_eq!(begin.read_only_hint, Some(false));

        let Some(EventMsg::McpToolCallEnd(end)) = item.as_legacy_end_event() else {
            panic!("expected MCP end event");
        };
        assert_eq!(end.call_id, "call-1");
        assert_eq!(
            end.mcp_app_resource_uri.as_deref(),
            Some("ui://calendar/event")
        );
        assert_eq!(end.read_only_hint, Some(false));
        assert_eq!(end.duration, std::time::Duration::from_millis(5));
        assert_eq!(
            end.result.expect_err("tool call should fail"),
            "approval denied"
        );
    }
    use ahead_agent_graph_store::{AgentGraphStore, ThreadSpawnEdgeStatus};
    use ahead_rpc::ahead::{
        AgentRuntimeState, AgentTurnRequestDto, CodeAnchor, ConversationMessage,
        TaskIntent,
    };
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicUsize;

    fn accept_model_post(
        listener: &TcpListener,
    ) -> (TcpStream, Vec<u8>, usize, usize) {
        loop {
            let (mut stream, _) = listener.accept().expect("accept mock model");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("bound mock model read");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 8192];
            let (header_end, content_length) = loop {
                let read = stream.read(&mut buffer).expect("read mock request");
                assert_ne!(read, 0, "mock request ended before its body");
                request.extend_from_slice(&buffer[..read]);
                let Some(header_end) = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| index + 4)
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                if request.len() >= header_end + content_length {
                    break (header_end, content_length);
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let request_line = headers.lines().next().unwrap_or_default();
            if request_line.starts_with("GET /v1/models?")
                || request_line.starts_with("GET /v1/models ")
            {
                let body = r#"{"models":[]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write mock model catalog");
                continue;
            }
            assert!(
                content_length > 0,
                "expected a model Responses POST: {headers}"
            );
            return (stream, request, header_end, content_length);
        }
    }

    #[test]
    fn subagent_output_limit_preserves_utf8_and_marks_truncation() {
        let mut output = String::new();
        assert!(append_bounded(&mut output, "猫abc", 5));
        assert_eq!(output, "猫ab");
        append_bounded_output_marker(&mut output, 32);
        assert!(output.ends_with("[Subagent output truncated]"));
        assert!(output.len() <= 32);
    }

    #[test]
    fn slash_skill_invocation_preserves_the_request_after_the_command() {
        assert_eq!(
            slash_skill_invocation("  /review-file inspect this\nthen summarize"),
            Some(("review-file", "inspect this\nthen summarize")),
        );
        assert_eq!(
            slash_skill_invocation("/review-file "),
            Some(("review-file", ""))
        );
        assert_eq!(slash_skill_invocation("/"), None);
        assert_eq!(slash_skill_invocation("ordinary prompt"), None);
    }

    #[test]
    fn slash_skill_source_qualifiers_select_the_matching_skill() {
        assert_eq!(agent_skill_source(SkillScope::User), AgentSkillSource::User);
        assert_eq!(
            agent_skill_source(SkillScope::Repo),
            AgentSkillSource::Project
        );
        assert_eq!(
            agent_skill_source(SkillScope::System),
            AgentSkillSource::System
        );
        assert_eq!(
            agent_skill_source(SkillScope::Admin),
            AgentSkillSource::Admin
        );
        assert!(skill_matches_slash_invocation(
            "review",
            AgentSkillSource::User,
            "user:review",
        ));
        assert!(!skill_matches_slash_invocation(
            "review",
            AgentSkillSource::User,
            ":review",
        ));
        assert!(skill_matches_slash_invocation(
            "review",
            AgentSkillSource::Project,
            "project:review",
        ));
        assert!(!skill_matches_slash_invocation(
            "review",
            AgentSkillSource::User,
            "project:review",
        ));
        assert!(skill_matches_slash_invocation(
            "review",
            AgentSkillSource::System,
            "review",
        ));
        assert!(!skill_matches_slash_invocation(
            "review",
            AgentSkillSource::Admin,
            "system:review",
        ));
    }

    #[test]
    fn slash_skill_selection_rejects_ambiguous_names_and_resolves_qualified_names() {
        let user = "user";
        let project = "project";
        let skills = [
            ("review", AgentSkillSource::User, &user),
            ("review", AgentSkillSource::Project, &project),
        ];

        assert!(
            resolve_slash_skill(skills.into_iter(), "review")
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert_eq!(
            resolve_slash_skill(skills.into_iter(), "user:review")
                .expect("user skill"),
            &user
        );
        assert_eq!(
            resolve_slash_skill(skills.into_iter(), "project:review")
                .expect("project skill"),
            &project
        );
        assert!(
            resolve_slash_skill(skills.into_iter(), "project:missing")
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
    }

    #[test]
    fn native_slash_actions_are_not_misrouted_as_skills() {
        assert!(is_native_slash_command("plan"));
        assert!(is_native_slash_command("compact"));
        assert!(!is_native_slash_command("review-file"));
    }

    #[test]
    fn subagent_activity_is_projected_into_its_parent_session() {
        let child_thread_id = ThreadId::new();
        let agent_path = codex_protocol::AgentPath::root()
            .join("worker")
            .expect("valid subagent path");

        for (kind, status) in [
            (SubAgentActivityKind::Started, "in_progress"),
            (SubAgentActivityKind::Interacted, "in_progress"),
            (SubAgentActivityKind::Interrupted, "failed"),
            (SubAgentActivityKind::Completed, "completed"),
        ] {
            assert_eq!(
                NativeClient::subagent_tool_call(
                    "parent-thread",
                    SubAgentActivityItem {
                        id: "activity-1".to_string(),
                        kind,
                        agent_thread_id: child_thread_id,
                        agent_path: agent_path.clone(),
                    },
                ),
                HarnessEvent::ToolCall {
                    acp_session_id: "parent-thread".to_string(),
                    call_id: format!("subagent-{child_thread_id}"),
                    title: "Subagent worker".to_string(),
                    status: status.to_string(),
                    kind: "subagent".to_string(),
                }
            );
        }
    }

    #[test]
    fn managed_turns_register_the_shared_editor_presentation_tools() {
        let shared_tools = presentation_tool_definitions();
        let managed_tools = ahead_dynamic_tools(true);
        let child_tools = ahead_dynamic_tools(false);
        for name in [
            AHEAD_READ_EDITOR_BUFFER_TOOL,
            AHEAD_PRESENT_CODE_TOOL,
            AHEAD_MOVE_CODE_POINTER_TOOL,
            AHEAD_CLEAR_PRESENTATION_TOOL,
            AHEAD_SPEAK_TEXT_TOOL,
            AHEAD_STOP_SPEAKING_TOOL,
        ] {
            assert!(shared_tools.iter().any(|(tool, _, _)| tool == name));
            assert!(managed_tools.iter().any(|tool| {
                matches!(
                    tool,
                    DynamicToolSpec::Function(spec) if spec.name == name
                )
            }));
        }
        assert!(managed_tools.iter().any(|tool| {
            matches!(tool, DynamicToolSpec::Function(spec) if spec.name == AHEAD_SPAWN_AGENT_TOOL)
        }));
        assert!(managed_tools.iter().any(|tool| {
            matches!(tool, DynamicToolSpec::Function(spec) if spec.name == AHEAD_SKILL_RESOURCE_READ_TOOL)
        }));
        let file_search = managed_tools
            .iter()
            .find_map(|tool| match tool {
                DynamicToolSpec::Function(spec)
                    if spec.name == AHEAD_FILE_SEARCH_TOOL =>
                {
                    Some(spec)
                }
                _ => None,
            })
            .expect("managed file_search tool");
        assert!(
            file_search.input_schema["properties"]
                .get("cursor")
                .is_some()
        );
        assert!(
            file_search.input_schema["properties"]
                .get("offset")
                .is_none()
        );
        assert!(
            file_search.input_schema["properties"]
                .get("exclude_pattern")
                .is_some()
        );
        assert!(!child_tools.iter().any(|tool| {
            matches!(tool, DynamicToolSpec::Function(spec) if spec.name == AHEAD_SPAWN_AGENT_TOOL)
        }));
    }

    #[test]
    fn spawn_agent_session_id_matches_zed_optional_input_behavior() {
        for session_id in [Value::Null, json!(""), json!(" \t")] {
            let input = serde_json::from_value::<SpawnAgentToolInput>(json!({
                "label": "Inspect",
                "message": "Check the implementation",
                "session_id": session_id,
            }))
            .expect("blank session IDs start a new child");
            assert_eq!(input.session_id, None);
        }

        let input = serde_json::from_value::<SpawnAgentToolInput>(json!({
            "label": "Inspect",
            "message": "Continue the investigation",
            "session_id": "child-session",
        }))
        .expect("valid session ID resumes a child");
        assert_eq!(input.session_id.as_deref(), Some("child-session"));
        assert!(
            serde_json::from_value::<SpawnAgentToolInput>(json!({
                "label": "Inspect",
                "message": "Continue the investigation",
                "session_id": 42,
            }))
            .is_err()
        );
    }

    #[test]
    fn spawn_agent_limits_match_schema_character_counts() {
        assert!(validate_spawn_agent_input(&"é".repeat(80), "Inspect").is_ok());
        assert!(validate_spawn_agent_input(&"é".repeat(81), "Inspect").is_err());
        assert!(validate_spawn_agent_input("Inspect", &"界".repeat(65_536)).is_ok());
        assert!(
            validate_spawn_agent_input("Inspect", &"界".repeat(65_537)).is_err()
        );
        assert!(validate_spawn_agent_input(" ", "Inspect").is_err());
        assert!(validate_spawn_agent_input("Inspect", " \t").is_err());
    }

    #[test]
    fn failed_spawn_agent_tool_response_keeps_child_session_id() {
        let response = subagent_tool_error("child-session", "turn cancelled");
        let response: Value = serde_json::from_str(&response.to_string())
            .expect("failed child result is valid JSON");
        assert_eq!(response["session_id"], "child-session");
        assert_eq!(response["error"], "turn cancelled");
    }

    #[test]
    fn skill_resource_pages_preserve_utf8_and_reject_stale_cursors() {
        let resource = "host-skill:project:sample:opaque";
        let contents = "猫".repeat(MAX_SKILL_RESOURCE_PAGE_BYTES);
        let first = page_skill_resource_result(resource, &contents, None)
            .expect("first resource page");
        assert!(first.len() <= MAX_SKILL_RESOURCE_PAGE_BYTES);
        let first: Value = serde_json::from_str(&first).expect("valid page JSON");
        let first_contents = first["contents"].as_str().expect("page contents");
        let cursor = first["next_cursor"].as_str().expect("continuation cursor");
        assert!(contents.starts_with(first_contents));
        assert!(first_contents.is_char_boundary(first_contents.len()));

        let mut combined = first_contents.to_string();
        let mut next_cursor = cursor.to_string();
        loop {
            let page =
                page_skill_resource_result(resource, &contents, Some(&next_cursor))
                    .expect("resource page");
            let page: Value = serde_json::from_str(&page).expect("valid page JSON");
            combined.push_str(page["contents"].as_str().expect("page contents"));
            match page["next_cursor"].as_str() {
                Some(cursor) => next_cursor = cursor.to_string(),
                None => break,
            }
        }
        assert_eq!(combined, contents);
        assert!(
            page_skill_resource_result(
                resource,
                &contents.replacen('猫', "犬", 1),
                Some(cursor)
            )
            .is_err()
        );
        let contents_with_multibyte = "a猫";
        let invalid_boundary_cursor =
            skill_resource_cursor(resource, contents_with_multibyte, 2);
        assert!(
            page_skill_resource_result(
                resource,
                contents_with_multibyte,
                Some(&invalid_boundary_cursor),
            )
            .is_err()
        );
    }

    #[test]
    fn move_code_pointer_requires_a_cue_and_exact_quote() {
        let action = parse_presentation_action(
            AHEAD_MOVE_CODE_POINTER_TOOL,
            json!({ "cue_id": "cue-1", "quote": "return value" }),
            Path::new("/workspace"),
            None,
        )
        .expect("parse pointer movement");
        assert_eq!(
            action,
            AgentPresentationAction::MovePointer {
                cue_id: "cue-1".to_string(),
                quote: "return value".to_string(),
            }
        );
        assert!(
            parse_presentation_action(
                AHEAD_MOVE_CODE_POINTER_TOOL,
                json!({ "cue_id": "cue-1", "quote": "" }),
                Path::new("/workspace"),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn native_agent_ignores_runtime_mcp_and_telemetry_with_path_preflight() {
        let temporary = tempfile::tempdir().expect("create runtime home");
        let runtime_home = temporary.path().join("runtime");
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir_all(&runtime_home)
            .expect("create runtime config directory");
        std::fs::create_dir_all(workspace.join(".ahead/memories"))
            .expect("create workspace memory directory");
        std::fs::write(
            workspace.join(".ahead/memories/MEMORY.md"),
            "project-memory-only-on-explicit-selection",
        )
        .expect("write project memory");
        std::fs::write(
            runtime_home.join("config.toml"),
            r#"
[mcp_servers.unmanaged]
command = "echo"
args = ["must not launch"]

[otel]
exporter = { otlp-http = { endpoint = "https://runtime.invalid/logs", protocol = "json" } }
trace_exporter = { otlp-http = { endpoint = "https://runtime.invalid/traces", protocol = "json" } }
metrics_exporter = { otlp-http = { endpoint = "https://runtime.invalid/metrics", protocol = "json" } }
log_user_prompt = true
"#,
        )
        .expect("write runtime-home MCP config");

        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        let retained_config = runtime
            .block_on(
                codex_core::config::ConfigBuilder::default()
                    .codex_home(runtime_home.clone())
                    .harness_overrides(codex_core::config::ConfigOverrides {
                        cwd: Some(workspace.clone()),
                        ..Default::default()
                    })
                    .build(),
            )
            .expect("load retained runtime config");
        assert_ne!(retained_config.otel.exporter, OtelExporterKind::None);
        assert_ne!(retained_config.otel.trace_exporter, OtelExporterKind::None);
        assert_ne!(
            retained_config.otel.metrics_exporter,
            OtelExporterKind::None
        );
        assert!(retained_config.otel.log_user_prompt);
        let config = runtime
            .block_on(NativeClient::build_config(
                &runtime_home,
                &workspace,
                None,
                None,
                McpServerPolicy::Disabled,
            ))
            .expect("build native agent config");

        assert!(config.mcp_servers.get().is_empty());
        assert_eq!(config.otel.exporter, OtelExporterKind::None);
        assert_eq!(config.otel.trace_exporter, OtelExporterKind::None);
        assert_eq!(config.otel.metrics_exporter, OtelExporterKind::None);
        assert!(!config.otel.log_user_prompt);
        let mcp_config =
            runtime.block_on(codex_core::McpManager::new().runtime_config(&config));
        assert_eq!(
            serde_json::to_value(mcp_config.client_elicitation_capability)
                .expect("serialize managed MCP elicitation capability"),
            json!({"form": {}, "url": {}}),
            "managed MCP advertises only the reviewed form and URL modes"
        );
        assert!(!config.features.enabled(Feature::Collab));
        assert!(!config.features.enabled(Feature::MultiAgentV2));
        assert!(config.features.enabled(Feature::Mcp20260728));
        assert!(config.features.enabled(Feature::ApplyPatchStreamingEvents));
        assert_eq!(
            config.mcp_protocol_mode(),
            codex_mcp::McpProtocolMode::V20260728
        );
        assert!(config.features.enabled(Feature::CodeModeHost));
        assert!(config.features.enabled(Feature::ToolCallMcpElicitation));
        let instructions =
            config.developer_instructions.as_deref().unwrap_or_default();
        assert!(instructions.contains(AHEAD_MANAGED_DEVELOPER_INSTRUCTIONS));
        assert!(instructions.contains(
            "For any requested project-file change, call the `apply_patch` tool so AHEAD can attribute it."
        ));
        assert!(
            instructions
                .contains("Shell writes are blocked and cannot be attributed")
        );
        assert!(!instructions.contains("AGENTS.override.md"));
        assert!(!instructions.contains("project-memory-only-on-explicit-selection"));

        let servers =
            runtime.block_on(codex_core::McpManager::new().runtime_servers(&config));
        assert!(
            servers.is_empty(),
            "managed sessions must not gain undeclared MCP servers"
        );
    }

    #[test]
    fn retired_on_failure_approval_policy_is_not_accepted() {
        assert!(
            toml::from_str::<codex_config::config_toml::ConfigToml>(
                "approval_policy = \"on-failure\""
            )
            .is_err(),
            "retired approval policy must not deserialize"
        );
    }

    #[test]
    fn managed_provider_uses_ahead_settings_without_runtime_config_file() {
        let temporary = tempfile::tempdir().expect("disposable workspace");
        let runtime_home = temporary.path().join("runtime");
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&runtime_home).expect("runtime home");
        std::fs::create_dir(&workspace).expect("workspace");
        std::fs::create_dir(workspace.join(".ahead"))
            .expect("AHEAD settings directory");
        std::fs::write(runtime_home.join("config.toml"), "invalid = [")
            .expect("stale runtime config");
        std::fs::write(
            workspace.join(".ahead/settings.toml"),
            "[ai]\nactive_connection = 'Local'\n[[ai.connections]]\nname = 'Local'\nprovider_id = 'local'\nbase_url = 'https://local.example/v1'\nmodel = 'test-model'\napi_key = 'private-token'\n",
        )
        .expect("private provider settings");

        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        let config = runtime
            .block_on(NativeClient::build_config(
                &runtime_home,
                &workspace,
                None,
                None,
                McpServerPolicy::Disabled,
            ))
            .expect("load AHEAD provider without runtime config file");
        assert_eq!(config.model.as_deref(), Some("test-model"));
        assert_eq!(config.model_provider_id, "local");
        assert_eq!(
            config.model_provider.base_url.as_deref(),
            Some("https://local.example/v1")
        );
        assert!(config.model_provider.experimental_bearer_token.is_some());
        let effective = config.config_layer_stack.effective_config();
        for retired_key in [
            "history",
            "file_opener",
            "cli_auth_credentials_store",
            "chatgpt_base_url",
        ] {
            assert!(
                effective.get(retired_key).is_none(),
                "AHEAD must not inherit packaged Codex setting {retired_key}"
            );
        }
        assert!(config.config_layer_stack.ignore_user_exec_policy_rules());
    }

    #[test]
    fn managed_model_manager_config_uses_the_selected_connection() {
        let temporary = tempfile::tempdir().expect("disposable workspace");
        let runtime_home = temporary.path().join("runtime");
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir_all(&runtime_home).expect("runtime home");
        std::fs::create_dir_all(workspace.join(".ahead"))
            .expect("workspace settings directory");
        std::fs::write(
            workspace.join(".ahead/settings.toml"),
            "[ai]\nactive_connection = 'First'\n[[ai.connections]]\nname = 'First'\nprovider_id = 'first'\nbase_url = 'https://first.example/v1'\nmodel = 'first-model'\n[[ai.connections]]\nname = 'Second'\nprovider_id = 'second'\nbase_url = 'https://second.example/v1'\nmodel = 'second-model'\n",
        )
        .expect("write provider settings");

        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        let default_config = runtime
            .block_on(NativeClient::build_config(
                &runtime_home,
                &workspace,
                None,
                None,
                McpServerPolicy::Disabled,
            ))
            .expect("build active provider config");
        let selected_config = runtime
            .block_on(NativeClient::build_config(
                &runtime_home,
                &workspace,
                Some("second-model"),
                Some("second"),
                McpServerPolicy::Disabled,
            ))
            .expect("build selected provider config");

        assert_eq!(default_config.model_provider_id, "first");
        assert_eq!(selected_config.model_provider_id, "second");
        assert_eq!(
            selected_config.model_provider.base_url.as_deref(),
            Some("https://second.example/v1")
        );
        assert_ne!(
            provider_fingerprint(&default_config)
                .expect("default provider fingerprint"),
            provider_fingerprint(&selected_config)
                .expect("selected provider fingerprint")
        );
    }

    #[test]
    fn managed_turn_rejects_changed_connection_before_network() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("private settings directory");
        let settings_path = ahead.join("settings.toml");
        let settings = |endpoint: &str, key: &str| {
            format!(
                "[ai]\nactive_connection = 'Mock'\n[[ai.connections]]\nname = 'Mock'\nprovider_id = 'mock'\nbase_url = '{endpoint}'\nmodel = 'gpt-5.5'\napi_key = '{key}'\n"
            )
        };
        std::fs::write(
            &settings_path,
            settings("http://127.0.0.1:49111/v1", "first-key"),
        )
        .expect("initial private connection");
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(workspace.path().to_path_buf()),
            Arc::new(TestHarnessStore::default()),
            Arc::new(|_| {}),
        )
        .expect("start managed agent");
        let thread_id = client
            .new_session(
                workspace.path(),
                "read-only",
                Some("gpt-5.5"),
                Some("mock"),
            )
            .expect("create managed thread");

        for changed in [
            settings("http://127.0.0.1:49112/v1", "first-key"),
            settings("http://127.0.0.1:49111/v1", "rotated-key"),
        ] {
            std::fs::write(&settings_path, changed).expect("edit connection");
            let error = client
                .prompt(&thread_id, "Do not send this to the old connection")
                .expect_err("changed connection must fail before a model call");
            assert!(
                error
                    .to_string()
                    .contains("retry after other managed turns finish"),
                "{error}"
            );
        }
        client.shutdown();
    }

    #[test]
    fn workspace_config_loads_only_opted_in_mcp_with_human_review() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let runtime_home = temporary.path().join("runtime");
        let workspace = temporary.path().join("workspace");
        let ahead = workspace.join(".ahead");
        std::fs::create_dir_all(&runtime_home).expect("runtime home");
        std::fs::create_dir_all(&ahead).expect("workspace AHEAD config");
        std::fs::write(
            runtime_home.join("config.toml"),
            "[mcp_servers.inherited]\ncommand = \"echo\"\n",
        )
        .expect("write inherited runtime MCP server");
        let declaration = "[mcp.servers.docs]\ncommand = \"echo\"\nargs = [\"docs\"]\ndefault_tools_approval_mode = \"approve\"\n";
        std::fs::write(ahead.join("config.toml"), declaration)
            .expect("write project MCP declaration");
        let parsed = declaration
            .parse::<toml::Table>()
            .expect("parse declaration");
        let fingerprint = crate::runtime_support::mcp_declaration_fingerprint(
            &parsed["mcp"]["servers"]["docs"],
        )
        .expect("fingerprint declaration");
        std::fs::write(
            ahead.join("settings.toml"),
            format!("[mcp]\nenabled_servers = [\"docs\"]\n[mcp.approved_declarations]\ndocs = \"{fingerprint}\"\n"),
        )
        .expect("write local MCP opt-in");

        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        let config = runtime
            .block_on(NativeClient::build_config(
                &runtime_home,
                &workspace,
                None,
                None,
                McpServerPolicy::PromptEveryCall,
            ))
            .expect("build configured managed runtime");

        let servers = config.mcp_servers.get();
        assert_eq!(servers.len(), 1);
        assert!(servers.contains_key("docs"));
        assert!(!servers.contains_key("inherited"));
        assert_eq!(
            servers["docs"].default_tools_approval_mode,
            Some(codex_config::AppToolApproval::Prompt)
        );

        let servers =
            runtime.block_on(codex_core::McpManager::new().runtime_servers(&config));
        assert_eq!(servers, *config.mcp_servers.get());
    }

    #[test]
    fn managed_parallel_stdio_mcp_calls_wait_for_individual_approval() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir_all(&ahead).expect("create private settings directory");
        let script = ahead.join("mcp_echo.py");
        let trace = ahead.join("mcp_calls.jsonl");
        std::fs::write(
            &script,
            include_str!("../../tests/fixtures/editor-smoke/mcp_echo.py"),
        )
        .expect("copy offline MCP server into disposable workspace");
        let declaration = format!(
            "[mcp.servers.echo]\ncommand = \"python3\"\nargs = [\"-u\", {}, {}]\n",
            toml::Value::String(script.to_string_lossy().into_owned()),
            toml::Value::String(trace.to_string_lossy().into_owned()),
        );
        std::fs::write(ahead.join("config.toml"), &declaration)
            .expect("declare local MCP server");
        let parsed = declaration.parse::<toml::Table>().expect("MCP declaration");
        let fingerprint = crate::runtime_support::mcp_declaration_fingerprint(
            &parsed["mcp"]["servers"]["echo"],
        )
        .expect("fingerprint MCP declaration");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock model");
        let address = listener.local_addr().expect("mock model address");
        std::fs::write(
            ahead.join("settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"gpt-5.5\"\n[mcp]\nenabled_servers = [\"echo\"]\n[mcp.approved_declarations]\necho = \"{fingerprint}\"\n"
            ),
        )
        .expect("enable reviewed MCP server and local model");
        let captured_requests = Arc::new(Mutex::new(Vec::new()));
        let model_requests = captured_requests.clone();
        let model_server = std::thread::spawn(move || {
            for turn in 0..10 {
                let (mut stream, request, header_end, content_length) =
                    accept_model_post(&listener);
                model_requests.lock().push(
                    serde_json::from_slice::<Value>(
                        &request[header_end..header_end + content_length],
                    )
                    .expect("model request JSON"),
                );
                let events = match turn {
                    0 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"mcp-search","type":"tool_search_call","call_id":"mcp-search","execution":"client","arguments":{"query":"echo","limit":1}}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"mcp-search","type":"tool_search_call","call_id":"mcp-search","execution":"client","arguments":{"query":"echo","limit":1}}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-1","end_turn":false}}),
                    ],
                    1 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"mcp-call-1","type":"function_call","call_id":"mcp-call-1","namespace":"mcp__echo","name":"echo","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"mcp-call-1","type":"function_call","call_id":"mcp-call-1","namespace":"mcp__echo","name":"echo","arguments":"{\"text\":\"MCP_ECHO_SENTINEL_1\",\"payload\":{\"depth\":1}}"}}),
                        json!({"type":"response.output_item.added","output_index":1,"item":{"id":"mcp-call-2","type":"function_call","call_id":"mcp-call-2","namespace":"mcp__echo","name":"echo","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":1,"item":{"id":"mcp-call-2","type":"function_call","call_id":"mcp-call-2","namespace":"mcp__echo","name":"echo","arguments":"{\"text\":\"MCP_ECHO_SENTINEL_2\",\"payload\":{\"depth\":2}}"}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-2","end_turn":false}}),
                    ],
                    2 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"resource-list-denied","type":"function_call","call_id":"resource-list-denied","name":"list_mcp_resources","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"resource-list-denied","type":"function_call","call_id":"resource-list-denied","name":"list_mcp_resources","arguments":"{}"}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-3","end_turn":false}}),
                    ],
                    3 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"resource-list","type":"function_call","call_id":"resource-list","name":"list_mcp_resources","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"resource-list","type":"function_call","call_id":"resource-list","name":"list_mcp_resources","arguments":"{}"}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-4","end_turn":false}}),
                    ],
                    4 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"resource-templates","type":"function_call","call_id":"resource-templates","name":"list_mcp_resource_templates","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"resource-templates","type":"function_call","call_id":"resource-templates","name":"list_mcp_resource_templates","arguments":"{}"}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-5","end_turn":false}}),
                    ],
                    5 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"resource-read","type":"function_call","call_id":"resource-read","name":"read_mcp_resource","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"resource-read","type":"function_call","call_id":"resource-read","name":"read_mcp_resource","arguments":"{\"server\":\"echo\",\"uri\":\"memo://ahead-smoke\"}"}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-6","end_turn":false}}),
                    ],
                    7 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"mcp-auto-search","type":"tool_search_call","call_id":"mcp-auto-search","execution":"client","arguments":{"query":"echo","limit":1}}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"mcp-auto-search","type":"tool_search_call","call_id":"mcp-auto-search","execution":"client","arguments":{"query":"echo","limit":1}}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-8","end_turn":false}}),
                    ],
                    8 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"mcp-auto-call","type":"function_call","call_id":"mcp-auto-call","namespace":"mcp__echo","name":"echo","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"mcp-auto-call","type":"function_call","call_id":"mcp-auto-call","namespace":"mcp__echo","name":"echo","arguments":"{\"text\":\"MCP_AUTO_SENTINEL\"}"}}),
                        json!({"type":"response.output_item.added","output_index":1,"item":{"id":"mcp-auto-resource","type":"function_call","call_id":"mcp-auto-resource","name":"list_mcp_resources","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":1,"item":{"id":"mcp-auto-resource","type":"function_call","call_id":"mcp-auto-resource","name":"list_mcp_resources","arguments":"{}"}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-9","end_turn":false}}),
                    ],
                    9 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"mcp-auto-message","type":"message","role":"assistant","content":[]}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"mcp-auto-message","type":"message","role":"assistant","content":[{"type":"output_text","text":"MCP auto-approved"}]}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-10","end_turn":true}}),
                    ],
                    _ => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"mcp-message","type":"message","role":"assistant","content":[]}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"mcp-message","type":"message","role":"assistant","content":[{"type":"output_text","text":"MCP finished"}]}}),
                        json!({"type":"response.completed","response":{"id":"mcp-response-7","end_turn":true}}),
                    ],
                };
                let body = events
                    .into_iter()
                    .map(|event| {
                        format!(
                            "event: {}\ndata: {event}\n\n",
                            event["type"].as_str().expect("event type")
                        )
                    })
                    .collect::<String>();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write model response");
            }
        });

        let (approval_sender, approval_receiver) = std::sync::mpsc::channel();
        let store = Arc::new(TestHarnessStore::default());
        let client = Arc::new(
            NativeClient::spawn(
                &NativeClientConfig::ahead(workspace.path().to_path_buf()),
                store,
                Arc::new(move |event| {
                    if let HarnessEvent::UserInputRequested { .. } = event {
                        approval_sender.send(event).expect("send approval request");
                    }
                }),
            )
            .expect("start managed agent"),
        );
        let thread_id = client
            .new_session(workspace.path(), "agent", Some("gpt-5.5"), Some("mock"))
            .expect("create managed session");
        let configured_servers = client.runtime.block_on(async {
            let thread = client.threads.lock()[&thread_id].clone();
            thread.config().await.mcp_servers.get().clone()
        });
        assert!(configured_servers.contains_key("echo"));
        let (prompt_sender, prompt_receiver) = std::sync::mpsc::channel();
        let prompt_client = client.clone();
        let prompt_thread_id = thread_id.clone();
        std::thread::spawn(move || {
            drop(prompt_sender.send(prompt_client.prompt(
                &prompt_thread_id,
                "Call echo twice with MCP_ECHO_SENTINEL_1 and MCP_ECHO_SENTINEL_2, then list resources and templates and read the smoke resource",
            )));
        });
        let mut approvals = Vec::new();
        for _ in 0..2 {
            let HarnessEvent::UserInputRequested {
                request_id,
                questions,
                ..
            } = approval_receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("each MCP call asks for approval")
            else {
                panic!("expected MCP approval request");
            };
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].header, "Approve MCP tool?");
            assert!(
                questions[0].question.starts_with(
                    "Allow the echo MCP server to run tool \"echo\"?\nArguments: "
                ),
                "unexpected MCP approval question: {:?}",
                questions[0].question
            );
            assert!(
                questions[0]
                    .options
                    .iter()
                    .any(|option| option.label == "Allow")
            );
            let unapproved_sentinel =
                if questions[0].id == "mcp_tool_call_approval_mcp-call-1" {
                    "MCP_ECHO_SENTINEL_1"
                } else {
                    assert_eq!(questions[0].id, "mcp_tool_call_approval_mcp-call-2");
                    "MCP_ECHO_SENTINEL_2"
                };
            assert!(questions[0].question.contains(unapproved_sentinel));
            assert!(
                !std::fs::read_to_string(&trace)
                    .unwrap_or_default()
                    .contains(unapproved_sentinel),
                "MCP call must not run before its own approval"
            );
            approvals.push((request_id, questions[0].id.clone()));
        }
        approvals.sort_by(|left, right| left.1.cmp(&right.1));
        assert_ne!(approvals[0].0, approvals[1].0);
        assert_eq!(
            approvals
                .iter()
                .map(|(_, question_id)| question_id.as_str())
                .collect::<Vec<_>>(),
            [
                "mcp_tool_call_approval_mcp-call-1",
                "mcp_tool_call_approval_mcp-call-2"
            ]
        );
        for (request_id, question_id) in approvals {
            client
                .answer_user_input(
                    &thread_id,
                    &request_id,
                    HashMap::from([(question_id, vec!["Allow".to_string()])]),
                )
                .expect("approve MCP call");
        }
        for (call_id, operation, answer) in [
            ("resource-list-denied", "list_mcp_resources", "Cancel"),
            ("resource-list", "list_mcp_resources", "Allow"),
            ("resource-templates", "list_mcp_resource_templates", "Allow"),
            ("resource-read", "read_mcp_resource", "Allow"),
        ] {
            let HarnessEvent::UserInputRequested {
                request_id,
                questions,
                ..
            } = approval_receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("each resource operation asks for approval")
            else {
                panic!("expected MCP resource approval request");
            };
            assert_eq!(
                questions[0].id,
                format!("mcp_tool_call_approval_{call_id}:echo")
            );
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].header, "Approve MCP tool?");
            assert!(questions[0].question.starts_with(&format!(
                "Allow the echo MCP server to run tool \"{operation}\"?"
            )));
            let method = match operation {
                "list_mcp_resources" => "resources/list",
                "list_mcp_resource_templates" => "resources/templates/list",
                "read_mcp_resource" => "resources/read",
                _ => panic!("unexpected resource operation"),
            };
            assert!(
                !std::fs::read_to_string(&trace)
                    .unwrap_or_default()
                    .contains(&format!("\"method\": \"{method}\"")),
                "MCP resource operation must not run before approval"
            );
            client
                .answer_user_input(
                    &thread_id,
                    &request_id,
                    HashMap::from([(
                        questions[0].id.clone(),
                        vec![answer.to_string()],
                    )]),
                )
                .expect("answer MCP resource approval");
        }
        let result = prompt_receiver.recv_timeout(Duration::from_secs(10));
        if result.is_err() {
            client.cancel(&thread_id).expect("cancel timed out turn");
            panic!(
                "managed MCP turn timed out after approval; model_requests={}; trace_exists={}",
                captured_requests.lock().len(),
                trace.exists(),
            );
        }
        result
            .expect("managed turn timed out")
            .expect("complete managed turn");
        {
            let requests = captured_requests.lock();
            assert!(requests[1].to_string().contains("mcp__echo"));
            assert!(requests[2].to_string().contains("MCP_ECHO_SENTINEL_1"));
            assert!(requests[2].to_string().contains("MCP_ECHO_SENTINEL_2"));
            assert!(
                requests[3]
                    .to_string()
                    .contains("user cancelled MCP resource operation")
            );
            assert!(requests[4].to_string().contains("memo://ahead-smoke"));
            assert!(requests[5].to_string().contains("memo://{id}"));
            assert!(requests[6].to_string().contains("MCP_RESOURCE_SENTINEL"));
        }
        let mut calls = std::fs::read_to_string(&trace)
            .expect("MCP call trace")
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("MCP arguments JSON")
            })
            .collect::<Vec<_>>();
        let mut request_ids = calls
            .iter()
            .map(|call| {
                call["request_id"]
                    .as_u64()
                    .expect("MCP trace records the JSON-RPC request id")
            })
            .collect::<Vec<_>>();
        let recorded_calls = request_ids.len();
        request_ids.sort_unstable();
        request_ids.dedup();
        assert_eq!(
            request_ids.len(),
            recorded_calls,
            "each MCP request keeps its own JSON-RPC id"
        );
        for call in &mut calls {
            call.as_object_mut()
                .expect("MCP trace entry is an object")
                .remove("request_id");
        }
        calls.sort_by_key(|value| {
            value["params"]["arguments"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        });
        assert_eq!(
            calls,
            vec![
                json!({"method":"resources/list","params":{}}),
                json!({"method":"resources/templates/list","params":{}}),
                json!({"method":"resources/read","params":{"uri":"memo://ahead-smoke"}}),
                json!({"method":"tools/call","params":{"name":"echo","arguments":{"text":"MCP_ECHO_SENTINEL_1","payload":{"depth":1}}}}),
                json!({"method":"tools/call","params":{"name":"echo","arguments":{"text":"MCP_ECHO_SENTINEL_2","payload":{"depth":2}}}}),
            ]
        );
        crate::runtime_support::set_mcp_server_approval(
            workspace.path(),
            "echo",
            &fingerprint,
            ahead_rpc::ahead::McpServerApprovalAction::AutoApproveAll,
        )
        .expect("auto-approve reviewed server");
        let auto_thread_id = client
            .new_session(workspace.path(), "agent", Some("gpt-5.5"), Some("mock"))
            .expect("create auto-approved managed session");
        let (auto_sender, auto_receiver) = std::sync::mpsc::channel();
        let auto_client = client.clone();
        let auto_prompt_thread_id = auto_thread_id.clone();
        std::thread::spawn(move || {
            drop(auto_sender.send(auto_client.prompt(
                &auto_prompt_thread_id,
                "Call echo with MCP_AUTO_SENTINEL and list resources",
            )));
        });
        let auto_result = auto_receiver.recv_timeout(Duration::from_secs(10));
        if auto_result.is_err() {
            client
                .cancel(&auto_thread_id)
                .expect("cancel timed out auto-approved turn");
            panic!("auto-approved MCP turn timed out");
        }
        auto_result
            .expect("auto-approved turn timed out")
            .expect("complete auto-approved turn");
        assert!(
            approval_receiver.try_recv().is_err(),
            "auto-approved tool and resource calls must not request per-call review"
        );
        model_server.join().expect("join local model server");
        let requests = captured_requests.lock();
        assert!(requests[9].to_string().contains("MCP_AUTO_SENTINEL"));
        assert!(requests[9].to_string().contains("memo://ahead-smoke"));
        let auto_trace =
            std::fs::read_to_string(&trace).expect("auto-approved MCP trace");
        assert!(auto_trace.contains("MCP_AUTO_SENTINEL"));
        assert_eq!(
            auto_trace.matches("\"method\": \"resources/list\"").count(),
            2
        );
        client.shutdown();
    }

    #[test]
    fn native_mcp_projection_preserves_stdio_environment() {
        let temporary = tempfile::tempdir().expect("runtime home and workspace");
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("create runtime");
        let mut config = runtime
            .block_on(NativeClient::build_config(
                temporary.path(),
                temporary.path(),
                None,
                None,
                McpServerPolicy::Disabled,
            ))
            .expect("build managed config");
        let manager = codex_core::McpManager::new();

        let declaration = json!({
            "command": "ahead-test-mcp",
            "args": ["--stdio"],
            "env": { "AHEAD_TEST_MCP_TOKEN": "test-only" },
            "env_vars": ["AHEAD_TEST_MCP_AUTH"],
        });
        let server: codex_config::McpServerConfig =
            serde_json::from_value(declaration.clone())
                .expect("parse stdio MCP config");
        config
            .mcp_servers
            .set(HashMap::from([("docs".to_string(), server.clone())]))
            .expect("set runtime test server");
        let effective = runtime.block_on(manager.effective_servers(&config));
        assert_eq!(effective["docs"].config(), &server);
        let serialized = serde_json::to_value(effective["docs"].config())
            .expect("serialize server configuration");
        for (name, value) in declaration.as_object().expect("declaration object") {
            assert_eq!(&serialized[name], value);
        }
    }

    #[test]
    fn only_assist_sessions_can_load_mcp_servers() {
        assert_eq!(
            mcp_server_policy("agent", false),
            McpServerPolicy::PromptEveryCall
        );
        assert_eq!(mcp_server_policy("agent", true), McpServerPolicy::Disabled);
        assert_eq!(
            mcp_server_policy("read-only", false),
            McpServerPolicy::Disabled
        );
    }

    #[test]
    fn editor_overrides_keep_workspace_and_scope_boundaries() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir_all(workspace.join("src"))
            .expect("create source directory");
        std::fs::create_dir_all(workspace.join("other"))
            .expect("create other directory");
        let snapshots = [
            ("src/live.rs", "unsaved"),
            ("src/server.pem", "secret"),
            ("other/file.rs", "outside scope"),
            ("../escape.rs", "outside workspace"),
        ]
        .into_iter()
        .map(|(path, content)| AgentBufferSnapshot {
            path: path.to_string(),
            content: content.to_string(),
            symbols: Vec::new(),
        })
        .collect();
        let (overrides, symbols) =
            open_buffer_overrides(&workspace, Some(&["src".to_string()]), snapshots);
        assert_eq!(overrides.len(), 1);
        assert!(symbols.is_empty());
        assert_eq!(
            overrides.get(&workspace.join("src/live.rs")),
            Some(&"unsaved".to_string())
        );
        let matches = ahead_core::search::search_paths_with_overrides(
            ahead_core::search::SearchScope::Workspace(&workspace),
            std::iter::once(workspace.join("src/live.rs")),
            &overrides,
            &ahead_core::search::FileSearchOptions {
                pattern: "unsaved".into(),
                case_sensitive: true,
                whole_word: false,
                is_regex: false,
                max_results: 20,
            },
            || true,
        )
        .expect("search unsaved editor text");
        assert_eq!(matches.len(), 1);

        std::fs::create_dir_all(workspace.join(".ahead"))
            .expect("create private settings directory");
        let (private_overrides, private_symbols) = open_buffer_overrides(
            &workspace,
            None,
            vec![AgentBufferSnapshot {
                path: ".ahead/settings.toml".to_string(),
                content: "api_key = 'secret'".to_string(),
                symbols: vec![AgentBufferSymbol {
                    name: "private_setting".into(),
                    start_line: 0,
                    end_line: 1,
                    depth: 0,
                }],
            }],
        );
        assert!(private_overrides.is_empty());
        assert!(private_symbols.is_empty());
    }

    #[test]
    fn buffer_snapshot_and_file_search_requests_round_trip_through_native_client() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let (event_sender, event_receiver) = std::sync::mpsc::channel();
        let client = Arc::new(
            NativeClient::spawn(
                &NativeClientConfig::ahead(temporary.path().to_path_buf()),
                Arc::new(TestHarnessStore::default()),
                Arc::new(move |event| {
                    event_sender.send(event).expect("send harness event");
                }),
            )
            .expect("start native client"),
        );
        let requester = client.clone();
        let worker = std::thread::spawn(move || {
            requester
                .runtime
                .block_on(requester.current_buffer_snapshots("thread", false))
        });
        let event = event_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("receive buffer snapshot request");
        let HarnessEvent::BufferSnapshotsRequested {
            acp_session_id,
            request_id,
            include_document_symbols,
        } = event
        else {
            panic!("expected buffer snapshot request");
        };
        assert_eq!(acp_session_id, "thread");
        assert!(!include_document_symbols);
        assert!(
            client
                .answer_buffer_snapshots("other", &request_id, Vec::new(), None)
                .is_err()
        );
        let buffers = vec![AgentBufferSnapshot {
            path: "src/live.rs".into(),
            content: "unsaved".into(),
            symbols: Vec::new(),
        }];
        client
            .answer_buffer_snapshots("thread", &request_id, buffers.clone(), None)
            .expect("answer buffer snapshot request");
        assert_eq!(
            worker
                .join()
                .expect("join buffer worker")
                .expect("receive editor buffers"),
            buffers
        );

        let requester = client.clone();
        let worker = std::thread::spawn(move || {
            requester
                .runtime
                .block_on(requester.current_buffer_snapshots("thread", false))
        });
        let event = event_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("receive second buffer request");
        let HarnessEvent::BufferSnapshotsRequested { request_id, .. } = event else {
            panic!("expected buffer snapshot request");
        };
        client
            .answer_buffer_snapshots(
                "thread",
                &request_id,
                Vec::new(),
                Some("AHEAD editor buffers exceed the search snapshot limit".into()),
            )
            .expect("deliver editor error");
        assert!(
            worker
                .join()
                .expect("join buffer worker")
                .expect_err("editor limit should fail the tool")
                .to_string()
                .contains("snapshot limit")
        );

        let source = temporary.path().join("src/live.rs");
        std::fs::create_dir_all(source.parent().expect("source parent"))
            .expect("create source directory");
        std::fs::write(&source, "UNSAVED_SEARCH_MARKER disk-only")
            .expect("write saved source");
        let second_source = temporary.path().join("src/other.rs");
        std::fs::write(&second_source, "UNSAVED_SEARCH_MARKER disk-only")
            .expect("write second saved source");
        std::fs::create_dir_all(temporary.path().join("src/generated"))
            .expect("create excluded generated directory");
        std::fs::write(
            temporary.path().join("src/generated/artifact.rs"),
            "UNSAVED_SEARCH_MARKER generated-only",
        )
        .expect("write excluded generated source");
        let requester = client.clone();
        let workspace = temporary.path().to_path_buf();
        let worker = std::thread::spawn(move || {
            requester.runtime.block_on(requester.file_search(
                "thread",
                &SessionSettings {
                    cwd: workspace,
                    mode_id: "read-only".into(),
                    model: None,
                    model_provider: None,
                    loaded_provider_fingerprint: [0; 32],
                },
                None,
                json!({
                    "pattern": "UNSAVED_SEARCH_MARKER",
                    "include_pattern": "src/**/*.rs",
                    "exclude_pattern": "src/generated/**"
                }),
            ))
        });
        let event = event_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("receive file search buffer request");
        let HarnessEvent::BufferSnapshotsRequested {
            request_id,
            include_document_symbols,
            ..
        } = event
        else {
            panic!("expected file search buffer request");
        };
        assert!(include_document_symbols);
        client
            .answer_buffer_snapshots(
                "thread",
                &request_id,
                vec![
                    AgentBufferSnapshot {
                        path: "src/live.rs".into(),
                        content: "UNSAVED_SEARCH_MARKER editor-only first-tab"
                            .into(),
                        symbols: Vec::new(),
                    },
                    AgentBufferSnapshot {
                        path: "src/other.rs".into(),
                        content: "UNSAVED_SEARCH_MARKER editor-only second-tab"
                            .into(),
                        symbols: Vec::new(),
                    },
                ],
                None,
            )
            .expect("answer file search buffer request");
        let result = worker
            .join()
            .expect("join file search worker")
            .expect("search unsaved buffers");
        assert!(result.contains("src/live.rs"), "{result}");
        assert!(result.contains("src/other.rs"), "{result}");
        assert!(
            result.contains("UNSAVED_SEARCH_MARKER editor-only first-tab"),
            "{result}"
        );
        assert!(
            result.contains("UNSAVED_SEARCH_MARKER editor-only second-tab"),
            "{result}"
        );
        assert!(!result.contains("generated-only"), "{result}");
        assert!(!result.contains("disk-only"), "{result}");

        for index in 0..256 {
            std::fs::write(
                temporary.path().join(format!("src/load-{index:03}.rs")),
                "unrelated source\n",
            )
            .expect("write concurrent search workload");
        }
        let concurrent_searches = ["agent-one", "agent-two"]
            .into_iter()
            .map(|acp_session_id| {
                let requester = client.clone();
                let session_id = acp_session_id.to_string();
                let worker_session_id = session_id.clone();
                let settings = SessionSettings {
                    cwd: temporary.path().to_path_buf(),
                    mode_id: "read-only".into(),
                    model: None,
                    model_provider: None,
                    loaded_provider_fingerprint: [0; 32],
                };
                let worker = std::thread::spawn(move || {
                    requester.runtime.block_on(requester.file_search(
                        &worker_session_id,
                        &settings,
                        None,
                        json!({"pattern": "UNSAVED_SEARCH_MARKER"}),
                    ))
                });
                (session_id, worker)
            })
            .collect::<Vec<_>>();
        let mut search_requests = Vec::with_capacity(concurrent_searches.len());
        for _ in 0..concurrent_searches.len() {
            let event = event_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("receive concurrent file search snapshot request");
            let HarnessEvent::BufferSnapshotsRequested {
                acp_session_id,
                request_id,
                include_document_symbols,
            } = event
            else {
                panic!("expected concurrent file search buffer request");
            };
            assert!(include_document_symbols);
            search_requests.push((acp_session_id, request_id));
        }
        assert!(
            search_requests
                .iter()
                .any(|(session, _)| session == "agent-one")
        );
        assert!(
            search_requests
                .iter()
                .any(|(session, _)| session == "agent-two")
        );
        for (session_id, request_id) in search_requests {
            client
                .answer_buffer_snapshots(&session_id, &request_id, Vec::new(), None)
                .expect("answer concurrent file search snapshot request");
        }
        for (session_id, worker) in concurrent_searches {
            let result = worker
                .join()
                .expect("join concurrent file search")
                .expect("independent managed search must complete");
            assert!(result.contains("src/live.rs"), "{session_id}: {result}");
            assert!(result.contains("src/other.rs"), "{session_id}: {result}");
            assert!(result.contains("disk-only"), "{session_id}: {result}");
        }
        client.shutdown();
    }

    #[test]
    fn managed_stdio_mcp_form_reaches_server_with_typed_answers() {
        managed_stdio_mcp_elicitation_reaches_server("form");
    }

    #[test]
    fn managed_stdio_mcp_url_reaches_server_after_explicit_consent() {
        managed_stdio_mcp_elicitation_reaches_server("url");
    }

    fn managed_stdio_mcp_elicitation_reaches_server(mode: &str) {
        let tool_name: &'static str =
            if mode == "url" { "ask_url" } else { "ask_form" };
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir_all(&ahead).expect("private settings directory");
        let script = ahead.join("mcp_echo.py");
        let trace = ahead.join("mcp_calls.jsonl");
        std::fs::write(
            &script,
            include_str!("../../tests/fixtures/editor-smoke/mcp_echo.py"),
        )
        .expect("copy offline MCP server");
        let declaration = format!(
            "[mcp.servers.echo]\ncommand = \"python3\"\nargs = [\"-u\", {}, {}, \"--{mode}\"]\n",
            toml::Value::String(script.to_string_lossy().into_owned()),
            toml::Value::String(trace.to_string_lossy().into_owned()),
        );
        std::fs::write(ahead.join("config.toml"), &declaration)
            .expect("declare offline MCP server");
        let parsed = declaration.parse::<toml::Table>().expect("MCP declaration");
        let fingerprint = crate::runtime_support::mcp_declaration_fingerprint(
            &parsed["mcp"]["servers"]["echo"],
        )
        .expect("fingerprint MCP declaration");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock model");
        let address = listener.local_addr().expect("mock model address");
        std::fs::write(
            ahead.join("settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"gpt-5.5\"\n[mcp]\nenabled_servers = [\"echo\"]\n[mcp.approved_declarations]\necho = \"{fingerprint}\"\n"
            ),
        )
        .expect("configure local model and MCP server");
        let model_server = std::thread::spawn(move || {
            for turn in 0..3 {
                let (mut stream, _, _, _) = accept_model_post(&listener);
                let events = match turn {
                    0 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"form-search","type":"tool_search_call","call_id":"form-search","execution":"client","arguments":{"query":tool_name,"limit":1}}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"form-search","type":"tool_search_call","call_id":"form-search","execution":"client","arguments":{"query":tool_name,"limit":1}}}),
                        json!({"type":"response.completed","response":{"id":"form-response-1","end_turn":false}}),
                    ],
                    1 => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"form-call","type":"function_call","call_id":"form-call","namespace":"mcp__echo","name":tool_name,"arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"form-call","type":"function_call","call_id":"form-call","namespace":"mcp__echo","name":tool_name,"arguments":"{}"}}),
                        json!({"type":"response.completed","response":{"id":"form-response-2","end_turn":false}}),
                    ],
                    _ => vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"form-message","type":"message","role":"assistant","content":[]}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"form-message","type":"message","role":"assistant","content":[{"type":"output_text","text":"MCP form complete"}]}}),
                        json!({"type":"response.completed","response":{"id":"form-response-3","end_turn":true}}),
                    ],
                };
                let body = events
                    .into_iter()
                    .map(|event| {
                        format!(
                            "event: {}\ndata: {event}\n\n",
                            event["type"].as_str().expect("event type")
                        )
                    })
                    .collect::<String>();
                write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body).expect("write model response");
            }
        });
        let (input_sender, input_receiver) = std::sync::mpsc::channel();
        let client = Arc::new(
            NativeClient::spawn(
                &NativeClientConfig::ahead(workspace.path().to_path_buf()),
                Arc::new(TestHarnessStore::default()),
                Arc::new(move |event| {
                    if let HarnessEvent::UserInputRequested { .. } = event {
                        input_sender
                            .send(event)
                            .expect("forward agent input request");
                    }
                }),
            )
            .expect("start managed agent"),
        );
        let thread_id = client
            .new_session(workspace.path(), "agent", Some("gpt-5.5"), Some("mock"))
            .expect("create managed session");
        let (prompt_sender, prompt_receiver) = std::sync::mpsc::channel();
        let prompt_client = client.clone();
        let prompt_thread = thread_id.clone();
        std::thread::spawn(move || {
            prompt_sender
                .send(prompt_client.prompt(
                    &prompt_thread,
                    &format!("Call {tool_name} on echo and use the user's answer"),
                ))
                .expect("send prompt result");
        });
        let HarnessEvent::UserInputRequested {
            request_id,
            questions,
            ..
        } = input_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("MCP tool approval")
        else {
            panic!("expected tool approval");
        };
        assert_eq!(questions[0].header, "Approve MCP tool?");
        client
            .answer_user_input(
                &thread_id,
                &request_id,
                HashMap::from([(
                    questions[0].id.clone(),
                    vec!["Allow".to_string()],
                )]),
            )
            .expect("approve MCP tool");
        let HarnessEvent::UserInputRequested {
            request_id,
            questions,
            ..
        } = input_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("MCP elicitation request")
        else {
            panic!("expected MCP elicitation");
        };
        let answers = if mode == "url" {
            assert!(request_id.starts_with("mcp-url-"));
            assert_eq!(questions.len(), 1);
            assert!(questions[0].header.contains("MCP server: echo"));
            assert_eq!(
                questions[0].external_url.as_deref(),
                Some("https://example.test/connect?state=abc")
            );
            HashMap::from([(
                ahead_rpc::ahead::MCP_FORM_ACTION_KEY.to_string(),
                vec!["accept".to_string()],
            )])
        } else {
            assert!(request_id.starts_with("mcp-form-"));
            assert_eq!(questions.len(), 3);
            assert!(
                questions
                    .iter()
                    .all(|question| question.header.contains("MCP server: echo"))
            );
            let field_id = |name: &str| {
                questions
                    .iter()
                    .find(|question| {
                        question.header.contains(&format!(" · {name} ("))
                    })
                    .expect("MCP form field")
                    .id
                    .clone()
            };
            let color_id = field_id("color");
            let skip_value = questions
                .iter()
                .find(|question| question.id == color_id)
                .and_then(|question| {
                    question
                        .options
                        .iter()
                        .find(|option| option.label == "Skip")
                })
                .expect("Skip option")
                .value
                .clone();
            HashMap::from([
                (color_id, vec![skip_value]),
                (field_id("count"), vec!["2".to_string()]),
                (field_id("nickname"), vec!["Nora".to_string()]),
            ])
        };
        client
            .answer_user_input(&thread_id, &request_id, answers)
            .expect("submit MCP elicitation");
        prompt_receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("managed turn completed")
            .expect("managed turn succeeded");
        model_server.join().expect("join model server");
        let trace = std::fs::read_to_string(&trace).expect("MCP server trace");
        assert!(
            trace.contains("\"method\": \"elicitation/result\""),
            "{trace}"
        );
        assert!(trace.contains("\"action\": \"accept\""), "{trace}");
        if mode == "url" {
            let response: Value = serde_json::from_str(
                trace.lines().last().expect("MCP URL response trace"),
            )
            .expect("MCP URL response JSON");
            assert_eq!(response["params"]["content"], json!({}));
        } else {
            assert!(trace.contains("\"nickname\": \"Nora\""), "{trace}");
            assert!(trace.contains("\"count\": 2"), "{trace}");
            assert!(
                !trace.contains("\"color\""),
                "optional color was skipped: {trace}"
            );
        }
        client.shutdown();
    }

    #[test]
    fn managed_model_file_search_uses_live_editor_buffer() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        let agents_doc = "AHEAD_PROJECT_PROVENANCE_20261003";
        let agents_path = workspace.path().join("AGENTS.md");
        std::fs::write(&agents_path, agents_doc)
            .expect("write project instructions");
        std::fs::create_dir_all(workspace.path().join(".ahead"))
            .expect("create private settings directory");
        std::fs::create_dir_all(workspace.path().join("src"))
            .expect("create source directory");
        std::fs::write(
            workspace.path().join("src/live.rs"),
            "SEARCH_MARKER disk-only",
        )
        .expect("write saved source");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock model");
        let address = listener.local_addr().expect("mock model address");
        let (model_request_sender, model_request_receiver) =
            std::sync::mpsc::channel();
        let (release_first_response, first_response_released) =
            std::sync::mpsc::channel();
        std::fs::write(
            workspace.path().join(".ahead/settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"gpt-5.5\"\n"
            ),
        )
        .expect("configure local model");
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for turn in 0..2 {
                let (mut stream, request, header_end, content_length) =
                    accept_model_post(&listener);
                requests.push(
                    serde_json::from_slice::<Value>(
                        &request[header_end..header_end + content_length],
                    )
                    .expect("model request JSON"),
                );
                model_request_sender
                    .send(turn)
                    .expect("record model request");
                if turn == 0 {
                    first_response_released
                        .recv_timeout(Duration::from_secs(10))
                        .expect("release first model response");
                }
                let events = if turn == 0 {
                    vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"search-call","type":"function_call","call_id":"search-call","name":"file_search","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"search-call","type":"function_call","call_id":"search-call","name":"file_search","arguments":"{\"pattern\":\"SEARCH_MARKER\"}"}}),
                        json!({"type":"response.completed","response":{"id":"search-response-1","end_turn":false}}),
                    ]
                } else {
                    vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"search-message","type":"message","role":"assistant","content":[]}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"search-message","type":"message","role":"assistant","content":[{"type":"output_text","text":"Search complete"}]}}),
                        json!({"type":"response.completed","response":{"id":"search-response-2","end_turn":true}}),
                    ]
                };
                let body = events
                    .into_iter()
                    .map(|event| {
                        format!(
                            "event: {}\ndata: {event}\n\n",
                            event["type"].as_str().expect("event type")
                        )
                    })
                    .collect::<String>();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write model response");
            }
            requests
        });

        let (event_sender, event_receiver) = std::sync::mpsc::channel();
        let store = Arc::new(TestHarnessStore::default());
        let client = Arc::new(
            NativeClient::spawn(
                &NativeClientConfig::ahead(workspace.path().to_path_buf()),
                store.clone(),
                Arc::new(move |event| {
                    if let HarnessEvent::BufferSnapshotsRequested { .. } = event {
                        event_sender.send(event).expect("send buffer request");
                    }
                }),
            )
            .expect("start managed agent"),
        );
        let thread_id = client
            .new_session(
                workspace.path(),
                "read-only",
                Some("gpt-5.5"),
                Some("mock"),
            )
            .expect("create managed session");
        let prompt_client = client.clone();
        let prompt_thread_id = thread_id.clone();
        let (prompt_sender, prompt_receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(prompt_sender.send(prompt_client.prompt_for_ahead_turn(
                &prompt_thread_id,
                "Search for SEARCH_MARKER",
                "turn-file-search",
                None,
            )));
        });
        assert_eq!(
            model_request_receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("managed turn did not reach the model"),
            0,
        );
        let snapshot_started = std::time::Instant::now();
        let saved_sources = loop {
            match store.get_project_instruction_sources("turn-file-search") {
                Ok(Some(sources)) => break sources,
                Ok(None) if snapshot_started.elapsed() < Duration::from_secs(10) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    panic!(
                        "managed turn did not persist instructions while streaming"
                    )
                }
                Err(error) => panic!("read in-flight instruction snapshot: {error}"),
            }
        };
        assert_eq!(saved_sources.len(), 1);
        assert_eq!(
            saved_sources[0].content_sha256,
            format!("{:x}", Sha256::digest(agents_doc.as_bytes())),
        );
        release_first_response
            .send(())
            .expect("release first model response");
        let HarnessEvent::BufferSnapshotsRequested { request_id, .. } =
            event_receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|error| {
                    panic!(
                        "model search did not ask for live buffers: {error}; prompt={:?}; model requests={:?}",
                        prompt_receiver.try_recv().map(|result| result.map(|_| ())),
                        model_request_receiver.try_iter().collect::<Vec<_>>()
                    )
                })
        else {
            panic!("expected live buffer request");
        };
        client
            .answer_buffer_snapshots(
                &thread_id,
                &request_id,
                vec![
                    AgentBufferSnapshot {
                        path: "src/live.rs".into(),
                        content: "SEARCH_MARKER editor-only".into(),
                        symbols: Vec::new(),
                    },
                    AgentBufferSnapshot {
                        path: ".ahead/settings.toml".into(),
                        content: "SEARCH_MARKER private-only".into(),
                        symbols: Vec::new(),
                    },
                ],
                None,
            )
            .expect("answer live editor request");
        prompt_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("managed search turn timed out")
            .expect("complete managed search turn");
        let requests = server.join().expect("join local model server");
        assert!(requests[0].to_string().contains(agents_doc));
        assert_eq!(
            saved_sources[0]
                .path
                .canonicalize()
                .expect("saved source path"),
            agents_path.canonicalize().expect("expected source path"),
        );
        assert_eq!(
            saved_sources[0].content_sha256,
            format!("{:x}", Sha256::digest(agents_doc.as_bytes())),
        );
        assert_eq!(
            client
                .current_project_instruction_sources(&thread_id)
                .expect("refresh unchanged project instructions"),
            saved_sources,
        );
        std::fs::write(&agents_path, "AHEAD_PROJECT_PROVENANCE_CHANGED_20261003")
            .expect("change project instructions");
        assert_ne!(
            client
                .current_project_instruction_sources(&thread_id)
                .expect("refresh changed project instructions"),
            saved_sources,
        );
        assert!(
            requests[0]["tools"]
                .as_array()
                .expect("model tools")
                .iter()
                .any(|tool| tool["name"] == AHEAD_FILE_SEARCH_TOOL),
            "file_search must be advertised to the direct-tool model"
        );
        let continuation = requests[1].to_string();
        assert!(continuation.contains("SEARCH_MARKER editor-only"));
        assert!(!continuation.contains("disk-only"));
        assert!(!continuation.contains("private-only"));
        client.shutdown();
    }

    #[test]
    fn managed_context_window_override_resets_to_startup_value() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        std::fs::create_dir_all(workspace.path().join(".ahead"))
            .expect("create private settings directory");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock model");
        let address = listener.local_addr().expect("mock model address");
        std::fs::write(
            workspace.path().join(".ahead/settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"gpt-5.5\"\n"
            ),
        )
        .expect("configure local model");
        let model_server = std::thread::spawn(move || {
            for turn in 0..2 {
                let (mut stream, _, _, _) = accept_model_post(&listener);
                let events = vec![
                    json!({"type":"response.output_item.added","output_index":0,"item":{"id":format!("context-message-{turn}"),"type":"message","role":"assistant","content":[]}}),
                    json!({"type":"response.output_item.done","output_index":0,"item":{"id":format!("context-message-{turn}"),"type":"message","role":"assistant","content":[{"type":"output_text","text":"Context window turn complete"}]}}),
                    json!({"type":"response.completed","response":{"id":format!("context-response-{turn}"),"end_turn":true}}),
                ];
                let body = events
                    .into_iter()
                    .map(|event| {
                        format!(
                            "event: {}\ndata: {event}\n\n",
                            event["type"].as_str().expect("event type")
                        )
                    })
                    .collect::<String>();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write model response");
            }
        });

        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(workspace.path().to_path_buf()),
            Arc::new(TestHarnessStore::default()),
            Arc::new(|_| {}),
        )
        .expect("start managed agent");
        let thread_id = client
            .new_session(
                workspace.path(),
                "read-only",
                Some("gpt-5.5"),
                Some("mock"),
            )
            .expect("create managed session");
        let thread = client
            .threads
            .lock()
            .get(&thread_id)
            .cloned()
            .expect("loaded runtime thread");
        let startup_context_window = client
            .runtime
            .block_on(thread.restorable_thread_settings())
            .model_context_window;

        client
            .prompt_for_ahead_turn(
                &thread_id,
                "Use the selected model context window",
                "turn-context-window",
                Some(120_000),
            )
            .expect("managed turn with model context override");
        assert_eq!(
            client
                .runtime
                .block_on(thread.restorable_thread_settings())
                .model_context_window,
            Some(Some(std::num::NonZeroU32::new(120_000).expect("nonzero"))),
        );

        client
            .prompt_for_ahead_turn(
                &thread_id,
                "Clear the selected model context window",
                "turn-context-window-reset",
                None,
            )
            .expect("managed turn clearing the model context override");
        assert_eq!(
            client
                .runtime
                .block_on(thread.restorable_thread_settings())
                .model_context_window,
            startup_context_window,
        );
        model_server.join().expect("join local model server");
        client.shutdown();
    }

    #[test]
    fn file_search_pages_matches_without_silent_truncation() {
        let workspace = Path::new("/workspace");
        let matches = (0..23)
            .map(|line| ahead_core::search::FileSearchMatch {
                line,
                start: 0,
                end_line: if line == 0 { 1 } else { line },
                end: 6,
                line_content: "needle".to_string(),
                preview_match: 0..6,
            })
            .collect::<Vec<_>>();
        let path = workspace.join("src/lib.rs");
        let results = vec![(path.clone(), matches.clone())];
        let query_fingerprint = "a".repeat(64);

        let first = format_file_search_results(
            workspace,
            results.clone(),
            0,
            7,
            &query_fingerprint,
            &std::collections::HashMap::new(),
        )
        .expect("format first page");
        assert_eq!(first.matches("  ").count(), AGENT_SEARCH_PAGE_SIZE);
        assert!(first.contains("\nsrc/lib.rs"));
        assert!(!first.contains("/workspace"));
        assert!(first.contains("1:0-2:6 needle"));
        let encoded_cursor = first
            .split_once("cursor `")
            .expect("first page provides a cursor")
            .1
            .split_once('`')
            .expect("cursor is delimited")
            .0;
        let cursor = parse_file_search_cursor(encoded_cursor)
            .expect("parse next-page cursor");
        assert_eq!(cursor.offset, AGENT_SEARCH_PAGE_SIZE);
        assert_eq!(cursor.search_revision, 7);
        validate_file_search_cursor(&cursor, 7, &query_fingerprint, &results)
            .expect("unchanged results keep cursor valid");

        let mut changed_results = results.clone();
        changed_results[0].1[0].line_content = "changed".to_string();
        assert!(
            validate_file_search_cursor(
                &cursor,
                7,
                &query_fingerprint,
                &changed_results,
            )
            .expect_err("changed earlier results invalidate cursor")
            .to_string()
            .contains("earlier results changed")
        );
        assert!(
            validate_file_search_cursor(&cursor, 8, &query_fingerprint, &results,)
                .is_err()
        );
        assert!(
            validate_file_search_cursor(&cursor, 7, &"b".repeat(64), &results)
                .is_err()
        );

        let second = format_file_search_results(
            workspace,
            vec![(path, matches)],
            AGENT_SEARCH_PAGE_SIZE,
            7,
            &query_fingerprint,
            &std::collections::HashMap::new(),
        )
        .expect("format second page");
        assert_eq!(second.matches("  ").count(), 3);
        assert!(!second.contains("More matches available"));
        assert!(
            format_file_search_results(
                workspace,
                vec![(PathBuf::from("/outside/lib.rs"), results[0].1.clone())],
                0,
                7,
                &query_fingerprint,
                &std::collections::HashMap::new(),
            )
            .expect_err("out-of-workspace result must fail closed")
            .to_string()
            .contains("outside the active workspace")
        );
        assert!(
            format_file_search_results(
                workspace,
                vec![(
                    PathBuf::from("/workspace/../outside/lib.rs"),
                    results[0].1.clone()
                )],
                0,
                7,
                &query_fingerprint,
                &std::collections::HashMap::new(),
            )
            .expect_err("parent components must not appear in results")
            .to_string()
            .contains("outside the active workspace")
        );
    }

    #[test]
    fn file_search_results_add_open_buffer_symbol_hierarchy() {
        let workspace = tempfile::tempdir().expect("workspace");
        let path = workspace.path().join("src/lib.rs");
        let matches = vec![ahead_core::search::FileSearchMatch {
            line: 4,
            start: 4,
            end_line: 4,
            end: 10,
            line_content: "    needle()".into(),
            preview_match: 4..10,
        }];
        let symbols = std::collections::HashMap::from([(
            path.clone(),
            vec![
                AgentBufferSymbol {
                    name: "outer".into(),
                    start_line: 0,
                    end_line: 20,
                    depth: 0,
                },
                AgentBufferSymbol {
                    name: "nested_fn".into(),
                    start_line: 2,
                    end_line: 9,
                    depth: 1,
                },
            ],
        )]);

        let output = format_file_search_results(
            workspace.path(),
            vec![(path.clone(), matches.clone())],
            0,
            1,
            &"a".repeat(64),
            &symbols,
        )
        .expect("format search result");

        assert!(
            output.contains("needle()  (in outer › nested_fn)"),
            "{output}"
        );

        let flat_symbols = vec![
            AgentBufferSymbol {
                name: "flat_file".into(),
                start_line: 0,
                end_line: 20,
                depth: 0,
            },
            AgentBufferSymbol {
                name: "flat_fn".into(),
                start_line: 2,
                end_line: 9,
                depth: 0,
            },
        ];
        let flat_output = format_file_search_results(
            workspace.path(),
            vec![(path.clone(), matches.clone())],
            0,
            1,
            &"a".repeat(64),
            &std::collections::HashMap::from([(path.clone(), flat_symbols)]),
        )
        .expect("format flat search result");
        assert!(flat_output.contains("(in flat_fn)"), "{flat_output}");
        assert!(!flat_output.contains("flat_file"), "{flat_output}");

        let deep_symbols = (0..MAX_AGENT_SEARCH_SYMBOL_CONTEXT_DEPTH + 2)
            .map(|depth| AgentBufferSymbol {
                name: format!("level_{depth}"),
                start_line: 0,
                end_line: 20,
                depth: depth as u16,
            })
            .collect();
        let deep_output = format_file_search_results(
            workspace.path(),
            vec![(
                workspace.path().join("src/lib.rs"),
                vec![ahead_core::search::FileSearchMatch {
                    line: 4,
                    start: 4,
                    end_line: 4,
                    end: 10,
                    line_content: "    needle()".into(),
                    preview_match: 4..10,
                }],
            )],
            0,
            1,
            &"a".repeat(64),
            &std::collections::HashMap::from([(
                workspace.path().join("src/lib.rs"),
                deep_symbols,
            )]),
        )
        .expect("format bounded search result");
        assert!(
            deep_output.contains("in … › level_2 › level_3"),
            "{deep_output}"
        );
        assert!(deep_output.contains("level_9)"), "{deep_output}");
        assert!(!deep_output.contains("level_1"), "{deep_output}");
    }

    #[test]
    fn file_search_query_fingerprint_binds_options_and_unsaved_buffers() {
        let workspace = Path::new("/workspace");
        let mut options = ahead_core::search::FileSearchOptions {
            pattern: "needle".to_string(),
            case_sensitive: false,
            whole_word: false,
            is_regex: false,
            max_results: AGENT_SEARCH_PAGE_SIZE + 1,
        };
        let mut buffers = HashMap::from([(
            workspace.join("src/lib.rs"),
            "unsaved needle".to_string(),
        )]);
        let original = file_search_query_fingerprint(
            workspace,
            &options,
            Some("src/**/*.rs"),
            Some("src/generated/**"),
            Some(&["src".to_string()]),
            &buffers,
        );

        assert_ne!(
            original,
            file_search_query_fingerprint(
                workspace,
                &options,
                Some("src/**/*.rs"),
                Some("src/vendor/**"),
                Some(&["src".to_string()]),
                &buffers,
            )
        );

        buffers.insert(
            workspace.join("src/lib.rs"),
            "unsaved text changed".to_string(),
        );
        assert_ne!(
            original,
            file_search_query_fingerprint(
                workspace,
                &options,
                Some("src/**/*.rs"),
                Some("src/generated/**"),
                Some(&["src".to_string()]),
                &buffers,
            )
        );

        options.case_sensitive = true;
        assert_ne!(
            original,
            file_search_query_fingerprint(
                workspace,
                &options,
                Some("src/**/*.rs"),
                Some("src/generated/**"),
                Some(&["src".to_string()]),
                &buffers,
            )
        );
    }

    fn turn_started_item(turn_id: &str) -> codex_rollout::RolloutItem {
        codex_rollout::RolloutItem::EventMsg(
            codex_protocol::protocol::EventMsg::TurnStarted(
                codex_protocol::protocol::TurnStartedEvent {
                    turn_id: turn_id.to_string(),
                    trace_id: None,
                    started_at: None,
                    model_context_window: Some(128_000),
                    collaboration_mode_kind: Default::default(),
                },
            ),
        )
    }

    fn turn_complete_item(turn_id: &str) -> codex_rollout::RolloutItem {
        codex_rollout::RolloutItem::EventMsg(
            codex_protocol::protocol::EventMsg::TurnComplete(
                codex_protocol::protocol::TurnCompleteEvent {
                    turn_id: turn_id.to_string(),
                    last_agent_message: None,
                    error: None,
                    started_at: None,
                    completed_at: None,
                    duration_ms: None,
                    time_to_first_token_ms: None,
                },
            ),
        )
    }

    #[derive(Default)]
    struct TestHarnessStore {
        agent_runtime_states: Mutex<HashMap<String, AgentRuntimeState>>,
        project_instruction_sources:
            Mutex<HashMap<String, Vec<crate::ProjectInstructionSource>>>,
        turn_skill_sources: Mutex<HashMap<String, Vec<crate::TurnSkillSource>>>,
        turn_skill_references:
            Mutex<HashMap<String, Vec<crate::TurnSkillReference>>>,
        native_threads: Mutex<HashMap<String, crate::NativeThreadSnapshot>>,
        fail_native_append: AtomicBool,
        fail_native_metadata_append: AtomicBool,
        native_agent_edges:
            Mutex<HashMap<String, (String, crate::NativeAgentEdgeStatus)>>,
        header_timestamps: Mutex<
            Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        >,
        full_thread_loads: AtomicUsize,
        replay_page_loads: AtomicUsize,
    }

    impl HarnessStore for TestHarnessStore {
        fn get_task_intent(&self, _session_id: &str) -> Result<Option<TaskIntent>> {
            Ok(None)
        }

        fn next_message_sequence(&self, _session_id: &str) -> Result<i64> {
            Ok(0)
        }

        fn upsert_message(&self, _message: &ConversationMessage) -> Result<()> {
            Ok(())
        }

        fn append_message_delta(
            &self,
            _message_id: &str,
            _delta: &str,
        ) -> Result<String> {
            Ok(String::new())
        }

        fn set_message_status(
            &self,
            _message_id: &str,
            _status: &str,
        ) -> Result<()> {
            Ok(())
        }

        fn get_agent_runtime_state(
            &self,
            session_id: &str,
        ) -> Result<Option<AgentRuntimeState>> {
            Ok(self.agent_runtime_states.lock().get(session_id).cloned())
        }

        fn set_agent_runtime_state(
            &self,
            session_id: &str,
            state: &AgentRuntimeState,
        ) -> Result<()> {
            self.agent_runtime_states
                .lock()
                .insert(session_id.to_string(), state.clone());
            Ok(())
        }

        fn save_turn_request(
            &self,
            _turn_id: &str,
            _request: &AgentTurnRequestDto,
            _instruction_sources: &[crate::InstructionFileSource],
        ) -> Result<()> {
            Ok(())
        }

        fn get_turn_request(
            &self,
            _turn_id: &str,
        ) -> Result<Option<AgentTurnRequestDto>> {
            Ok(None)
        }

        fn list_turn_instruction_sources(
            &self,
            _session_id: &str,
            _turn_id: &str,
        ) -> Result<Vec<crate::InstructionFileSource>> {
            Ok(Vec::new())
        }

        fn get_project_instruction_sources(
            &self,
            turn_id: &str,
        ) -> Result<Option<Vec<crate::ProjectInstructionSource>>> {
            Ok(self
                .project_instruction_sources
                .lock()
                .get(turn_id)
                .cloned())
        }

        fn set_project_instruction_sources(
            &self,
            turn_id: &str,
            sources: &[crate::ProjectInstructionSource],
        ) -> Result<()> {
            self.project_instruction_sources
                .lock()
                .insert(turn_id.to_string(), sources.to_vec());
            Ok(())
        }

        fn set_turn_skill_sources(
            &self,
            turn_id: &str,
            sources: &[crate::TurnSkillSource],
        ) -> Result<()> {
            self.turn_skill_sources
                .lock()
                .insert(turn_id.to_string(), sources.to_vec());
            Ok(())
        }

        fn get_turn_skill_sources(
            &self,
            turn_id: &str,
        ) -> Result<Option<Vec<crate::TurnSkillSource>>> {
            Ok(self.turn_skill_sources.lock().get(turn_id).cloned())
        }

        fn append_turn_skill_reference(
            &self,
            turn_id: &str,
            reference: &crate::TurnSkillReference,
        ) -> Result<()> {
            let mut references = self.turn_skill_references.lock();
            let turn_references = references.entry(turn_id.to_string()).or_default();
            if !turn_references.contains(reference) {
                turn_references.push(reference.clone());
            }
            Ok(())
        }

        fn get_turn_skill_references(
            &self,
            turn_id: &str,
        ) -> Result<Option<Vec<crate::TurnSkillReference>>> {
            Ok(self.turn_skill_references.lock().get(turn_id).cloned())
        }

        fn remove_turn_request(&self, _turn_id: &str) -> Result<()> {
            Ok(())
        }

        fn list_messages(
            &self,
            _session_id: &str,
        ) -> Result<Vec<ConversationMessage>> {
            Ok(Vec::new())
        }

        fn session_title(&self, _session_id: &str) -> Result<Option<String>> {
            Ok(None)
        }

        fn update_session_title(
            &self,
            _session_id: &str,
            _title: &str,
        ) -> Result<()> {
            Ok(())
        }

        fn set_harness_binding(
            &self,
            _session_id: &str,
            _acp_session_id: &str,
            _backend: &str,
        ) -> Result<()> {
            Ok(())
        }

        fn get_harness_binding(
            &self,
            _session_id: &str,
        ) -> Result<Option<(String, String)>> {
            Ok(None)
        }

        fn record_edit_anchor(&self, _anchor: &CodeAnchor) -> Result<()> {
            Ok(())
        }

        fn create_native_thread(
            &self,
            thread_id: &str,
            create_params: Value,
        ) -> Result<()> {
            let previous = self.native_threads.lock().insert(
                thread_id.to_string(),
                crate::NativeThreadSnapshot {
                    create_params,
                    metadata_patches: Vec::new(),
                    rollout_items: Vec::new(),
                    archived: false,
                    archived_at: None,
                },
            );
            anyhow::ensure!(previous.is_none(), "duplicate test thread {thread_id}");
            Ok(())
        }

        fn import_legacy_native_thread(
            &self,
            import: LegacyNativeThreadImport,
        ) -> Result<bool> {
            let mut threads = self.native_threads.lock();
            if threads.contains_key(&import.thread_id) {
                return Ok(false);
            }
            threads.insert(
                import.thread_id,
                crate::NativeThreadSnapshot {
                    create_params: import.create_params,
                    metadata_patches: Vec::new(),
                    rollout_items: import.rollout_items,
                    archived: import.archived_at.is_some(),
                    archived_at: import.archived_at,
                },
            );
            Ok(true)
        }

        fn append_native_thread_items(
            &self,
            thread_id: &str,
            items: Vec<Value>,
        ) -> Result<()> {
            anyhow::ensure!(
                !self.fail_native_append.load(Ordering::Relaxed),
                "injected native history write failure"
            );
            self.native_threads
                .lock()
                .get_mut(thread_id)
                .context("test native thread not found")?
                .rollout_items
                .extend(items);
            Ok(())
        }

        fn append_native_thread_metadata(
            &self,
            thread_id: &str,
            patch: Value,
        ) -> Result<()> {
            anyhow::ensure!(
                !self.fail_native_metadata_append.load(Ordering::Relaxed),
                "injected native metadata write failure"
            );
            self.native_threads
                .lock()
                .get_mut(thread_id)
                .context("test native thread not found")?
                .metadata_patches
                .push(patch);
            Ok(())
        }

        fn load_native_thread(
            &self,
            thread_id: &str,
        ) -> Result<Option<crate::NativeThreadSnapshot>> {
            self.full_thread_loads.fetch_add(1, Ordering::Relaxed);
            Ok(self.native_threads.lock().get(thread_id).cloned())
        }

        fn load_native_thread_header(
            &self,
            thread_id: &str,
        ) -> Result<Option<crate::NativeThreadHeader>> {
            let timestamps = self.header_timestamps.lock().clone();
            Ok(self.native_threads.lock().get(thread_id).map(|snapshot| {
                crate::NativeThreadHeader {
                    thread_id: thread_id.to_string(),
                    create_params: snapshot.create_params.clone(),
                    metadata_patches: snapshot.metadata_patches.clone(),
                    archived: snapshot.archived,
                    archived_at: snapshot.archived_at,
                    created_at: timestamps
                        .as_ref()
                        .map(|(created_at, _)| created_at.clone()),
                    updated_at: timestamps
                        .as_ref()
                        .map(|(_, updated_at)| updated_at.clone()),
                }
            }))
        }

        fn load_native_thread_item_page(
            &self,
            thread_id: &str,
            before_ordinal: Option<i64>,
            limit: usize,
        ) -> Result<crate::NativeThreadReplayPage> {
            anyhow::ensure!(
                (1..=256).contains(&limit),
                "test replay page limit must be between 1 and 256"
            );
            self.replay_page_loads.fetch_add(1, Ordering::Relaxed);
            let threads = self.native_threads.lock();
            let items = &threads
                .get(thread_id)
                .context("test native thread not found")?
                .rollout_items;
            let end = before_ordinal
                .map(usize::try_from)
                .transpose()
                .context("invalid replay page cursor")?
                .unwrap_or(items.len())
                .min(items.len());
            let start = end.saturating_sub(limit);
            let next_before_ordinal =
                (start > 0).then(|| i64::try_from(start)).transpose()?;
            Ok(crate::NativeThreadReplayPage {
                items: items[start..end].iter().rev().cloned().collect(),
                next_before_ordinal,
            })
        }

        fn list_native_thread_headers(
            &self,
        ) -> Result<Vec<crate::NativeThreadHeader>> {
            let timestamps = self.header_timestamps.lock().clone();
            let mut headers = self
                .native_threads
                .lock()
                .iter()
                .map(|(thread_id, snapshot)| crate::NativeThreadHeader {
                    thread_id: thread_id.clone(),
                    create_params: snapshot.create_params.clone(),
                    metadata_patches: snapshot.metadata_patches.clone(),
                    archived: snapshot.archived,
                    archived_at: snapshot.archived_at,
                    created_at: timestamps
                        .as_ref()
                        .map(|(created_at, _)| created_at.clone()),
                    updated_at: timestamps
                        .as_ref()
                        .map(|(_, updated_at)| updated_at.clone()),
                })
                .collect::<Vec<_>>();
            headers.sort_by(|left, right| left.thread_id.cmp(&right.thread_id));
            Ok(headers)
        }

        fn set_native_thread_archived(
            &self,
            thread_id: &str,
            archived: bool,
        ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
            let archived_at = archived.then(chrono::Utc::now);
            let mut threads = self.native_threads.lock();
            let snapshot = threads
                .get_mut(thread_id)
                .context("test native thread not found")?;
            snapshot.archived = archived;
            snapshot.archived_at = archived_at;
            Ok(archived_at)
        }

        fn delete_native_thread(&self, thread_id: &str) -> Result<()> {
            self.native_threads.lock().remove(thread_id);
            self.native_agent_edges.lock().retain(|child, (parent, _)| {
                child != thread_id && parent != thread_id
            });
            Ok(())
        }

        fn upsert_native_agent_edge(
            &self,
            parent_thread_id: &str,
            child_thread_id: &str,
            status: crate::NativeAgentEdgeStatus,
        ) -> Result<()> {
            self.native_agent_edges.lock().insert(
                child_thread_id.to_string(),
                (parent_thread_id.to_string(), status),
            );
            Ok(())
        }

        fn set_native_agent_edge_status(
            &self,
            child_thread_id: &str,
            status: crate::NativeAgentEdgeStatus,
        ) -> Result<()> {
            if let Some((_, current)) =
                self.native_agent_edges.lock().get_mut(child_thread_id)
            {
                *current = status;
            }
            Ok(())
        }

        fn list_native_agent_children(
            &self,
            parent_thread_id: &str,
            status: Option<crate::NativeAgentEdgeStatus>,
        ) -> Result<Vec<String>> {
            let mut children = self
                .native_agent_edges
                .lock()
                .iter()
                .filter(|(_, (parent, edge_status))| {
                    parent == parent_thread_id
                        && status.is_none_or(|status| *edge_status == status)
                })
                .map(|(child, _)| child.clone())
                .collect::<Vec<_>>();
            children.sort();
            Ok(children)
        }
    }

    #[test]
    fn native_agent_graph_orders_descendants_and_filters_closed_edges() {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create test runtime");
        let graph = TursoAgentGraphStore::new(Arc::new(TestHarnessStore::default()));
        let root = ThreadId::from_u128(1);
        let first = ThreadId::from_u128(2);
        let second = ThreadId::from_u128(3);
        let grandchild = ThreadId::from_u128(4);
        runtime.block_on(async {
            graph
                .upsert_thread_spawn_edge(root, second, ThreadSpawnEdgeStatus::Open)
                .await
                .expect("store second child");
            graph
                .upsert_thread_spawn_edge(root, first, ThreadSpawnEdgeStatus::Open)
                .await
                .expect("store first child");
            graph
                .upsert_thread_spawn_edge(
                    first,
                    grandchild,
                    ThreadSpawnEdgeStatus::Open,
                )
                .await
                .expect("store grandchild");
            assert_eq!(
                graph
                    .list_thread_spawn_descendants(root, None)
                    .await
                    .expect("list descendants"),
                vec![first, second, grandchild]
            );
            graph
                .set_thread_spawn_edge_status(first, ThreadSpawnEdgeStatus::Closed)
                .await
                .expect("close first child");
            assert_eq!(
                graph
                    .list_thread_spawn_descendants(
                        root,
                        Some(ThreadSpawnEdgeStatus::Open),
                    )
                    .await
                    .expect("list open descendants"),
                vec![second]
            );
        });
    }

    #[test]
    fn native_thread_listing_hydrates_active_and_archived_headers_without_history() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let store = Arc::new(TestHarnessStore::default());
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(temporary.path().to_path_buf()),
            store.clone(),
            Arc::new(|_| {}),
        )
        .expect("start native client");
        let active = client
            .new_session(temporary.path(), "agent", None, None)
            .expect("create active thread");
        let archived = client
            .new_session(temporary.path(), "agent", None, None)
            .expect("create archived thread");
        client.shutdown();
        store
            .set_native_thread_archived(&archived, true)
            .expect("archive native thread");
        let created_at =
            chrono::DateTime::parse_from_rfc3339("2025-01-02T03:04:05Z")
                .expect("valid creation time")
                .with_timezone(&chrono::Utc);
        let updated_at =
            chrono::DateTime::parse_from_rfc3339("2025-01-03T04:05:06Z")
                .expect("valid update time")
                .with_timezone(&chrono::Utc);
        *store.header_timestamps.lock() = Some((created_at, updated_at));

        let fresh = TursoThreadStore::new(store.clone());
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create test runtime");
        let cold_read = runtime
            .block_on(fresh.read_thread(codex_thread_store::ReadThreadParams {
                thread_id:
                    ThreadId::from_string(&active).expect("valid active thread id"),
                include_archived: false,
                include_history: false,
            }))
            .expect("read active thread metadata before listing");
        assert_eq!(cold_read.created_at, created_at);
        assert_eq!(cold_read.updated_at, updated_at);
        let params = codex_thread_store::ListThreadsParams {
            page_size: 20,
            cursor: None,
            sort_key: codex_thread_store::ThreadSortKey::UpdatedAt,
            sort_direction: codex_thread_store::SortDirection::Desc,
            allowed_sources: Vec::new(),
            model_providers: None,
            cwd_filters: None,
            section: None,
            project_id: None,
            archived: false,
            search_term: None,
            relation_filter: None,
        };
        let active_page = runtime
            .block_on(fresh.list_threads(params.clone()))
            .expect("list active native threads");
        assert_eq!(active_page.items.len(), 1);
        assert_eq!(active_page.items[0].thread_id.to_string(), active);
        assert_eq!(active_page.items[0].created_at, created_at);
        assert_eq!(active_page.items[0].updated_at, updated_at);
        let archived_page = runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                archived: true,
                ..params.clone()
            }))
            .expect("list archived native threads");
        assert_eq!(archived_page.items.len(), 1);
        assert_eq!(archived_page.items[0].thread_id.to_string(), archived);
        let next_update =
            chrono::DateTime::parse_from_rfc3339("2025-01-04T05:06:07Z")
                .expect("valid later update time")
                .with_timezone(&chrono::Utc);
        *store.header_timestamps.lock() = Some((created_at, next_update));
        let updated_page = runtime
            .block_on(fresh.list_threads(params))
            .expect("refresh persisted listing timestamps");
        assert_eq!(updated_page.items[0].updated_at, next_update);
        assert_eq!(store.full_thread_loads.load(Ordering::Relaxed), 0);

        runtime
            .block_on(fresh.read_thread(codex_thread_store::ReadThreadParams {
                thread_id:
                    ThreadId::from_string(&active).expect("valid active thread id"),
                include_archived: false,
                include_history: false,
            }))
            .expect("read active thread metadata after listing");
        assert_eq!(store.full_thread_loads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn thread_source_deserialization_rejects_unknown_labels() {
        use codex_protocol::protocol::ThreadSource;

        assert_eq!(
            serde_json::from_str::<ThreadSource>("\"user\"").expect("known source"),
            ThreadSource::User
        );
        assert!(
            serde_json::from_str::<ThreadSource>("\"background-review\"").is_err()
        );
    }

    #[test]
    fn archived_native_thread_appends_remain_durable() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let store = Arc::new(TestHarnessStore::default());
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(temporary.path().to_path_buf()),
            store.clone(),
            Arc::new(|_| {}),
        )
        .expect("start native client");
        let thread_id = client
            .new_session(temporary.path(), "agent", None, None)
            .expect("create native thread");
        client.shutdown();
        store
            .set_native_thread_archived(&thread_id, true)
            .expect("archive native thread");

        let parsed = ThreadId::from_string(&thread_id).expect("valid thread id");
        let fresh = TursoThreadStore::new(store.clone());
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create test runtime");
        let item = turn_complete_item("resumed-turn");
        runtime
            .block_on(fresh.append_items(
                codex_thread_store::AppendThreadItemsParams {
                    thread_id: parsed,
                    items: vec![item.clone()],
                },
            ))
            .expect("append resumed turn to archived thread");

        let snapshot = store
            .load_native_thread(&thread_id)
            .expect("read durable thread")
            .expect("thread remains durable");
        assert!(snapshot.archived);
        assert_eq!(
            snapshot.rollout_items,
            vec![serde_json::to_value(item).expect("encode item")]
        );
    }

    #[test]
    fn managed_thread_appends_durable_history_without_rollout_files() {
        let workspace = tempfile::tempdir().expect("create workspace");
        let store = Arc::new(TestHarnessStore::default());
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(workspace.path().to_path_buf()),
            store.clone(),
            Arc::new(|_| {}),
        )
        .expect("start native client");
        let thread_id = client
            .new_session(workspace.path(), "agent", None, None)
            .expect("create native thread");
        let thread = client
            .threads
            .lock()
            .get(&thread_id)
            .cloned()
            .expect("loaded managed thread");

        client
            .runtime
            .block_on(thread.inject_response_items(vec![
                codex_protocol::models::ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![codex_protocol::models::ContentItem::InputText {
                        text: "Turso owns managed replay".to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ]))
            .expect("append through the managed runtime");

        let snapshot = store
            .load_native_thread(&thread_id)
            .expect("read durable thread")
            .expect("managed thread remains durable");
        assert!(
            snapshot.rollout_items.iter().any(|item| {
                item.to_string().contains("Turso owns managed replay")
            })
        );
        client.shutdown();
        assert!(!client.runtime_home.join("sessions").exists());
    }

    #[test]
    fn native_history_write_failure_poisoned_until_restart() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let store = Arc::new(TestHarnessStore::default());
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(temporary.path().to_path_buf()),
            store.clone(),
            Arc::new(|_| {}),
        )
        .expect("start native client");
        let thread_id = client
            .new_session(temporary.path(), "agent", None, None)
            .expect("create native thread");
        store.fail_native_append.store(true, Ordering::Relaxed);
        let parsed = ThreadId::from_string(&thread_id).expect("valid thread id");
        client
            .runtime
            .block_on(client.thread_store.append_items(
                codex_thread_store::AppendThreadItemsParams {
                    thread_id: parsed,
                    items: vec![turn_complete_item("failed-write")],
                },
            ))
            .expect_err("injected history write must fail");
        store.fail_native_append.store(false, Ordering::Relaxed);

        let error = client
            .prompt(&thread_id, "Do not send this turn")
            .expect_err("poisoned thread must reject another turn");
        assert!(error.to_string().contains("restart AHEAD before retrying"));
        assert!(client.compact(&thread_id).is_err());

        let metadata_thread_id = client
            .new_session(temporary.path(), "agent", None, None)
            .expect("create another native thread");
        store
            .fail_native_metadata_append
            .store(true, Ordering::Relaxed);
        client
            .runtime
            .block_on(
                client.thread_store.update_thread_metadata(
                    codex_thread_store::UpdateThreadMetadataParams {
                        thread_id: ThreadId::from_string(&metadata_thread_id)
                            .expect("valid metadata thread id"),
                        patch: codex_thread_store::ThreadMetadataPatch {
                            title: Some("Updated title".to_string()),
                            ..Default::default()
                        },
                        include_archived: false,
                    },
                ),
            )
            .expect_err("injected metadata write must fail");
        store
            .fail_native_metadata_append
            .store(false, Ordering::Relaxed);
        let metadata_error = client
            .prompt(&metadata_thread_id, "Do not send this turn either")
            .expect_err("metadata failure must poison that thread");
        assert!(
            metadata_error
                .to_string()
                .contains("injected native metadata write failure")
        );
        client.shutdown();
    }

    #[test]
    fn native_turn_reports_a_failed_history_write() {
        // The model never answers; a failed history write must interrupt first.
        let unanswered_model =
            TcpListener::bind("127.0.0.1:0").expect("bind mock model");
        let address = unanswered_model.local_addr().expect("mock model address");

        let workspace = tempfile::tempdir().expect("create workspace");
        std::fs::create_dir_all(workspace.path().join(".ahead"))
            .expect("create private settings directory");
        std::fs::write(
            workspace.path().join(".ahead/settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"gpt-5.6-sol\"\n"
            ),
        )
        .expect("write local model settings");
        let store = Arc::new(TestHarnessStore::default());
        let client = Arc::new(
            NativeClient::spawn(
                &NativeClientConfig::ahead(workspace.path().to_path_buf()),
                store.clone(),
                Arc::new(|_| {}),
            )
            .expect("start native client"),
        );
        let thread_id = client
            .new_session(
                workspace.path(),
                "read-only",
                Some("gpt-5.6-sol"),
                Some("mock"),
            )
            .expect("create native thread");
        store.fail_native_append.store(true, Ordering::Relaxed);
        let (sender, receiver) = std::sync::mpsc::channel();
        let prompt_client = client.clone();
        let prompt_thread_id = thread_id.clone();
        std::thread::spawn(move || {
            drop(sender.send(prompt_client.prompt(&prompt_thread_id, "Reply PONG")));
        });
        let result = receiver.recv_timeout(Duration::from_secs(10));
        if result.is_err() {
            client.cancel(&thread_id).expect("cancel timed out turn");
        }
        let error = result
            .expect("turn timed out")
            .expect_err("failed history write must fail the turn");
        assert!(error.to_string().contains("restart AHEAD before retrying"));
        assert!(
            error
                .to_string()
                .contains("injected native history write failure")
        );
        drop(unanswered_model);
        assert!(
            store
                .load_native_thread(&thread_id)
                .expect("read native history")
                .expect("thread exists")
                .rollout_items
                .is_empty()
        );
        store.fail_native_append.store(false, Ordering::Relaxed);
        assert!(client.prompt(&thread_id, "Do not retry yet").is_err());
        client.shutdown();
    }

    #[test]
    fn paginated_model_context_reads_reverse_pages_without_full_hydration() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let store = Arc::new(TestHarnessStore::default());
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(temporary.path().to_path_buf()),
            store.clone(),
            Arc::new(|_| {}),
        )
        .expect("start native client");
        let thread_id = client
            .new_session(temporary.path(), "agent", None, None)
            .expect("create native thread");
        client.shutdown();

        {
            let mut threads = store.native_threads.lock();
            let snapshot = threads
                .get_mut(&thread_id)
                .expect("native thread snapshot exists");
            let mut create_params: codex_thread_store::CreateThreadParams =
                serde_json::from_value(snapshot.create_params.clone())
                    .expect("decode native thread parameters");
            create_params.history_mode =
                codex_protocol::protocol::ThreadHistoryMode::Paginated;
            snapshot.create_params = serde_json::to_value(create_params)
                .expect("encode native thread parameters");
            snapshot.rollout_items = (0..300)
                .map(|ordinal| {
                    serde_json::to_value(codex_rollout::RolloutItem::EventMsg(
                        codex_protocol::protocol::EventMsg::TurnComplete(
                            codex_protocol::protocol::TurnCompleteEvent {
                                turn_id: ordinal.to_string(),
                                last_agent_message: None,
                                error: None,
                                started_at: None,
                                completed_at: None,
                                duration_ms: None,
                                time_to_first_token_ms: None,
                            },
                        ),
                    ))
                    .expect("encode replay item")
                })
                .collect();
        }

        let fresh = TursoThreadStore::new(store.clone());
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create test runtime");
        let context = runtime
            .block_on(
                fresh.load_latest_model_context(
                    codex_thread_store::LoadThreadHistoryParams {
                        thread_id: ThreadId::from_string(&thread_id)
                            .expect("valid native thread id"),
                        include_archived: false,
                    },
                ),
            )
            .expect("load paginated model context");
        assert_eq!(context.items.len(), 300);
        for (ordinal, item) in context.items.iter().enumerate() {
            let codex_rollout::RolloutItem::EventMsg(
                codex_protocol::protocol::EventMsg::TurnComplete(event),
            ) = item
            else {
                panic!("expected completed-turn replay item");
            };
            assert_eq!(event.turn_id, ordinal.to_string());
        }
        assert_eq!(store.full_thread_loads.load(Ordering::Relaxed), 0);
        assert_eq!(store.replay_page_loads.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn paginated_model_context_stops_at_the_latest_safe_checkpoint() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let store = Arc::new(TestHarnessStore::default());
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(temporary.path().to_path_buf()),
            store.clone(),
            Arc::new(|_| {}),
        )
        .expect("start native client");
        let thread_id = client
            .new_session(temporary.path(), "agent", None, None)
            .expect("create native thread");
        client.shutdown();

        {
            let mut threads = store.native_threads.lock();
            let snapshot = threads
                .get_mut(&thread_id)
                .expect("native thread snapshot exists");
            let mut create_params: codex_thread_store::CreateThreadParams =
                serde_json::from_value(snapshot.create_params.clone())
                    .expect("decode native thread parameters");
            create_params.history_mode =
                codex_protocol::protocol::ThreadHistoryMode::Paginated;
            snapshot.create_params = serde_json::to_value(create_params)
                .expect("encode native thread parameters");

            let thread_id =
                ThreadId::from_string(&thread_id).expect("valid thread id");
            let mut items = (0..100)
                .map(|ordinal| turn_complete_item(&format!("old-{ordinal}")))
                .collect::<Vec<_>>();
            items.push(codex_rollout::RolloutItem::Compacted(
                codex_rollout::CompactedItem {
                    message: "checkpoint".to_string(),
                    replacement_history: Some(Vec::new()),
                    window_number: Some(1),
                    first_window_id: None,
                    previous_window_id: None,
                    window_id: None,
                },
            ));
            for ordinal in 0..70 {
                let turn_id = format!("prior-{ordinal}");
                items.push(turn_started_item(&turn_id));
                items.push(turn_complete_item(&turn_id));
            }
            let turn_id = "current";
            items.push(turn_started_item(turn_id));
            items.push(codex_rollout::RolloutItem::EventMsg(
                codex_protocol::protocol::EventMsg::ItemCompleted(
                    codex_protocol::protocol::ItemCompletedEvent {
                        thread_id,
                        turn_id: turn_id.to_string(),
                        item: codex_protocol::items::TurnItem::UserMessage(
                            codex_protocol::items::UserMessageItem {
                                id: "current-user".to_string(),
                                client_id: None,
                                content: vec![
                                    codex_protocol::user_input::UserInput::Text {
                                        text: "continue".to_string(),
                                        text_elements: Vec::new(),
                                    },
                                ],
                            },
                        ),
                        started_at_ms: Some(0),
                        completed_at_ms: 0,
                    },
                ),
            ));
            items.push(codex_rollout::RolloutItem::TurnContext(
                codex_protocol::protocol::TurnContextItem {
                    turn_id: Some(turn_id.to_string()),
                    cwd: serde_json::from_value(
                        serde_json::to_value(temporary.path()).expect("serialize workspace path"),
                    )
                    .expect("absolute workspace path"),
                    workspace_roots: None,
                    current_date: None,
                    timezone: None,
                    approval_policy: codex_protocol::protocol::AskForApproval::Never,
                    sandbox_policy:
                        codex_protocol::protocol::SandboxPolicy::new_read_only_policy(),
                    permission_profile: None,
                    active_permission_profile: None,
                    file_system_sandbox_policy: None,
                    model: "test-model".to_string(),
                    comp_hash: None,
                    personality: None,
                    collaboration_mode: None,
                    multi_agent_version: None,
                    realtime_active: None,
                    effort: None,
                    summary: codex_protocol::config_types::ReasoningSummary::Auto,
                },
            ));
            items.push(turn_complete_item(turn_id));
            snapshot.rollout_items = items
                .into_iter()
                .map(|item| serde_json::to_value(item).expect("encode replay item"))
                .collect();
        }

        let fresh = TursoThreadStore::new(store.clone());
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create test runtime");
        let context = runtime
            .block_on(
                fresh.load_latest_model_context(
                    codex_thread_store::LoadThreadHistoryParams {
                        thread_id: ThreadId::from_string(&thread_id)
                            .expect("valid native thread id"),
                        include_archived: false,
                    },
                ),
            )
            .expect("load bounded paginated model context");

        assert_eq!(store.full_thread_loads.load(Ordering::Relaxed), 0);
        assert_eq!(store.replay_page_loads.load(Ordering::Relaxed), 2);
        assert_eq!(context.items.len(), 146);
        assert!(matches!(
            context.items.first(),
            Some(codex_rollout::RolloutItem::SessionMeta(meta))
                if meta.meta.history_mode == codex_protocol::protocol::ThreadHistoryMode::Paginated
        ));
        assert!(matches!(
            context.items.get(1),
            Some(codex_rollout::RolloutItem::Compacted(item)) if item.message == "checkpoint"
        ));
        assert!(!context.items.iter().any(|item| {
            matches!(item, codex_rollout::RolloutItem::EventMsg(
                codex_protocol::protocol::EventMsg::TurnComplete(event)
            ) if event.turn_id == "old-99")
        }));
    }

    #[test]
    fn native_thread_listing_filters_sorts_and_pages_without_history() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let store = Arc::new(TestHarnessStore::default());
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(temporary.path().to_path_buf()),
            store.clone(),
            Arc::new(|_| {}),
        )
        .expect("start native client");
        let ids = (0..3)
            .map(|_| {
                client
                    .new_session(temporary.path(), "agent", None, None)
                    .expect("create native thread")
            })
            .collect::<Vec<_>>();
        client.shutdown();
        for (index, id) in ids.iter().enumerate() {
            store
                .append_native_thread_metadata(
                    id,
                    json!({
                        "created_at": format!("2025-01-0{}T00:00:00Z", index + 1),
                        "updated_at": format!("2025-01-0{}T00:00:00Z", index + 1),
                    }),
                )
                .expect("persist thread timestamps");
        }
        store
            .append_native_thread_metadata(&ids[1], json!({"title": "needle title"}))
            .expect("persist searchable title");

        let fresh = TursoThreadStore::new(store.clone());
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create test runtime");
        let params = codex_thread_store::ListThreadsParams {
            page_size: 2,
            cursor: None,
            sort_key: codex_thread_store::ThreadSortKey::UpdatedAt,
            sort_direction: codex_thread_store::SortDirection::Desc,
            allowed_sources: Vec::new(),
            model_providers: None,
            cwd_filters: None,
            section: None,
            project_id: None,
            archived: false,
            search_term: None,
            relation_filter: None,
        };
        let first = runtime
            .block_on(fresh.list_threads(params.clone()))
            .expect("list newest threads");
        assert_eq!(
            first
                .items
                .iter()
                .map(|thread| thread.thread_id.to_string())
                .collect::<Vec<_>>(),
            vec![ids[2].clone(), ids[1].clone()]
        );
        let second = runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                cursor: first.next_cursor,
                ..params.clone()
            }))
            .expect("continue thread listing");
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].thread_id.to_string(), ids[0]);
        assert!(second.next_cursor.is_none());
        let oldest_first = runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                sort_direction: codex_thread_store::SortDirection::Asc,
                ..params.clone()
            }))
            .expect("list oldest threads first");
        assert_eq!(oldest_first.items[0].thread_id.to_string(), ids[0]);
        let correct_workspace = runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                page_size: 3,
                cwd_filters: Some(vec![temporary.path().to_path_buf()]),
                allowed_sources: vec![oldest_first.items[0].source.clone()],
                project_id: Some(None),
                ..params.clone()
            }))
            .expect("filter by the actual workspace and source");
        assert_eq!(correct_workspace.items.len(), 3);

        let title_match = runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                search_term: Some("needle".to_string()),
                ..params.clone()
            }))
            .expect("search persisted title");
        assert_eq!(title_match.items.len(), 1);
        assert_eq!(title_match.items[0].thread_id.to_string(), ids[1]);
        let wrong_workspace = runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                cwd_filters: Some(vec![temporary.path().join("other")]),
                ..params.clone()
            }))
            .expect("filter by workspace path");
        assert!(wrong_workspace.items.is_empty());
        let wrong_provider = runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                model_providers: Some(vec!["missing".to_string()]),
                ..params.clone()
            }))
            .expect("filter by provider");
        assert!(wrong_provider.items.is_empty());
        let wrong_project = runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                project_id: Some(Some("missing".to_string())),
                ..params.clone()
            }))
            .expect("filter by project");
        assert!(wrong_project.items.is_empty());
        runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                cursor: Some("broken".to_string()),
                ..params.clone()
            }))
            .expect_err("invalid cursor must fail");
        runtime
            .block_on(fresh.list_threads(codex_thread_store::ListThreadsParams {
                page_size: 0,
                ..params
            }))
            .expect_err("zero-size pages must fail");
        assert_eq!(store.full_thread_loads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn native_child_restore_hides_spawn_tool_after_restart() {
        let temporary = tempfile::tempdir().expect("create workspace");
        let store = Arc::new(TestHarnessStore::default());
        let config = NativeClientConfig::ahead(temporary.path().to_path_buf());
        let client = NativeClient::spawn(&config, store.clone(), Arc::new(|_| {}))
            .expect("start native client");
        let parent_id = client
            .new_session(temporary.path(), "read-only", None, None)
            .expect("create parent");
        let parent_thread_id =
            ThreadId::from_string(&parent_id).expect("parse parent ID");
        let child_id = client.runtime.block_on(async {
            let runtime_config = NativeClient::build_config(
                &client.runtime_home,
                temporary.path(),
                None,
                None,
                McpServerPolicy::Disabled,
            )
            .await
            .expect("build child configuration");
            client
                .manager
                .spawn_subagent_session(
                    parent_thread_id,
                    StartThreadOptions {
                        session_source: Some(SessionSource::SubAgent(
                            SubAgentSource::ThreadSpawn {
                                parent_thread_id,
                                depth: 1,
                                agent_path: None,
                                agent_nickname: None,
                            },
                        )),
                        dynamic_tools: ahead_dynamic_tools(false),
                        ..StartThreadOptions::new(runtime_config)
                    },
                )
                .await
                .expect("create child")
                .thread_id
        });
        let check_child_tools = |client: &NativeClient| {
            let (is_subagent, tools) = client
                .dynamic_tools_for_resume(child_id)
                .expect("read child metadata");
            assert!(is_subagent);
            for expected in [
                AHEAD_FILE_SEARCH_TOOL,
                AHEAD_SKILL_RESOURCE_READ_TOOL,
                AHEAD_READ_EDITOR_BUFFER_TOOL,
                AHEAD_PRESENT_CODE_TOOL,
                AHEAD_MOVE_CODE_POINTER_TOOL,
                AHEAD_CLEAR_PRESENTATION_TOOL,
                AHEAD_SPEAK_TEXT_TOOL,
                AHEAD_STOP_SPEAKING_TOOL,
            ] {
                assert!(
                    tools.iter().any(|tool| matches!(
                        tool,
                        DynamicToolSpec::Function(spec) if spec.name == expected
                    )),
                    "child thread is missing AHEAD tool {expected}"
                );
            }
            assert!(!tools.iter().any(|tool| {
                matches!(tool, DynamicToolSpec::Function(spec) if spec.name == AHEAD_SPAWN_AGENT_TOOL)
            }));
        };
        check_child_tools(&client);
        client.shutdown();
        drop(client);

        let reopened = NativeClient::spawn(&config, store.clone(), Arc::new(|_| {}))
            .expect("restart native client");
        reopened
            .load_session(
                &child_id.to_string(),
                temporary.path(),
                "read-only",
                None,
                None,
            )
            .expect("resume persisted child");
        check_child_tools(&reopened);
        let snapshot = store
            .load_native_thread(&child_id.to_string())
            .expect("read child")
            .expect("child remains durable");
        assert_eq!(
            snapshot.create_params["parent_thread_id"],
            json!(parent_thread_id)
        );
        assert_eq!(
            store
                .list_native_agent_children(&parent_id, None)
                .expect("read child edge"),
            vec![child_id.to_string()],
        );
        reopened.shutdown();
    }

    #[test]
    fn file_changes_preserve_update_diff_and_synthesize_add_ranges() {
        let changes = HashMap::from([
            (
                PathBuf::from("src/lib.rs"),
                FileChange::Update {
                    unified_diff: "@@ -1 +1 @@\n-old\n+new".to_string(),
                    move_path: None,
                },
            ),
            (
                PathBuf::from("README.md"),
                FileChange::Add {
                    content: "one\ntwo".to_string(),
                },
            ),
            (
                PathBuf::from("old.rs"),
                FileChange::Update {
                    unified_diff: "@@ -1 +1 @@\n-old\n+new".to_string(),
                    move_path: Some(PathBuf::from("new.rs")),
                },
            ),
        ]);
        let changes = file_changes(changes);
        assert!(changes.iter().any(|change| {
            change.path == "src/lib.rs" && change.diff.contains("-old")
        }));
        assert!(changes.iter().any(|change| {
            change.path == "README.md" && change.diff.starts_with("@@ -0,0 +1,2 @@")
        }));
        assert!(changes.iter().any(|change| change.path == "new.rs"));
    }

    #[test]
    fn patch_previews_are_bounded_without_splitting_utf8() {
        let preview = bounded_patch_preview(
            "call-1".into(),
            (0..9)
                .map(|index| {
                    (
                        PathBuf::from(format!("{}-{index}.rs", "é".repeat(300))),
                        FileChange::Add {
                            content: "界".repeat(2_100),
                        },
                    )
                })
                .collect(),
        );

        assert_eq!(preview.call_id, "call-1");
        assert_eq!(preview.changes.len(), 8);
        assert!(preview.truncated);
        assert!(preview.changes.iter().all(|change| {
            change.path.chars().count() <= 256
                && change.diff.chars().count() <= 2_000
        }));
    }

    #[test]
    fn native_agent_cancel_interrupts_an_active_child_before_the_parent() {
        std::thread::Builder::new()
            .name("ahead-agent-child-cancel-test".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| stop_native_agent_with_active_child(false))
            .expect("spawn cancellation test thread")
            .join()
            .expect("cancellation test thread panicked");
    }

    #[test]
    fn native_shutdown_waits_for_parent_and_child_termination() {
        std::thread::Builder::new()
            .name("ahead-agent-child-shutdown-test".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| stop_native_agent_with_active_child(true))
            .expect("spawn shutdown test thread")
            .join()
            .expect("shutdown test thread panicked");
    }

    #[test]
    fn native_shutdown_clears_editor_waiters_and_rejects_new_requests() {
        let workspace = tempfile::tempdir().unwrap();
        let client = NativeClient::spawn(
            &NativeClientConfig::ahead(workspace.path().to_path_buf()),
            Arc::new(TestHarnessStore::default()),
            Arc::new(|_| panic!("stopped runtime must not request editor context")),
        )
        .unwrap();
        let (input_sender, mut input_receiver) = oneshot::channel();
        client
            .pending_inputs
            .lock()
            .entry("thread".into())
            .or_default()
            .insert(
                "input".into(),
                PendingUserInput {
                    sender: input_sender,
                    kind: PendingUserInputKind::AgentQuestion,
                },
            );
        let (buffer_sender, mut buffer_receiver) = oneshot::channel();
        client
            .pending_buffer_snapshots
            .lock()
            .insert(("thread".into(), "buffer".into()), buffer_sender);
        let (presentation_sender, mut presentation_receiver) = oneshot::channel();
        client.pending_editor_presentations.lock().insert(
            ("thread".into(), "presentation".into()),
            presentation_sender,
        );

        client.shutdown();

        assert!(matches!(
            input_receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
        assert!(matches!(
            buffer_receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
        assert!(matches!(
            presentation_receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
        assert!(
            client
                .runtime
                .block_on(client.current_buffer_snapshots("thread", false))
                .is_err()
        );
        assert!(client.pending_buffer_snapshots.lock().is_empty());
        client.shutdown();
    }

    fn stop_native_agent_with_active_child(shutdown: bool) {
        let workspace = tempfile::tempdir().expect("create workspace");
        std::fs::create_dir_all(workspace.path().join(".ahead"))
            .expect("create AHEAD settings directory");
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("bind local mock model");
        let address = listener.local_addr().expect("read mock model address");
        std::fs::write(
            workspace.path().join(".ahead/settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"ahead-test\"\n"
            ),
        )
        .expect("write mock model settings");

        let (child_active_sender, child_active_receiver) =
            std::sync::mpsc::channel();
        let (release_child_sender, release_child_receiver) =
            std::sync::mpsc::channel();
        let (cancel_started_sender, cancel_started_receiver) =
            std::sync::mpsc::channel();
        let requests_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_requests_seen = requests_seen.clone();
        let connections_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_connections_seen = connections_seen.clone();
        let bytes_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let model_bytes_seen = bytes_seen.clone();
        let model_server = std::thread::spawn(move || {
            let write_response =
                |stream: &mut TcpStream, events: Vec<(&str, serde_json::Value)>| {
                    let body = events
                        .into_iter()
                        .map(|(event, payload)| {
                            format!("event: {event}\ndata: {payload}\n\n")
                        })
                        .collect::<String>();
                    write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .map(|_| ())
                };

            let (mut parent_stream, parent_request, _, _) =
                accept_model_post(&listener);
            model_connections_seen.fetch_add(1, Ordering::SeqCst);
            model_bytes_seen.fetch_add(parent_request.len(), Ordering::SeqCst);
            model_requests_seen.fetch_add(1, Ordering::SeqCst);
            let call_id = "call-ahead-cancel-child";
            let item_id = "function-ahead-cancel-child";
            let arguments = serde_json::json!({
                "label": "Cancelable child",
                "message": "Wait for the model response"
            })
            .to_string();
            write_response(
                &mut parent_stream,
                vec![
                    (
                        "response.output_item.added",
                        serde_json::json!({
                            "type": "response.output_item.added",
                            "output_index": 0,
                            "item": {
                                "id": item_id,
                                "type": "function_call",
                                "call_id": call_id,
                                "name": "spawn_agent",
                                "arguments": ""
                            }
                        }),
                    ),
                    (
                        "response.output_item.done",
                        serde_json::json!({
                            "type": "response.output_item.done",
                            "output_index": 0,
                            "item": {
                                "id": item_id,
                                "type": "function_call",
                                "call_id": call_id,
                                "name": "spawn_agent",
                                "arguments": arguments
                            }
                        }),
                    ),
                    (
                        "response.completed",
                        serde_json::json!({
                            "type": "response.completed",
                            "response": {
                                "id": "resp-ahead-cancel-parent",
                                "end_turn": false
                            }
                        }),
                    ),
                ],
            )
            .expect("write parent model response");
            drop(parent_stream);

            let (child_stream, child_request, _, _) = accept_model_post(&listener);
            model_connections_seen.fetch_add(1, Ordering::SeqCst);
            model_bytes_seen.fetch_add(child_request.len(), Ordering::SeqCst);
            model_requests_seen.fetch_add(1, Ordering::SeqCst);
            child_active_sender
                .send(())
                .expect("notify that child model request is active");
            release_child_receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("release active child model request");
            drop(child_stream);
        });

        let events = Arc::new(Mutex::new(Vec::new()));
        let observed_events = events.clone();
        let store = Arc::new(TestHarnessStore::default());
        let client = Arc::new(
            NativeClient::spawn(
                &NativeClientConfig::ahead(workspace.path().to_path_buf()),
                store.clone(),
                Arc::new(move |event| observed_events.lock().push(event)),
            )
            .expect("spawn native client"),
        );
        let parent_thread_id = client
            .new_session(
                workspace.path(),
                "read-only",
                Some("ahead-test"),
                Some("mock"),
            )
            .expect("create parent thread");
        client
            .set_scope(&parent_thread_id, workspace.path(), &[], true)
            .expect("set parent scope");
        let (prompt_sender, prompt_receiver) = std::sync::mpsc::channel();
        let prompt_client = client.clone();
        let prompt_parent_thread_id = parent_thread_id.clone();
        let prompt_thread = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let result = prompt_client.prompt(
                    &prompt_parent_thread_id,
                    "Start a child and wait for its response",
                );
                let _ = prompt_sender.send(result);
            })
            .expect("spawn parent prompt");

        if let Err(error) =
            child_active_receiver.recv_timeout(Duration::from_secs(10))
        {
            panic!(
                "child model request did not start: {error}; connections={}; request_bytes={}; complete_requests={}; prompt={:?}; events={:?}",
                connections_seen.load(Ordering::SeqCst),
                bytes_seen.load(Ordering::SeqCst),
                requests_seen.load(Ordering::SeqCst),
                prompt_receiver.try_recv(),
                events.lock()
            );
        }
        let child_thread_id = store
            .list_native_agent_children(
                &parent_thread_id,
                Some(crate::NativeAgentEdgeStatus::Open),
            )
            .expect("list active child threads")
            .into_iter()
            .next()
            .expect("active child edge was persisted");
        let cancel_client = client.clone();
        let cancel_parent_thread_id = parent_thread_id.clone();
        let (cancel_result_sender, cancel_result_receiver) =
            std::sync::mpsc::channel();
        let cancel_thread = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                cancel_started_sender
                    .send(())
                    .expect("notify cancellation start");
                let result = if shutdown {
                    cancel_client.shutdown();
                    Ok(())
                } else {
                    cancel_client.cancel(&cancel_parent_thread_id)
                };
                cancel_result_sender
                    .send(result)
                    .expect("send parent cancellation result");
            })
            .expect("spawn cancellation request");
        cancel_started_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("wait for cancellation request to start");
        std::thread::sleep(Duration::from_millis(100));
        release_child_sender
            .send(())
            .expect("release child model response");
        cancel_result_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("wait for parent cancellation")
            .expect("cancel parent and active child");
        cancel_thread.join().expect("join cancellation thread");

        let result = prompt_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("wait for cancelled parent prompt");
        let error = result.expect_err("cancelled parent prompt must fail");
        if !shutdown {
            assert!(
                error.to_string().contains("turn aborted"),
                "expected an interrupt error, got: {error:#}"
            );
        }
        prompt_thread.join().expect("join parent prompt thread");
        model_server.join().expect("join mock model server");
        assert_eq!(
            store
                .list_native_agent_children(
                    &parent_thread_id,
                    Some(crate::NativeAgentEdgeStatus::Closed),
                )
                .expect("list closed child threads"),
            [child_thread_id]
        );
        client.shutdown();
        assert!(!client.is_running());
        assert!(
            client
                .runtime
                .block_on(client.manager.list_thread_ids())
                .is_empty()
        );
        assert!(
            client
                .new_session(workspace.path(), "read-only", None, None)
                .is_err()
        );
        assert!(
            client
                .load_session(
                    &parent_thread_id,
                    workspace.path(),
                    "read-only",
                    None,
                    None
                )
                .is_err()
        );
        assert!(client.prompt(&parent_thread_id, "too late").is_err());
        assert!(client.compact(&parent_thread_id).is_err());
    }

    #[test]
    fn native_agent_advertises_and_dispatches_ahead_dynamic_tools() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock model");
        let address = listener.local_addr().expect("mock model address");
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for turn in 0..4 {
                let (mut stream, request, header_end, content_length) =
                    accept_model_post(&listener);
                let model_request = serde_json::from_slice::<Value>(
                    &request[header_end..header_end + content_length],
                )
                .expect("model request JSON");
                let events = if turn == 0 {
                    let resource_access = model_request["input"]
                        .as_array()
                        .expect("model input messages")
                        .iter()
                        .filter_map(|item| item["content"].as_array())
                        .flatten()
                        .filter_map(|content| content["text"].as_str())
                        .find_map(|text| {
                            let metadata = text
                                .split_once("<resource_access>")?
                                .1
                                .split_once("</resource_access>")?
                                .0;
                            serde_json::from_str::<Value>(metadata).ok()
                        })
                        .expect("selected skill resource locator");
                    let package = resource_access["package"]
                        .as_str()
                        .expect("opaque skill package handle");
                    let unselected_arguments = json!({
                        "package": "host-skill:project:not-selected:00000000"
                    })
                    .to_string();
                    let arguments = json!({
                        "package": package,
                        "resource": format!("{package}/references/guide.md")
                    })
                    .to_string();
                    vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"unselected-skill-resource-call","type":"function_call","call_id":"unselected-skill-resource-call","name":"skill_resource_read","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"unselected-skill-resource-call","type":"function_call","call_id":"unselected-skill-resource-call","name":"skill_resource_read","arguments":unselected_arguments}}),
                        json!({"type":"response.output_item.added","output_index":1,"item":{"id":"skill-resource-call","type":"function_call","call_id":"skill-resource-call","name":"skill_resource_read","arguments":""}}),
                        json!({"type":"response.output_item.done","output_index":1,"item":{"id":"skill-resource-call","type":"function_call","call_id":"skill-resource-call","name":"skill_resource_read","arguments":arguments}}),
                        json!({"type":"response.completed","response":{"id":"resp-ahead-skill-call","end_turn":false}}),
                    ]
                } else {
                    vec![
                        json!({"type":"response.output_item.added","output_index":0,"item":{"id":"msg-ahead-test","type":"message","role":"assistant","content":[]}}),
                        json!({"type":"response.output_text.delta","item_id":"msg-ahead-test","output_index":0,"content_index":0,"delta":"PONG"}),
                        json!({"type":"response.output_item.done","output_index":0,"item":{"id":"msg-ahead-test","type":"message","role":"assistant","content":[{"type":"output_text","text":"PONG"}]}}),
                        json!({"type":"response.completed","response":{"id":"resp-ahead-test","end_turn":true}}),
                    ]
                };
                let body = events
                    .into_iter()
                    .map(|event| {
                        format!(
                            "event: {}\ndata: {event}\n\n",
                            event["type"].as_str().expect("event type")
                        )
                    })
                    .collect::<String>();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write model response");
                let authorization = String::from_utf8_lossy(&request[..header_end])
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization")
                            .then(|| value.trim().to_string())
                    });
                requests.push((authorization, model_request));
            }
            requests
        });

        let workspace = std::env::temp_dir()
            .join(format!("ahead-native-roundtrip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(workspace.join(".ahead"))
            .expect("create test workspace");
        std::fs::write(
            workspace.join(".ahead/settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"gpt-5.5\"\napi_key = \"ahead-test-key\"\n"
            ),
        )
        .expect("write test model config");
        let skill_directory = workspace.join(".agents/skills/calendar");
        let reference_directory = skill_directory.join("references");
        std::fs::create_dir_all(&reference_directory)
            .expect("create project skill references");
        std::fs::write(
            skill_directory.join("SKILL.md"),
            "---\nname: calendar\ndescription: Exercise project skill injection\n---\nAHEAD_PROJECT_SKILL_BODY_SENTINEL\n",
        )
        .expect("write project skill");
        let reference_contents = "Calendar reference for nested provenance.\n";
        std::fs::write(reference_directory.join("guide.md"), reference_contents)
            .expect("write nested skill reference");

        let events = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(TestHarnessStore::default());
        let captured = events.clone();
        let client = Arc::new(
            NativeClient::spawn(
                &NativeClientConfig::ahead(workspace.clone()),
                store.clone(),
                Arc::new(move |event| captured.lock().push(event)),
            )
            .expect("spawn native client"),
        );
        let thread_id = client
            .new_session(&workspace, "read-only", Some("gpt-5.5"), Some("mock"))
            .expect("create native session");
        client
            .set_scope(&thread_id, &workspace, &[], false)
            .expect("set native scope");
        let (prompt_sender, prompt_receiver) = std::sync::mpsc::channel();
        let prompt_client = client.clone();
        let prompt_thread_id = thread_id.clone();
        std::thread::spawn(move || {
            drop(prompt_sender.send(prompt_client.prompt_for_ahead_turn(
                &prompt_thread_id,
                "/calendar Reply PONG",
                "ahead-turn-skill-provenance",
                None,
            )));
        });
        let prompt_result =
            prompt_receiver.recv_timeout(std::time::Duration::from_secs(10));
        if prompt_result.is_err() {
            client
                .cancel(&thread_id)
                .expect("cancel timed out native turn");
        }
        prompt_result
            .expect("native turn timed out")
            .expect("run native turn");

        let skill_sources = store
            .get_turn_skill_sources("ahead-turn-skill-provenance")
            .expect("load managed skill provenance")
            .expect("managed skill provenance was persisted");
        assert_eq!(skill_sources.len(), 1);
        assert_eq!(skill_sources[0].name, "calendar");
        assert_eq!(skill_sources[0].scope, "project");
        assert_eq!(skill_sources[0].selected_by, "human");
        assert!(
            skill_sources[0]
                .skill_id
                .starts_with("host-skill:project:calendar:")
        );
        assert!(
            !skill_sources[0]
                .skill_id
                .contains(workspace.to_string_lossy().as_ref())
        );
        assert_eq!(
            skill_sources[0].source_sha256,
            format!(
                "{:x}",
                Sha256::digest(
                    std::fs::read(skill_directory.join("SKILL.md"))
                        .expect("read project skill")
                )
            )
        );
        let resource = format!("{}/references/guide.md", skill_sources[0].skill_id);
        let expected_response =
            page_skill_resource_result(&resource, reference_contents, None)
                .expect("build expected nested resource response");
        let references = store
            .get_turn_skill_references("ahead-turn-skill-provenance")
            .expect("load nested skill resource provenance")
            .expect("nested skill resource provenance was persisted");
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].skill_name, "calendar");
        assert_eq!(references[0].scope, "project");
        assert_eq!(references[0].resource_path, "references/guide.md");
        assert_eq!(references[0].loaded_by, "agent");
        assert_eq!(
            references[0].source_sha256,
            format!("{:x}", Sha256::digest(reference_contents.as_bytes()))
        );
        assert_eq!(
            references[0].response_sha256,
            format!("{:x}", Sha256::digest(expected_response.as_bytes()))
        );
        assert!(!expected_response.contains(workspace.to_string_lossy().as_ref()));

        assert!(events.lock().iter().any(|event| {
            matches!(event, HarnessEvent::AgentDelta { text, .. } if text == "PONG")
        }));
        let (compact_sender, compact_receiver) = std::sync::mpsc::channel();
        let compact_client = client.clone();
        let compact_thread_id = thread_id.clone();
        std::thread::spawn(move || {
            drop(compact_sender.send(compact_client.compact(&compact_thread_id)));
        });
        let compact_result = compact_receiver.recv_timeout(Duration::from_secs(10));
        if compact_result.is_err() {
            client
                .cancel(&thread_id)
                .expect("cancel timed out compaction");
        }
        compact_result
            .expect("native compaction timed out")
            .expect("run native compaction");
        let fallback_thread_id = client
            .new_session(
                &workspace,
                "read-only",
                Some("ahead-test-unknown-model"),
                Some("mock"),
            )
            .expect("create unknown-model session");
        let (fallback_sender, fallback_receiver) = std::sync::mpsc::channel();
        let fallback_client = client.clone();
        let fallback_prompt_thread_id = fallback_thread_id.clone();
        std::thread::spawn(move || {
            drop(fallback_sender.send(fallback_client.prompt(
                &fallback_prompt_thread_id,
                "Answer PONG using the unknown-model fallback.",
            )));
        });
        let fallback_result =
            fallback_receiver.recv_timeout(Duration::from_secs(10));
        if fallback_result.is_err() {
            client
                .cancel(&fallback_thread_id)
                .expect("cancel timed out unknown-model turn");
        }
        fallback_result
            .expect("unknown-model turn timed out")
            .expect("run unknown-model turn");
        let snapshot = store
            .load_native_thread(&thread_id)
            .expect("load persisted test thread")
            .expect("thread snapshot exists");
        let world_states = snapshot
            .rollout_items
            .iter()
            .filter(|item| item["type"] == "world_state")
            .collect::<Vec<_>>();
        assert!(
            !world_states.is_empty(),
            "native turn context should be written to the harness store"
        );
        assert!(
            snapshot
                .rollout_items
                .iter()
                .any(|item| item["type"] == "compacted"),
            "manual compaction should persist a replay checkpoint"
        );
        assert!(
            world_states.iter().all(|item| {
                item["payload"]["state"].get("apps_instructions").is_none()
            }),
            "AHEAD must not persist hosted Apps instruction state"
        );
        let requests = server.join().expect("join mock model");
        assert_eq!(requests.len(), 4);
        let (authorization, request) = &requests[0];
        assert_eq!(authorization.as_deref(), Some("Bearer ahead-test-key"));
        let input = request["input"].as_array().expect("model input messages");
        let texts = input
            .iter()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .filter_map(|content| content["text"].as_str())
            .collect::<Vec<_>>();
        assert!(
            texts
                .iter()
                .any(|text| text.contains("AHEAD_PROJECT_SKILL_BODY_SENTINEL")),
            "the selected project skill body must reach the native model request"
        );
        let skill_prompt = texts
            .iter()
            .find(|text| text.contains("AHEAD_PROJECT_SKILL_BODY_SENTINEL"))
            .expect("find injected skill prompt");
        assert_eq!(
            skill_sources[0].prompt_sha256,
            format!("{:x}", Sha256::digest(skill_prompt.as_bytes()))
        );
        assert!(
            requests[1]
                .1
                .to_string()
                .contains("Calendar reference for nested provenance."),
            "the native skill-resource result must reach the model's follow-up request"
        );
        assert!(
            requests[1]
                .1
                .to_string()
                .contains("host skill resource was not selected for this turn"),
            "the host must deny resource reads for skills not selected in this turn"
        );
        assert!(
            texts.iter().any(|text| text.contains(
                "This model guidance does not replace AHEAD's host-enforced workspace and permission boundaries."
            )),
            "the direct model request must retain AHEAD's host-boundary guidance"
        );
        assert!(
            !request.to_string().contains("Codex CLI"),
            "catalog model request must not contain Codex CLI instructions"
        );
        assert!(texts.iter().any(|text| text.contains("Reply PONG")));
        assert!(
            texts
                .iter()
                .all(|text| !text.contains("<apps_instructions>")),
            "AHEAD must not inject hosted Apps instructions"
        );
        assert_eq!(request["model"].as_str(), Some("gpt-5.5"));
        assert!(
            requests[2].1["input"]
                .as_array()
                .expect("compaction input")
                .iter()
                .any(|item| item
                    .to_string()
                    .contains("CONTEXT CHECKPOINT COMPACTION")),
            "manual compaction should make a model request with its summary prompt"
        );
        assert!(
            requests[3].1.to_string().contains(
                "You are AHEAD, an AI coding agent integrated into the AHEAD editor."
            ),
            "unknown model instructions must use AHEAD's default on the first request; model={:?}, instructions={:?}",
            requests[3].1["model"],
            requests[3].1["instructions"]
        );
        assert!(
            !requests[3].1.to_string().contains("Codex CLI"),
            "unknown model request must not contain Codex CLI instructions"
        );
        let tool_names = request["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(
                request["input"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|item| item["type"] == "additional_tools")
                    .filter_map(|item| item["tools"].as_array())
                    .flatten(),
            )
            .flat_map(|tool| {
                std::iter::once(tool)
                    .chain(tool["tools"].as_array().into_iter().flatten())
            })
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(
            !tool_names
                .iter()
                .any(|name| matches!(*name, "exec" | "wait")),
            "the direct AHEAD harness must not advertise an unavailable Code Mode host: {tool_names:?}"
        );
        assert!(
            tool_names.contains(&"skill_resource_read"),
            "the managed runtime must advertise AHEAD's selected-skill reader; names={tool_names:?}; request={request:#?}"
        );
        client.shutdown();
        drop(client);
        let reopened = NativeClient::spawn(
            &NativeClientConfig::ahead(workspace.clone()),
            store,
            Arc::new(|_| {}),
        )
        .expect("reopen native client");
        reopened
            .load_session(
                &thread_id,
                &workspace,
                "read-only",
                Some("gpt-5.5"),
                Some("mock"),
            )
            .expect("resume native thread from harness store");
        reopened.shutdown();
        drop(reopened);
        for attempt in 0..20 {
            match std::fs::remove_dir_all(&workspace) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    break;
                }
                Err(_) if attempt < 19 => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => panic!("remove test workspace: {error}"),
            }
        }
    }
}
