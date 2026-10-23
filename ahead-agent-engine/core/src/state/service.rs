use std::sync::Arc;

use crate::agent::AgentControl;
use crate::agents_md_manager::AgentsMdManager;
use crate::client::ModelClient;
use crate::current_time::TimeProvider;
use crate::elicitation::ElicitationService;
use crate::environment_selection::ThreadEnvironments;
use crate::exec_policy::ExecPolicyManager;
use crate::mcp::McpManager;
use crate::mcp_tool_exposure::McpHandlerCache;
use crate::tools::ExecutedToolCallRecorder;
use crate::tools::code_mode::CodeModeService;
use crate::tools::handlers::ToolSearchHandlerCache;
use crate::tools::sandboxing::ApprovalStore;
use crate::unified_exec::UnifiedExecProcessManager;
use ahead_agent_skills::HostSkillsService;
use ahead_model_auth::AuthManager;
use ahead_runtime_api::RuntimeData;
use codex_mcp::McpRuntime;
use codex_models_manager::manager::SharedModelsManager;
use codex_otel::SessionTelemetry;
use codex_protocol::mcp::ClientMcpExtensions;
use codex_rollout_trace::ThreadTraceContext;
use codex_thread_store::LiveThread;
#[cfg(any(feature = "codex-realtime-voice", test))]
use codex_thread_store::ThreadStore;
use tokio::runtime::Handle;
use tokio::sync::Mutex;

pub(crate) struct SessionServices {
    /// The single owner of live MCP connections for this thread.
    pub(crate) mcp_runtime: Arc<McpRuntime>,
    /// Immutable MCP handlers scoped to this thread's current binding.
    pub(crate) mcp_handler_cache: McpHandlerCache,
    pub(crate) unified_exec_manager: UnifiedExecProcessManager,
    pub(crate) elicitations: ElicitationService,
    pub(crate) rollout_thread_trace: ThreadTraceContext,
    pub(crate) user_shell: Arc<crate::shell::Shell>,
    pub(crate) show_raw_agent_reasoning: bool,
    pub(crate) exec_policy: Arc<ExecPolicyManager>,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) models_manager: SharedModelsManager,
    pub(crate) session_telemetry: SessionTelemetry,
    pub(crate) tool_approvals: Mutex<ApprovalStore>,
    pub(crate) runtime_handle: Handle,
    pub(crate) skills_service: Arc<HostSkillsService>,
    pub(crate) agents_md_manager: Arc<AgentsMdManager>,
    pub(crate) mcp_manager: Arc<McpManager>,
    pub(crate) thread_runtime_data: RuntimeData,
    /// MCP extensions fixed when this session is created.
    pub(crate) client_mcp_extensions: ClientMcpExtensions,
    /// Raw capability selections for this thread. Each model step resolves them against its
    /// current executor environments before using them.
    pub(crate) agent_control: AgentControl,
    pub(crate) live_thread: Option<LiveThread>,
    #[cfg(any(feature = "codex-realtime-voice", test))]
    pub(crate) thread_store: Arc<dyn ThreadStore>,
    pub(crate) time_provider: Arc<dyn TimeProvider>,
    /// Session-scoped model client shared across turns.
    pub(crate) model_client: ModelClient,
    pub(crate) executed_tool_calls: Option<Arc<ExecutedToolCallRecorder>>,
    pub(crate) code_mode_service: CodeModeService,
    pub(crate) tool_search_handler_cache: ToolSearchHandlerCache,
    pub(crate) turn_environments: Arc<ThreadEnvironments>,
}
