use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::apply_patch;
use crate::apply_patch::convert_apply_patch_to_protocol;
use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::ApplyPatchToolOutput;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::handlers::apply_granted_turn_permissions;
use crate::tools::handlers::apply_patch_spec::create_apply_patch_tool;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolArgumentDiffConsumer;
use crate::tools::registry::ToolExecutor;
use crate::tools::runtimes::apply_patch::ApplyPatchRequest;
use crate::tools::runtimes::apply_patch::ApplyPatchRuntime;
use crate::tools::sandboxing::ToolCtx;
use codex_apply_patch::ApplyPatchAction;
use codex_apply_patch::ApplyPatchFileChange;
use codex_apply_patch::ApplyPatchFileUpdateMode;
use codex_apply_patch::Hunk;
use codex_apply_patch::StreamingPatchParser;
use codex_exec_server::ExecutorFileSystem;
use codex_features::Feature;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::models::FileSystemPermissions;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::PatchApplyUpdatedEvent;
use codex_sandboxing::policy_transforms::effective_file_system_sandbox_policy;
use codex_sandboxing::policy_transforms::merge_permission_profiles;
use codex_sandboxing::policy_transforms::normalize_additional_permissions;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;

const APPLY_PATCH_ARGUMENT_DIFF_BUFFER_INTERVAL: Duration = Duration::from_millis(500);
const MAX_FUNCTION_APPLY_PATCH_ARGUMENT_BYTES: usize = 4 * 1024 * 1024;

fn apply_patch_file_update_mode(turn: &TurnContext) -> ApplyPatchFileUpdateMode {
    if turn
        .config
        .features
        .enabled(Feature::ApplyPatchPreserveLineEndings)
    {
        ApplyPatchFileUpdateMode::PreserveLineEndings
    } else {
        ApplyPatchFileUpdateMode::NormalizeToLf
    }
}

/// Handles `apply_patch` requests in the turn workspace.
pub struct ApplyPatchHandler {
    tool_type: ApplyPatchToolType,
}

impl ApplyPatchHandler {
    pub fn new(tool_type: ApplyPatchToolType) -> Self {
        Self { tool_type }
    }
}

fn patch_input(payload: ToolPayload) -> Result<String, FunctionCallError> {
    match payload {
        ToolPayload::Custom { input } => Ok(input),
        ToolPayload::Function { arguments } => {
            let arguments: serde_json::Value =
                serde_json::from_str(&arguments).map_err(|error| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to parse apply_patch arguments: {error}"
                    ))
                })?;
            arguments
                .get("patch")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "apply_patch arguments must include a string `patch` field".to_string(),
                    )
                })
        }
        _ => Err(FunctionCallError::RespondToModel(
            "apply_patch handler received unsupported payload".to_string(),
        )),
    }
}

#[derive(Default)]
struct ApplyPatchArgumentDiffConsumer {
    parser: StreamingPatchParser,
    last_sent_at: Option<Instant>,
    pending: Option<PatchApplyUpdatedEvent>,
}

#[derive(Default)]
struct FunctionApplyPatchArgumentDiffConsumer {
    state: FunctionPatchArgumentState,
    key: String,
    patch_json_string: String,
    patch_escape_pending: bool,
    decoded_raw_bytes: usize,
    received_bytes: usize,
    preview: ApplyPatchArgumentDiffConsumer,
    enabled: bool,
    exceeded_limit: bool,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum FunctionPatchArgumentState {
    #[default]
    BeforeObject,
    BeforeKey,
    ReadingKey,
    BeforeColon,
    BeforeValue,
    ReadingPatch,
    Done,
    Invalid,
}

impl ToolArgumentDiffConsumer for FunctionApplyPatchArgumentDiffConsumer {
    fn consume_diff(
        &mut self,
        turn: &TurnContext,
        call_id: String,
        diff: &str,
    ) -> Option<EventMsg> {
        if !turn
            .config
            .features
            .enabled(Feature::ApplyPatchStreamingEvents)
            || self.exceeded_limit
            || matches!(
                self.state,
                FunctionPatchArgumentState::Done | FunctionPatchArgumentState::Invalid
            )
        {
            return None;
        }

        self.enabled = true;
        self.received_bytes = self.received_bytes.saturating_add(diff.len());
        if self.received_bytes > MAX_FUNCTION_APPLY_PATCH_ARGUMENT_BYTES {
            self.patch_json_string.clear();
            self.exceeded_limit = true;
            return None;
        }

        for character in diff.chars() {
            match self.state {
                FunctionPatchArgumentState::BeforeObject => {
                    if character.is_whitespace() {
                        continue;
                    }
                    if character == '{' {
                        self.state = FunctionPatchArgumentState::BeforeKey;
                    } else {
                        self.state = FunctionPatchArgumentState::Invalid;
                    }
                }
                FunctionPatchArgumentState::BeforeKey => {
                    if character.is_whitespace() {
                        continue;
                    }
                    if character == '"' {
                        self.key.clear();
                        self.state = FunctionPatchArgumentState::ReadingKey;
                    } else {
                        self.state = FunctionPatchArgumentState::Invalid;
                    }
                }
                FunctionPatchArgumentState::ReadingKey => {
                    if character == '"' {
                        self.state = if self.key == "patch" {
                            FunctionPatchArgumentState::BeforeColon
                        } else {
                            FunctionPatchArgumentState::Invalid
                        };
                    } else if character == '\\' || self.key.len() >= 32 {
                        self.state = FunctionPatchArgumentState::Invalid;
                    } else {
                        self.key.push(character);
                    }
                }
                FunctionPatchArgumentState::BeforeColon => {
                    if character.is_whitespace() {
                        continue;
                    }
                    if character == ':' {
                        self.state = FunctionPatchArgumentState::BeforeValue;
                    } else {
                        self.state = FunctionPatchArgumentState::Invalid;
                    }
                }
                FunctionPatchArgumentState::BeforeValue => {
                    if character.is_whitespace() {
                        continue;
                    }
                    if character == '"' {
                        self.state = FunctionPatchArgumentState::ReadingPatch;
                    } else {
                        self.state = FunctionPatchArgumentState::Invalid;
                    }
                }
                FunctionPatchArgumentState::ReadingPatch => {
                    if self.patch_escape_pending {
                        self.patch_json_string.push(character);
                        self.patch_escape_pending = false;
                    } else if character == '\\' {
                        self.patch_json_string.push(character);
                        self.patch_escape_pending = true;
                    } else if character == '"' {
                        self.state = FunctionPatchArgumentState::Done;
                    } else {
                        self.patch_json_string.push(character);
                    }
                }
                FunctionPatchArgumentState::Done | FunctionPatchArgumentState::Invalid => {}
            }
        }

        self.push_decoded_patch(call_id)
    }

    fn finish(&mut self) -> Result<Option<EventMsg>, FunctionCallError> {
        if !self.enabled || self.exceeded_limit || self.state != FunctionPatchArgumentState::Done {
            return Ok(None);
        }

        match self.preview.finish() {
            Ok(event) => Ok(event),
            Err(_) => Ok(None),
        }
    }
}

impl FunctionApplyPatchArgumentDiffConsumer {
    fn push_decoded_patch(&mut self, call_id: String) -> Option<EventMsg> {
        if !matches!(
            self.state,
            FunctionPatchArgumentState::ReadingPatch | FunctionPatchArgumentState::Done
        ) {
            return None;
        }

        let Some(raw_delta) = self.patch_json_string.get(self.decoded_raw_bytes..) else {
            self.state = FunctionPatchArgumentState::Invalid;
            return None;
        };
        if raw_delta.is_empty() {
            return None;
        }
        let wrapped = format!("\"{raw_delta}\"");
        let Ok(delta) = serde_json::from_str::<String>(&wrapped) else {
            return None;
        };

        self.decoded_raw_bytes = self.patch_json_string.len();
        self.preview
            .push_delta(call_id, &delta)
            .map(EventMsg::PatchApplyUpdated)
    }
}

impl ToolArgumentDiffConsumer for ApplyPatchArgumentDiffConsumer {
    fn consume_diff(
        &mut self,
        turn: &TurnContext,
        call_id: String,
        diff: &str,
    ) -> Option<EventMsg> {
        if !turn
            .config
            .features
            .enabled(Feature::ApplyPatchStreamingEvents)
        {
            return None;
        }

        self.push_delta(call_id, diff)
            .map(EventMsg::PatchApplyUpdated)
    }

    fn finish(&mut self) -> Result<Option<EventMsg>, FunctionCallError> {
        self.finish_update_on_complete()
            .map(|event| event.map(EventMsg::PatchApplyUpdated))
    }
}

impl ApplyPatchArgumentDiffConsumer {
    fn push_delta(&mut self, call_id: String, delta: &str) -> Option<PatchApplyUpdatedEvent> {
        let hunks = self.parser.push_delta(delta).ok()?;
        if hunks.is_empty() {
            return None;
        }
        let changes = convert_apply_patch_hunks_to_protocol(&hunks);
        let event = PatchApplyUpdatedEvent { call_id, changes };
        let now = Instant::now();
        match self.last_sent_at {
            Some(last_sent_at)
                if now.duration_since(last_sent_at) < APPLY_PATCH_ARGUMENT_DIFF_BUFFER_INTERVAL =>
            {
                self.pending = Some(event);
                None
            }
            Some(_) | None => {
                self.pending = None;
                self.last_sent_at = Some(now);
                Some(event)
            }
        }
    }

    fn finish_update_on_complete(
        &mut self,
    ) -> Result<Option<PatchApplyUpdatedEvent>, FunctionCallError> {
        self.parser.finish().map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to parse apply_patch: {err}"))
        })?;

        let event = self.pending.take();
        if event.is_some() {
            self.last_sent_at = Some(Instant::now());
        }
        Ok(event)
    }
}

fn convert_apply_patch_hunks_to_protocol(hunks: &[Hunk]) -> HashMap<PathBuf, FileChange> {
    hunks
        .iter()
        .map(|hunk| {
            let path = hunk_source_path(hunk).to_path_buf();
            let change = match hunk {
                Hunk::AddFile { contents, .. } => FileChange::Add {
                    content: contents.clone(),
                },
                Hunk::DeleteFile { .. } => FileChange::Delete {
                    content: String::new(),
                },
                Hunk::UpdateFile {
                    chunks, move_path, ..
                } => FileChange::Update {
                    unified_diff: format_update_chunks_for_progress(chunks),
                    move_path: move_path.clone(),
                },
            };
            (path, change)
        })
        .collect()
}

fn hunk_source_path(hunk: &Hunk) -> &Path {
    match hunk {
        Hunk::AddFile { path, .. } | Hunk::DeleteFile { path } | Hunk::UpdateFile { path, .. } => {
            path
        }
    }
}

fn format_update_chunks_for_progress(chunks: &[codex_apply_patch::UpdateFileChunk]) -> String {
    let mut unified_diff = String::new();
    for chunk in chunks {
        match &chunk.change_context {
            Some(context) => {
                unified_diff.push_str("@@ ");
                unified_diff.push_str(context);
                unified_diff.push('\n');
            }
            None => {
                unified_diff.push_str("@@");
                unified_diff.push('\n');
            }
        }
        for line in &chunk.old_lines {
            unified_diff.push('-');
            unified_diff.push_str(line);
            unified_diff.push('\n');
        }
        for line in &chunk.new_lines {
            unified_diff.push('+');
            unified_diff.push_str(line);
            unified_diff.push('\n');
        }
        if chunk.is_end_of_file {
            unified_diff.push_str("*** End of File");
            unified_diff.push('\n');
        }
    }
    unified_diff
}

fn file_paths_for_action(action: &ApplyPatchAction) -> Vec<PathUri> {
    let mut keys = Vec::new();
    for (path, change) in action.changes() {
        keys.push(path.clone());

        if let ApplyPatchFileChange::Update { move_path, .. } = change
            && let Some(dest) = move_path
        {
            keys.push(dest.clone());
        }
    }

    keys
}

fn write_permissions_for_paths(
    file_paths: &[AbsolutePathBuf],
    file_system_sandbox_policy: &codex_protocol::permissions::FileSystemSandboxPolicy,
    cwd: &AbsolutePathBuf,
) -> Option<AdditionalPermissionProfile> {
    let write_paths = file_paths
        .iter()
        // Skip already-writable targets before deriving parent permissions.
        // Otherwise, a writable directory could grant access to its parent.
        .filter(|path| {
            !file_system_sandbox_policy.can_write_path_with_cwd(path.as_path(), cwd.as_path())
        })
        .map(|path| {
            path.parent()
                .unwrap_or_else(|| path.clone())
                .into_path_buf()
        })
        .filter(|path| {
            !file_system_sandbox_policy.can_write_path_with_cwd(path.as_path(), cwd.as_path())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(AbsolutePathBuf::from_absolute_path)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;

    let permissions = (!write_paths.is_empty()).then_some(AdditionalPermissionProfile {
        file_system: Some(FileSystemPermissions::from_read_write_roots(
            Some(vec![]),
            Some(write_paths),
        )),
        ..Default::default()
    })?;

    normalize_additional_permissions(permissions).ok()
}

/// Extracts the raw patch text used as the command-shaped hook input for apply_patch.
async fn effective_patch_permissions(
    session: &Session,
    environment: &TurnEnvironment,
    action: &ApplyPatchAction,
    cwd: &PathUri,
) -> std::io::Result<(
    Vec<PathUri>,
    crate::tools::handlers::EffectiveAdditionalPermissions,
    codex_protocol::permissions::FileSystemSandboxPolicy,
)> {
    let environment_id = environment.selection.environment_id.as_str();
    let file_paths = file_paths_for_action(action);
    let native_cwd = cwd.to_abs_path()?;
    let granted_permissions = merge_permission_profiles(
        session
            .granted_session_permissions(environment_id)
            .await
            .as_ref(),
        session
            .granted_turn_permissions(environment_id)
            .await
            .as_ref(),
    );
    let base_file_system_sandbox_policy = environment
        .permission_profile_with_workspace_roots()
        .file_system_sandbox_policy();
    let file_system_sandbox_policy = effective_file_system_sandbox_policy(
        &base_file_system_sandbox_policy,
        granted_permissions.as_ref(),
    );
    let native_file_paths = file_paths
        .iter()
        .map(PathUri::to_abs_path)
        .collect::<Result<Vec<_>, _>>()?;
    let effective_additional_permissions = apply_granted_turn_permissions(
        session,
        environment_id,
        native_cwd.as_path(),
        crate::sandboxing::SandboxPermissions::UseDefault,
        write_permissions_for_paths(&native_file_paths, &file_system_sandbox_policy, &native_cwd),
    )
    .await;

    Ok((
        file_paths,
        effective_additional_permissions,
        file_system_sandbox_policy,
    ))
}

fn patch_permissions_without_path_matching(
    action: &ApplyPatchAction,
) -> (
    Vec<PathUri>,
    crate::tools::handlers::EffectiveAdditionalPermissions,
    codex_protocol::permissions::FileSystemSandboxPolicy,
) {
    // TODO(anp): Make permission matching operate on PathUri. Until then, foreign paths skip
    // permission matching; a managed turn still fails closed at the platform sandbox boundary.
    (
        file_paths_for_action(action),
        crate::tools::handlers::EffectiveAdditionalPermissions {
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            permissions_preapproved: false,
        },
        codex_protocol::permissions::FileSystemSandboxPolicy::unrestricted(),
    )
}

impl ToolExecutor<ToolInvocation> for ApplyPatchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("apply_patch")
    }

    fn spec(&self) -> ToolSpec {
        create_apply_patch_tool(&self.tool_type)
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl ApplyPatchHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            step_context,
            tracker,
            call_id,
            tool_name,
            payload,
            ..
        } = invocation;

        let patch_input = patch_input(payload)?;
        let args = match codex_apply_patch::parse_patch(&patch_input) {
            Ok(args) => args,
            Err(parse_error) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "apply_patch verification failed: {parse_error}"
                )));
            }
        };
        let Some(turn_environment) = resolve_tool_environment(&step_context.environments) else {
            return Err(FunctionCallError::RespondToModel(
                "apply_patch is unavailable in this session".to_string(),
            ));
        };
        let fs = turn_environment.environment.get_filesystem();
        let sandbox = turn_environment.sandbox_context(/*additional_permissions*/ None);
        match codex_apply_patch::verify_apply_patch_args_with_mode(
            args,
            turn_environment.cwd(),
            apply_patch_file_update_mode(step_context.turn.as_ref()),
            fs.as_ref(),
            Some(&sandbox),
        )
        .await
        {
            codex_apply_patch::MaybeApplyPatchVerified::Body(changes) => {
                let tool_ctx = ToolCtx {
                    session,
                    step_context: Arc::clone(&step_context),
                    call_id,
                    tool_name,
                };
                let content = execute_verified_patch(
                    changes,
                    turn_environment.cwd(),
                    turn_environment.clone(),
                    Some(&tracker),
                    tool_ctx,
                )
                .await?;
                Ok(boxed_tool_output(ApplyPatchToolOutput::from_text(content)))
            }
            codex_apply_patch::MaybeApplyPatchVerified::CorrectnessError(parse_error) => {
                Err(FunctionCallError::RespondToModel(format!(
                    "apply_patch verification failed: {parse_error}"
                )))
            }
            codex_apply_patch::MaybeApplyPatchVerified::ShellParseError(error) => {
                tracing::trace!("Failed to parse apply_patch input, {error:?}");
                Err(FunctionCallError::RespondToModel(
                    "apply_patch handler received invalid patch input".to_string(),
                ))
            }
            codex_apply_patch::MaybeApplyPatchVerified::NotApplyPatch => {
                Err(FunctionCallError::RespondToModel(
                    "apply_patch handler received non-apply_patch input".to_string(),
                ))
            }
        }
    }
}

impl CoreToolRuntime for ApplyPatchHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(
            (&self.tool_type, payload),
            (ApplyPatchToolType::Freeform, ToolPayload::Custom { .. })
                | (ApplyPatchToolType::Function, ToolPayload::Function { .. })
        )
    }

    fn create_diff_consumer(&self) -> Option<Box<dyn ToolArgumentDiffConsumer>> {
        match &self.tool_type {
            ApplyPatchToolType::Freeform => Some(Box::<ApplyPatchArgumentDiffConsumer>::default()),
            ApplyPatchToolType::Function => {
                Some(Box::<FunctionApplyPatchArgumentDiffConsumer>::default())
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn intercept_apply_patch(
    command: &[String],
    cwd: &PathUri,
    fs: &dyn ExecutorFileSystem,
    turn_environment: TurnEnvironment,
    session: Arc<Session>,
    step_context: Arc<StepContext>,
    tracker: Option<&SharedTurnDiffTracker>,
    call_id: &str,
    tool_name: &str,
) -> Result<Option<FunctionToolOutput>, FunctionCallError> {
    let turn = &step_context.turn;
    let sandbox = turn_environment.sandbox_context(/*additional_permissions*/ None);
    match codex_apply_patch::maybe_parse_apply_patch_verified_with_mode(
        command,
        cwd,
        apply_patch_file_update_mode(turn),
        fs,
        Some(&sandbox),
    )
    .await
    {
        codex_apply_patch::MaybeApplyPatchVerified::Body(changes) => {
            let tool_ctx = ToolCtx {
                session,
                step_context,
                call_id: call_id.to_string(),
                tool_name: ToolName::plain(tool_name),
            };
            let content =
                execute_verified_patch(changes, cwd, turn_environment, tracker, tool_ctx).await?;
            Ok(Some(FunctionToolOutput::from_text(content, Some(true))))
        }
        codex_apply_patch::MaybeApplyPatchVerified::CorrectnessError(parse_error) => {
            Err(FunctionCallError::RespondToModel(format!(
                "apply_patch verification failed: {parse_error}"
            )))
        }
        codex_apply_patch::MaybeApplyPatchVerified::ShellParseError(error) => {
            tracing::trace!("Failed to parse apply_patch input, {error:?}");
            Ok(None)
        }
        codex_apply_patch::MaybeApplyPatchVerified::NotApplyPatch => Ok(None),
    }
}

async fn execute_verified_patch(
    action: ApplyPatchAction,
    cwd: &PathUri,
    turn_environment: TurnEnvironment,
    tracker: Option<&SharedTurnDiffTracker>,
    tool_ctx: ToolCtx,
) -> Result<String, FunctionCallError> {
    let (file_paths, effective_additional_permissions, file_system_sandbox_policy) =
        effective_patch_permissions(tool_ctx.session.as_ref(), &turn_environment, &action, cwd)
            .await
            .unwrap_or_else(|_| patch_permissions_without_path_matching(&action));
    let apply = apply_patch::prepare_apply_patch(
        &tool_ctx.step_context,
        &turn_environment,
        &file_system_sandbox_policy,
        action,
    )?;
    let changes = convert_apply_patch_to_protocol(&apply.action);
    let emitter = ToolEmitter::apply_patch_for_environment(
        changes.clone(),
        apply.auto_approved,
        turn_environment.selection.environment_id.clone(),
    );
    let event_ctx = ToolEventCtx::new(
        tool_ctx.session.as_ref(),
        tool_ctx.step_context.turn.as_ref(),
        &tool_ctx.call_id,
        tracker,
    );
    emitter.begin(event_ctx).await;

    let request = ApplyPatchRequest {
        turn_environment,
        action: apply.action,
        file_paths,
        changes: Arc::new(changes),
        exec_approval_requirement: apply.exec_approval_requirement,
        additional_permissions: effective_additional_permissions.additional_permissions,
        permissions_preapproved: effective_additional_permissions.permissions_preapproved,
    };
    let mut orchestrator = ToolOrchestrator::new();
    let mut runtime = ApplyPatchRuntime::new();
    let result = orchestrator
        .run(&mut runtime, &request, &tool_ctx)
        .await
        .map(|result| result.output);
    let (result, delta) = match result {
        Ok(output) => (Ok(output.exec_output), Some(output.delta)),
        Err(error) => (Err(error), Some(runtime.committed_delta().clone())),
    };
    let event_ctx = ToolEventCtx::new(
        tool_ctx.session.as_ref(),
        tool_ctx.step_context.turn.as_ref(),
        &tool_ctx.call_id,
        tracker,
    );
    emitter.finish(event_ctx, result, delta.as_ref()).await
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
