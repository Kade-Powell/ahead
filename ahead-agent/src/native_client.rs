//! In-process AHEAD agent connection.
//!
//! This follows Zed's native-agent split: the built-in agent owns its model and
//! tool loop in-process, while the editor consumes the same normalized events
//! used for external ACP agents. ACP is not involved in this path.

use std::{
    collections::HashMap,
    collections::hash_map::DefaultHasher,
    hash::Hash,
    hash::Hasher,
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
    AgentBufferSnapshot, AgentPresentationAction, AgentSkill, AgentSkillCatalog,
    AgentSkillSource, validate_agent_buffer_snapshots,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use codex_core::{CodexThread, StartThreadOptions, ThreadManager};
use codex_exec_server::LOCAL_FS;
use codex_features::Feature;
use codex_protocol::{
    ThreadId,
    config_types::SandboxMode,
    dynamic_tools::{
        DynamicToolCallOutputContentItem, DynamicToolFunctionSpec,
        DynamicToolResponse, DynamicToolSpec,
    },
    items::{SubAgentActivityItem, TurnItem},
    plan_tool::StepStatus,
    protocol::{
        AskForApproval, EventMsg, FileChange, Op, ReviewDecision, SessionSource,
        SkillScope, SubAgentActivityKind, SubAgentSource, ThreadSettingsOverrides,
    },
    request_permissions::{
        PermissionGrantScope, RequestPermissionProfile, RequestPermissionsResponse,
    },
    request_user_input::{RequestUserInputAnswer, RequestUserInputResponse},
    turn_input::{StartIfIdleSubmission, TurnInputRequest},
    user_input::UserInput,
};
use codex_thread_store::{ReadThreadParams, ThreadStore};
use codex_utils_absolute_path::AbsolutePathBuf;
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot;

use crate::editor_tools::{
    AHEAD_CLEAR_PRESENTATION_TOOL, AHEAD_MOVE_CODE_POINTER_TOOL,
    AHEAD_PRESENT_CODE_TOOL, AHEAD_READ_EDITOR_BUFFER_TOOL, AHEAD_SPEAK_TEXT_TOOL,
    AHEAD_STOP_SPEAKING_TOOL, parse_presentation_action,
    presentation_tool_definitions,
};
use crate::instructions::EmptyUserInstructionsProvider;
use crate::{
    acp_client::{
        HarnessEvent, HarnessFileChange, HarnessPlanEntry, HarnessSink,
        HarnessUserInputOption, HarnessUserInputQuestion,
    },
    runtime_support::{
        AgentScope, McpServerPolicy, file_change_allowed, mcp_servers_for_workspace,
        path_is_allowed, permission_profile, prepare_runtime_home,
    },
    store::HarnessStore,
    turso_agent_graph_store::TursoAgentGraphStore,
    turso_thread_store::TursoThreadStore,
};

#[derive(Debug, Clone)]
pub struct NativeClientConfig {
    pub cwd: PathBuf,
    pub file_index: Option<Arc<WorkspaceFileIndex>>,
}

impl NativeClientConfig {
    pub fn ahead(cwd: PathBuf) -> Self {
        Self {
            cwd,
            file_index: None,
        }
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
}

type UserInputAnswers = HashMap<String, Vec<String>>;
type PendingUserInput = oneshot::Sender<UserInputAnswers>;
type PendingUserInputs = HashMap<String, HashMap<String, PendingUserInput>>;
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
            None,
            None,
            McpServerPolicy::Disabled,
        ))?;
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
        let codex_self_exe = std::env::current_exe()
            .context("failed to resolve the AHEAD proxy executable")?;
        #[cfg(any(test, feature = "test-support"))]
        let codex_self_exe = test_sandbox_helper_executable(codex_self_exe)?;
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
            codex_self_exe: Some(codex_self_exe),
            ..Default::default()
        };
        let mut config = codex_core::config::ConfigBuilder::default()
            .codex_home(runtime_home.to_path_buf())
            .harness_overrides(overrides)
            .loader_overrides(codex_core::config::LoaderOverrides {
                ignore_login_requirements: true,
                ignore_project_config: true,
                ignore_system_config: true,
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
                    "AHEAD native runtime cannot disable Codex product feature {:?}: {error}",
                    feature
                )
            })?;
        }
        config.memories.generate_memories = false;
        config.memories.use_memories = false;
        config.memories.dedicated_tools = false;
        let mut developer_instructions = config
            .developer_instructions
            .take()
            .into_iter()
            .collect::<Vec<_>>();
        developer_instructions.push(AHEAD_PATH_INSTRUCTION_PREFLIGHT.to_string());
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
        let Some(skill) = resolve_slash_skill(
            outcome
                .skills
                .iter()
                .filter(|skill| outcome.is_skill_enabled(skill))
                .map(|skill| {
                    (skill.name.as_str(), agent_skill_source(skill.scope), skill)
                }),
            skill_name,
        ) else {
            return Ok(plain_text());
        };
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
            return self.set_session_selection(
                thread_id,
                cwd,
                mode_id,
                model,
                model_provider,
            );
        }
        let parsed = ThreadId::from_string(thread_id).map_err(|error| {
            anyhow!("invalid AHEAD thread id `{thread_id}`: {error}")
        })?;
        let (is_subagent, dynamic_tools) = self.dynamic_tools_for_resume(parsed)?;
        let config = self.runtime.block_on(Self::build_config(
            &self.runtime_home,
            cwd,
            model,
            model_provider,
            mcp_server_policy(mode_id, false),
        ))?;
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
        self.set_session_selection(thread_id, cwd, mode_id, model, model_provider)
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

    fn set_session_selection(
        &self,
        thread_id: &str,
        cwd: &Path,
        mode_id: &str,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<()> {
        if !self.threads.lock().contains_key(thread_id) {
            bail!("AHEAD agent thread `{thread_id}` is not loaded");
        }
        self.settings.lock().insert(
            thread_id.to_string(),
            SessionSettings {
                cwd: cwd.to_path_buf(),
                mode_id: mode_id.to_string(),
                model: model.map(str::to_string),
                model_provider: model_provider.map(str::to_string),
            },
        );
        Ok(())
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
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
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
        let user_input = self.prompt_user_input(&settings, text)?;
        let scope = self.scopes.lock().get(thread_id).cloned();
        let permission_profile =
            permission_profile(&settings.mode_id, &settings.cwd, scope.as_ref())?;
        let thread_settings = ThreadSettingsOverrides {
            approval_policy: Some(AskForApproval::OnRequest),
            permission_profile: Some(permission_profile),
            model: settings.model.clone(),
            ..Default::default()
        };
        let request = TurnInputRequest::user_input(user_input)
            .with_thread_settings(thread_settings);
        self.runtime.block_on(async {
            match thread.start_turn_if_idle(request).await? {
                StartIfIdleSubmission::Started { .. } => {}
                StartIfIdleSubmission::NotSubmitted { reason } => {
                    bail!("AHEAD agent did not start the turn: {reason:?}");
                }
            }
            self.consume_turn(
                thread_id,
                thread_id,
                &thread,
                &settings.mode_id,
                false,
            )
            .await
        })
    }

    async fn consume_turn(
        &self,
        acp_session_id: &str,
        runtime_thread_id: &str,
        thread: &Arc<CodexThread>,
        mode_id: &str,
        return_agent_message: bool,
    ) -> Result<String> {
        let thread_id = acp_session_id;
        let is_child_thread = runtime_thread_id != acp_session_id;
        let mut terminal_error = None;
        let mut saw_agent_delta = false;
        let mut saw_reasoning_delta = false;
        let mut agent_message = String::new();
        let mut agent_message_truncated = false;
        loop {
            let event = thread.next_event().await?;
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
                EventMsg::Warning(event) | EventMsg::GuardianWarning(event) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: thread_id.to_string(),
                            text: format!("\nWarning: {}\n", event.message),
                        });
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
                EventMsg::ModelReroute(event) => {
                    if !is_child_thread {
                        (self.sink)(HarnessEvent::AgentThought {
                            acp_session_id: thread_id.to_string(),
                            text: format!(
                                "\nModel rerouted from {} to {} ({:?}).\n",
                                event.from_model, event.to_model, event.reason
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
                    let is_blocking = event.is_blocking;
                    let (sender, receiver) = oneshot::channel();
                    {
                        let mut pending = self.pending_inputs.lock();
                        anyhow::ensure!(
                            self.is_running(),
                            "AHEAD agent is shutting down"
                        );
                        pending
                            .entry(thread_id.to_string())
                            .or_default()
                            .insert(call_id.clone(), sender);
                    }
                    (self.sink)(HarnessEvent::UserInputRequested {
                        acp_session_id: thread_id.to_string(),
                        request_id: call_id.clone(),
                        is_blocking,
                        questions: event
                            .questions
                            .into_iter()
                            .map(|question| HarnessUserInputQuestion {
                                id: question.id,
                                header: question.header,
                                question: question.question,
                                options: question
                                    .options
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|option| HarnessUserInputOption {
                                        label: option.label,
                                        description: option.description,
                                    })
                                    .collect(),
                                allows_other: question.is_other,
                                is_secret: question.is_secret,
                            })
                            .collect(),
                    });
                    let Ok(answers) = receiver.await else {
                        continue;
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
                    let result = match tool.as_str() {
                        AHEAD_READ_EDITOR_BUFFER_TOOL => Some(
                            self.read_editor_buffer(
                                runtime_thread_id,
                                thread_id,
                                event.arguments,
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
                                    event.arguments,
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
                                self.read_skill_resource(&settings, event.arguments)
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
                                event.arguments,
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
                            self.handle_spawn_agent_tool(
                                runtime_thread_id,
                                thread_id,
                                mode_id,
                                call_id.clone(),
                                event.arguments,
                            )
                            .await,
                        ),
                        _ => None,
                    };
                    let success =
                        result.as_ref().is_some_and(|result| result.is_ok());
                    let response_text = match result {
                        Some(Ok(text)) => text,
                        Some(Err(error)) => error.to_string(),
                        None => {
                            "AHEAD has no registered dynamic client tool for this request"
                                .to_string()
                        }
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
                }
                EventMsg::RequestPermissions(event) => {
                    thread
                        .submit(Op::RequestPermissionsResponse {
                            id: event.call_id,
                            response: RequestPermissionsResponse {
                                permissions: RequestPermissionProfile::default(),
                                scope: PermissionGrantScope::Turn,
                                strict_auto_review: false,
                            },
                        })
                        .await?;
                }
                EventMsg::ElicitationRequest(event) => {
                    thread
                        .submit(Op::ResolveElicitation {
                            server_name: event.server_name,
                            request_id: event.id,
                            decision: codex_protocol::approvals::ElicitationAction::Decline,
                            content: None,
                            meta: None,
                        })
                        .await?;
                }
                EventMsg::Error(event) => {
                    terminal_error = Some(event.message);
                }
                EventMsg::TurnComplete(event) => {
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
        let result = self
            .run_subagent_turn(
                parent_thread_id,
                acp_session_id,
                mode_id,
                &label,
                arguments,
            )
            .await;
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
        label: &str,
        arguments: Value,
    ) -> Result<String> {
        let admission = self.thread_admission.read().await;
        anyhow::ensure!(self.is_running(), "AHEAD agent is shutting down");
        let message = arguments
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty() && message.len() <= 65_536)
            .context("AHEAD spawn_agent requires a message of at most 64 KiB")?;
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

        let child_session_id = match arguments.get("session_id") {
            Some(value) => {
                let child_session_id = value
                    .as_str()
                    .filter(|session_id| !session_id.trim().is_empty())
                    .context(
                        "AHEAD spawn_agent session_id must be a non-empty string",
                    )?;
                let children = self
                    .store
                    .list_native_agent_children(parent_thread_id, None)?;
                anyhow::ensure!(
                    children.iter().any(|child| child == child_session_id),
                    "AHEAD can resume only a direct child of the current thread"
                );
                let parsed_child_id = ThreadId::from_string(child_session_id)
                    .map_err(|error| {
                        anyhow!("invalid AHEAD child thread ID: {error}")
                    })?;
                let child = if let Some(child) =
                    self.threads.lock().get(child_session_id).cloned()
                {
                    child
                } else {
                    self.manager
                        .resume_thread_by_id_with_tools(
                            parsed_child_id,
                            config,
                            ahead_dynamic_tools(false),
                        )
                        .await
                        .map_err(|error| {
                            anyhow!("failed to resume AHEAD subagent: {error}")
                        })?
                        .thread
                };
                self.threads
                    .lock()
                    .insert(child_session_id.to_string(), child);
                child_session_id.to_string()
            }
            None => {
                let options = StartThreadOptions {
                    session_source: Some(SessionSource::SubAgent(
                        SubAgentSource::ThreadSpawn {
                            parent_thread_id: parent_id,
                            depth: i32::from(child_depth),
                            agent_path: None,
                            agent_nickname: Some(label.to_string()),
                            agent_role: None,
                        },
                    )),
                    dynamic_tools: ahead_dynamic_tools(false),
                    ..StartThreadOptions::new(config)
                };
                let child = self
                    .manager
                    .spawn_subagent_session(parent_id, options)
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
            .context("AHEAD child thread was not registered")?;
        self.store.set_native_agent_edge_status(
            &child_session_id,
            crate::NativeAgentEdgeStatus::Open,
        )?;
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
            ))
            .await
        }
        .await;
        if let Err(error) = self.store.set_native_agent_edge_status(
            &child_session_id,
            crate::NativeAgentEdgeStatus::Closed,
        ) {
            return match turn_result {
                Ok(_) => Err(anyhow!(
                    "AHEAD subagent finished but its Turso edge could not be closed: {error}"
                )),
                Err(turn_error) => Err(anyhow!(
                    "AHEAD subagent turn failed: {turn_error}; its Turso edge also could not be closed: {error}"
                )),
            };
        }
        let output = turn_result?;
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
            self.consume_turn(thread_id, thread_id, &thread, &mode_id, false)
                .await
        })
    }

    pub fn answer_user_input(
        &self,
        thread_id: &str,
        request_id: &str,
        answers: HashMap<String, Vec<String>>,
    ) -> Result<()> {
        validate_mcp_approval_answers(&answers)?;
        let sender = self
            .pending_inputs
            .lock()
            .get_mut(thread_id)
            .and_then(|requests| requests.remove(request_id))
            .with_context(|| {
                format!(
                    "AHEAD agent input request `{request_id}` is no longer pending"
                )
            })?;
        sender.send(answers).map_err(|_| {
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
                    tracing::error!(%error, "AHEAD agent shutdown deadline elapsed")
                }
            }
        });
    }

    async fn current_buffer_snapshots(
        &self,
        thread_id: &str,
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
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .context("AHEAD read_editor_buffer requires a worktree-relative path")?;
        anyhow::ensure!(path.len() <= 512, "AHEAD editor path is too long");
        let relative = Path::new(path);
        anyhow::ensure!(
            !relative.is_absolute()
                && relative.components().all(|component| matches!(
                    component,
                    std::path::Component::Normal(_)
                )),
            "AHEAD editor path must stay inside the worktree"
        );
        anyhow::ensure!(
            !ahead_core::search::is_private_file(relative),
            "AHEAD cannot read a private editor buffer"
        );

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
        anyhow::ensure!(
            ahead_core::search::resolve_open_buffer_path(workspace, relative)
                .is_some(),
            "AHEAD editor path is not a safe file in the worktree"
        );
        if let Some(scope) = scope.as_ref() {
            anyhow::ensure!(
                path_is_allowed(path, &scope.allowed_paths, workspace),
                "AHEAD editor path is outside this session's file scope"
            );
        }

        let buffers = self.current_buffer_snapshots(acp_session_id).await?;
        let buffer = buffers
            .iter()
            .find(|buffer| Path::new(&buffer.path) == relative)
            .with_context(|| {
                format!("`{path}` is not currently open in the AHEAD editor")
            })?;
        anyhow::ensure!(
            buffer.content.len() <= 262_144,
            "AHEAD editor buffer is larger than 256 KiB; use file_search for a focused excerpt"
        );
        Ok(format!(
            "Unsaved editor buffer `{path}`:\n{}",
            buffer.content
        ))
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
        let legacy_offset = match arguments.get("offset") {
            Some(value) => usize::try_from(value.as_u64().context(
                "AHEAD file_search offset must be a non-negative integer",
            )?)
            .context("AHEAD file_search offset is too large")?,
            None => 0,
        };
        ensure!(
            cursor.is_none() || legacy_offset == 0,
            "AHEAD file_search accepts either cursor or legacy offset, not both"
        );
        let offset = cursor
            .as_ref()
            .map_or(legacy_offset, |cursor| cursor.offset);
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
        let path_filter = ahead_core::search::WorkspacePathFilter::new(
            &workspace,
            include_pattern,
            None,
        )?;
        let allowed_paths = scope.map(|scope| scope.allowed_paths.clone());
        let snapshots = self.current_buffer_snapshots(acp_session_id).await?;
        let overrides =
            open_buffer_overrides(&workspace, allowed_paths.as_deref(), snapshots);
        let query_fingerprint = file_search_query_fingerprint(
            &workspace,
            &options,
            include_pattern,
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
        let keep_running = Arc::new(AtomicBool::new(true));
        let cancellation = SearchCancellation(keep_running.clone());
        let (search_revision, results) = tokio::task::spawn_blocking(move || {
            let (search_revision, indexed_files) = match search_index.as_ref() {
                Some(index) => {
                    let (search_revision, files) = index
                        .snapshot_with_search_revision_while(|| {
                            keep_running.load(Ordering::Relaxed)
                        })
                        .map(|(_, revision, files)| (revision, files))
                        .ok_or(ahead_core::search::FileSearchError::Cancelled)?;
                    (search_revision, Some(files))
                }
                None => (0, None),
            };
            let disk_paths: Box<dyn Iterator<Item = PathBuf> + '_> =
                if let Some(files) = indexed_files.as_ref() {
                    Box::new(files.iter().cloned().filter(|path| {
                        ahead_core::search::is_agent_visible_path(&workspace, path)
                    }))
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
                || keep_running.load(Ordering::Relaxed),
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

        Ok(format_file_search_results(
            results,
            offset,
            search_revision,
            &query_fingerprint,
        ))
    }

    async fn read_skill_resource(
        &self,
        settings: &SessionSettings,
        arguments: Value,
    ) -> Result<String> {
        let package = arguments
            .get("package")
            .and_then(Value::as_str)
            .context("skill resource package must be a string")?;
        validate_skill_handle("package", package)?;
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

        let snapshot = self
            .load_skill_snapshot(
                &settings.cwd,
                settings.model.as_deref(),
                settings.model_provider.as_deref(),
                false,
            )
            .await?;
        let contents = snapshot
            .read_package_resource(package, resource)
            .await
            .map_err(|_| anyhow!("host skill resource could not be read"))?;
        page_skill_resource_result(resource.unwrap_or(package), &contents, cursor)
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
const MAX_SKILL_HANDLE_BYTES: usize = 2_048;
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
    allowed_paths: Option<&[String]>,
    overrides: &HashMap<PathBuf, String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"AHEAD file_search query v1");
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
    buffers.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
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
        let end =
            contents.ceil_char_boundary(lower.midpoint(upper).saturating_add(1));
        let candidate =
            response(end, Some(skill_resource_cursor(resource, contents, end)));
        if candidate.len() <= MAX_SKILL_RESOURCE_PAGE_BYTES {
            lower = end;
            best = Some(candidate);
        } else {
            upper = contents.floor_char_boundary(end.saturating_sub(1));
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
        Some((scope, name)) => {
            skill_name == name
                && (source.slash_prefix() == scope
                    || (scope.is_empty() && source == AgentSkillSource::User))
        }
        None => skill_name == invocation,
    }
}

fn resolve_slash_skill<'a, T>(
    skills: impl Iterator<Item = (&'a str, AgentSkillSource, &'a T)>,
    invocation: &str,
) -> Option<&'a T> {
    let mut matches = skills.filter(|(name, source, _)| {
        skill_matches_slash_invocation(name, *source, invocation)
    });
    let matched_skill = matches.next()?;
    matches.next().is_none().then_some(matched_skill.2)
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
) -> HashMap<PathBuf, String> {
    snapshots
        .into_iter()
        .filter_map(|snapshot| {
            let relative = Path::new(&snapshot.path);
            if ahead_core::search::is_private_file(relative)
                || allowed_paths.is_some_and(|allowed| {
                    !path_is_allowed(&snapshot.path, allowed, workspace)
                })
            {
                return None;
            }
            let path =
                ahead_core::search::resolve_open_buffer_path(workspace, relative)?;
            Some((path, snapshot.content))
        })
        .collect()
}

fn format_file_search_results(
    results: Vec<(PathBuf, Vec<ahead_core::search::FileSearchMatch>)>,
    offset: usize,
    search_revision: u64,
    query_fingerprint: &str,
) -> String {
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
                output.push_str(&format!("\n{}", path.display()));
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
            output.push_str(&format!(
                "\n  {location} {}",
                line_match.line_content.trim_end()
            ));
            match_index += 1;
            shown += 1;
        }
        if has_more {
            break;
        }
    }
    if shown == 0 {
        return if offset == 0 {
            "No matches found".to_string()
        } else {
            format!("No matches at offset {offset}")
        };
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
    output
}

const AHEAD_DISABLED_FEATURES: &[Feature] = &[
    Feature::Chronicle,
    Feature::CodeMode,
    Feature::CodeModeHost,
    Feature::CodeModeInterrupt,
    Feature::CodeModeOnly,
    Feature::CodeModePrewarm,
    Feature::Collab,
    Feature::EnableMcpApps,
    Feature::ExecutorCapabilityDiscovery,
    Feature::ExternalAgentMemoryImport,
    Feature::Mcp20260728,
    Feature::MemoryTool,
    Feature::MultiAgentV2,
    Feature::NetworkProxy,
    Feature::RespectSystemProxy,
    Feature::StandaloneWebSearch,
    Feature::ToolCallMcpElicitation,
    Feature::WebSearchCached,
    Feature::WebSearchRequest,
];

const AHEAD_PATH_INSTRUCTION_PREFLIGHT: &str = "For concrete task target paths, inspect applicable `AGENTS.md` files from the active workspace root through each target's parent directory (or the target directory itself when it is a directory) before choosing additional project skills or taking side-effecting actions. Read only those ancestor files, outer to inner, and apply the nearest file when instructions conflict. Do not recursively scan unrelated subtrees. If targets become clear only through exploration, keep that exploration read-only and check their instruction ancestry before editing or executing code there. This model guidance does not replace AHEAD's host-enforced workspace and permission boundaries.";

fn ahead_dynamic_tools(include_spawn_agent: bool) -> Vec<DynamicToolSpec> {
    let mut tools = vec![
        DynamicToolSpec::Function(DynamicToolFunctionSpec {
            name: AHEAD_FILE_SEARCH_TOOL.to_string(),
            description: "Search file contents and live unsaved editor buffers in the current AHEAD worktree with editor ignore and private-file rules. Use include_pattern to restrict paths with a glob such as src/**/*.rs; do not put a path in pattern. Results are paginated with 20 matches per page; continue with the exact cursor returned by the previous page. If the cursor is stale, restart without a cursor."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "include_pattern": { "type": "string", "description": "Optional glob for worktree-relative paths, e.g. src/**/*.rs, or paths prefixed by the worktree root name." },
                    "case_sensitive": { "type": "boolean", "default": false },
                    "whole_word": { "type": "boolean", "default": false },
                    "is_regex": { "type": "boolean", "default": false },
                    "cursor": { "type": "string", "minLength": 1, "maxLength": MAX_AGENT_SEARCH_CURSOR_BYTES },
                    "offset": { "type": "integer", "minimum": 0, "maximum": 5000, "default": 0, "description": "Legacy page offset; use the returned cursor for stable pagination." }
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
            defer_loading: false,
        }),
        DynamicToolSpec::Function(DynamicToolFunctionSpec {
            name: AHEAD_SKILL_RESOURCE_READ_TOOL.to_string(),
            description: "Read a selected host skill's SKILL.md or one package-relative reference. Use the opaque package handle from that skill's <resource_access>; omit resource for its SKILL.md, or pass <package>/<relative-path>. Absolute paths and paths outside the skill package are rejected. Continue with next_cursor when a page is returned.".to_string(),
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
            defer_loading: false,
        }),
    ];
    if include_spawn_agent {
        tools.push(DynamicToolSpec::Function(DynamicToolFunctionSpec {
            name: AHEAD_SPAWN_AGENT_TOOL.to_string(),
            description: "Delegate a well-scoped task to a separate AHEAD-managed agent thread. The child does not inherit the conversation; include the relevant paths, requirements and constraints in message. Use session_id to continue a previous child. AHEAD keeps the child within the same workspace, permission profile and editor context.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "label": { "type": "string", "minLength": 1, "maxLength": 80 },
                    "message": { "type": "string", "minLength": 1, "maxLength": 65536 },
                    "session_id": { "type": "string", "minLength": 1 }
                },
                "required": ["label", "message"],
                "additionalProperties": false
            }),
            defer_loading: false,
        }));
    }
    tools.extend(presentation_tool_definitions().into_iter().map(
        |(name, description, input_schema)| {
            DynamicToolSpec::Function(DynamicToolFunctionSpec {
                name,
                description,
                input_schema,
                defer_loading: false,
            })
        },
    ));
    tools
}

fn file_changes(changes: HashMap<PathBuf, FileChange>) -> Vec<HarnessFileChange> {
    changes
        .into_iter()
        .map(|(path, change)| HarnessFileChange {
            path: path.to_string_lossy().into_owned(),
            diff: match change {
                FileChange::Update { unified_diff, .. } => unified_diff,
                FileChange::Add { content } => {
                    let line_count = content.lines().count().max(1);
                    let added = content
                        .lines()
                        .map(|line| format!("+{line}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("@@ -0,0 +1,{line_count} @@\n{added}")
                }
                FileChange::Delete { content } => {
                    let line_count = content.lines().count().max(1);
                    let deleted = content
                        .lines()
                        .map(|line| format!("-{line}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("@@ -1,{line_count} +0,0 @@\n{deleted}")
                }
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    use ahead_rpc::ahead::{
        AgentRuntimeState, AgentTurnRequestDto, CodeAnchor, ConversationMessage,
        TaskIntent,
    };
    use codex_agent_graph_store::{AgentGraphStore, ThreadSpawnEdgeStatus};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicUsize;

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
        assert!(skill_matches_slash_invocation(
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

        assert_eq!(resolve_slash_skill(skills.into_iter(), "review"), None);
        assert_eq!(
            resolve_slash_skill(skills.into_iter(), "user:review"),
            Some(&user)
        );
        assert_eq!(
            resolve_slash_skill(skills.into_iter(), "project:review"),
            Some(&project)
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
        assert!(!child_tools.iter().any(|tool| {
            matches!(tool, DynamicToolSpec::Function(spec) if spec.name == AHEAD_SPAWN_AGENT_TOOL)
        }));
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
    fn managed_config_disables_unmanaged_mcp_and_includes_path_preflight() {
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
            "[mcp_servers.unmanaged]\ncommand = \"echo\"\nargs = [\"must not launch\"]\n",
        )
        .expect("write runtime-home MCP config");

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
            .expect("build managed config");

        assert!(config.mcp_servers.get().is_empty());
        let mcp_config =
            runtime.block_on(codex_core::McpManager::new().runtime_config(&config));
        assert_eq!(
            serde_json::to_value(mcp_config.client_elicitation_capability)
                .expect("serialize managed MCP elicitation capability"),
            json!({}),
            "managed MCP must not advertise URL elicitation without an AHEAD UI"
        );
        assert!(!config.features.enabled(Feature::Collab));
        assert!(!config.features.enabled(Feature::MultiAgentV2));
        let instructions =
            config.developer_instructions.as_deref().unwrap_or_default();
        assert!(instructions.contains(AHEAD_PATH_INSTRUCTION_PREFLIGHT));
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
    fn managed_config_loads_only_opted_in_mcp_with_human_review() {
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
    fn native_mcp_projection_preserves_server_owned_credentials() {
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

        for credentials in [
            json!({ "bearer_token_env_var": "AHEAD_TEST_MCP_TOKEN" }),
            json!({ "http_headers": { "Authorization": "Bearer test-only" } }),
            json!({ "env_http_headers": { "Authorization": "AHEAD_TEST_MCP_AUTH" } }),
            json!({ "oauth": { "client_id": "test-client" }, "scopes": ["read"] }),
        ] {
            let mut declaration = json!({ "url": "https://mcp.example.test" });
            declaration
                .as_object_mut()
                .expect("MCP declaration object")
                .extend(credentials.as_object().expect("credential object").clone());
            let server: codex_config::McpServerConfig =
                serde_json::from_value(declaration.clone())
                    .expect("parse explicit MCP credentials");
            config
                .mcp_servers
                .set(HashMap::from([("docs".to_string(), server.clone())]))
                .expect("set runtime test server");
            let effective = runtime.block_on(manager.effective_servers(&config));
            assert_eq!(effective["docs"].config(), &server);
            let serialized = serde_json::to_value(effective["docs"].config())
                .expect("serialize server configuration");
            for (name, value) in declaration.as_object().expect("declaration object")
            {
                assert_eq!(&serialized[name], value);
            }
            assert!(serialized.get("auth").is_none());
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
        })
        .collect();
        let overrides =
            open_buffer_overrides(&workspace, Some(&["src".to_string()]), snapshots);
        assert_eq!(overrides.len(), 1);
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
        let private_overrides = open_buffer_overrides(
            &workspace,
            None,
            vec![AgentBufferSnapshot {
                path: ".ahead/settings.toml".to_string(),
                content: "api_key = 'secret'".to_string(),
            }],
        );
        assert!(private_overrides.is_empty());
    }

    #[test]
    fn buffer_snapshot_request_round_trips_through_native_client() {
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
                .block_on(requester.current_buffer_snapshots("thread"))
        });
        let event = event_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("receive buffer snapshot request");
        let HarnessEvent::BufferSnapshotsRequested {
            acp_session_id,
            request_id,
        } = event
        else {
            panic!("expected buffer snapshot request");
        };
        assert_eq!(acp_session_id, "thread");
        assert!(
            client
                .answer_buffer_snapshots("other", &request_id, Vec::new(), None)
                .is_err()
        );
        let buffers = vec![AgentBufferSnapshot {
            path: "src/live.rs".into(),
            content: "unsaved".into(),
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
                .block_on(requester.current_buffer_snapshots("thread"))
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
        client.shutdown();
    }

    #[test]
    fn file_search_pages_matches_without_silent_truncation() {
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
        let path = PathBuf::from("src/lib.rs");
        let results = vec![(path.clone(), matches.clone())];
        let query_fingerprint = "a".repeat(64);

        let first =
            format_file_search_results(results.clone(), 0, 7, &query_fingerprint);
        assert_eq!(first.matches("  ").count(), AGENT_SEARCH_PAGE_SIZE);
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
            vec![(path, matches)],
            AGENT_SEARCH_PAGE_SIZE,
            7,
            &query_fingerprint,
        );
        assert_eq!(second.matches("  ").count(), 3);
        assert!(!second.contains("More matches available"));
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
            Some(&["src".to_string()]),
            &buffers,
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
        native_threads: Mutex<HashMap<String, crate::NativeThreadSnapshot>>,
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

        fn append_native_thread_items(
            &self,
            thread_id: &str,
            items: Vec<Value>,
        ) -> Result<()> {
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
                    network: None,
                    file_system_sandbox_policy: None,
                    model: "test-model".to_string(),
                    comp_hash: None,
                    personality: None,
                    collaboration_mode: None,
                    multi_agent_version: None,
                    multi_agent_mode: None,
                    realtime_active: None,
                    cyber_access_program: None,
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
                                agent_role: None,
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
        ]);
        let changes = file_changes(changes);
        assert!(changes.iter().any(|change| {
            change.path == "src/lib.rs" && change.diff.contains("-old")
        }));
        assert!(changes.iter().any(|change| {
            change.path == "README.md" && change.diff.starts_with("@@ -0,0 +1,2 @@")
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
            .insert("input".into(), input_sender);
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
                .block_on(client.current_buffer_snapshots("thread"))
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
        let model_server = std::thread::spawn(move || {
            let read_request = |stream: &mut TcpStream| {
                let mut request = Vec::new();
                let mut buffer = [0_u8; 8192];
                loop {
                    let read = stream.read(&mut buffer).expect("read model request");
                    assert_ne!(read, 0, "model request ended before its body");
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
                        .unwrap_or(0);
                    if request.len() >= header_end + content_length {
                        return;
                    }
                }
            };
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

            let (mut parent_stream, _) =
                listener.accept().expect("accept parent model request");
            read_request(&mut parent_stream);
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

            let (mut child_stream, _) = listener
                .accept()
                .expect("accept active child model request");
            read_request(&mut child_stream);
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
                "child model request did not start: {error}; requests={}; prompt={:?}; events={:?}",
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
    fn native_agent_streams_a_responses_api_turn_in_process() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock model");
        let address = listener.local_addr().expect("mock model address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept model request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let read = stream.read(&mut buffer).expect("read model request");
                if read == 0 {
                    break;
                }
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
                    .unwrap_or(0);
                if request.len() >= header_end + content_length {
                    break;
                }
            }
            let body = concat!(
                "event: response.output_item.added\n",
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"msg-ahead-test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
                "event: response.output_text.delta\n",
                "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg-ahead-test\",\"output_index\":0,\"content_index\":0,\"delta\":\"PONG\"}\n\n",
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"msg-ahead-test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"PONG\"}]}}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-ahead-test\",\"end_turn\":true}}\n\n"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write model response");
            let header_end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("model request headers")
                + 4;
            serde_json::from_slice::<Value>(&request[header_end..])
                .expect("model request JSON")
        });

        let workspace = std::env::temp_dir()
            .join(format!("ahead-native-roundtrip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(workspace.join(".ahead"))
            .expect("create test workspace");
        std::fs::write(
            workspace.join(".ahead/settings.toml"),
            format!(
                "[ai]\nactive_connection = \"Mock\"\n[[ai.connections]]\nname = \"Mock\"\nprovider_id = \"mock\"\nbase_url = \"http://{address}/v1\"\nmodel = \"ahead-test\"\n"
            ),
        )
        .expect("write test model config");
        let skill_directory = workspace.join(".agents/skills/calendar");
        std::fs::create_dir_all(&skill_directory).expect("create project skill");
        std::fs::write(
            skill_directory.join("SKILL.md"),
            "---\nname: calendar\ndescription: Exercise project skill injection\n---\nAHEAD_PROJECT_SKILL_BODY_SENTINEL\n",
        )
        .expect("write project skill");

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
            .new_session(&workspace, "read-only", Some("ahead-test"), Some("mock"))
            .expect("create native session");
        client
            .set_scope(&thread_id, &workspace, &[], false)
            .expect("set native scope");
        let (prompt_sender, prompt_receiver) = std::sync::mpsc::channel();
        let prompt_client = client.clone();
        let prompt_thread_id = thread_id.clone();
        std::thread::spawn(move || {
            drop(prompt_sender.send(
                prompt_client.prompt(&prompt_thread_id, "/calendar Reply PONG"),
            ));
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

        assert!(events.lock().iter().any(|event| {
            matches!(event, HarnessEvent::AgentDelta { text, .. } if text == "PONG")
        }));
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
            world_states.iter().all(|item| {
                item["payload"]["state"].get("apps_instructions").is_none()
            }),
            "AHEAD must not persist hosted Apps instruction state"
        );
        let request = server.join().expect("join mock model");
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
        assert!(texts.iter().any(|text| text.contains("Reply PONG")));
        assert!(
            texts
                .iter()
                .all(|text| !text.contains("<apps_instructions>")),
            "AHEAD must not inject hosted Apps instructions"
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
                Some("ahead-test"),
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
