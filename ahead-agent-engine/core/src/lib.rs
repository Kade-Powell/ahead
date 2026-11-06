//! Root of the `codex-core` library.

// Prevent accidental direct writes to stdout/stderr in library code. All
// user-visible output must go through the appropriate abstraction (e.g.,
// the TUI or the tracing stack).
#![deny(clippy::print_stdout, clippy::print_stderr)]

mod apply_patch;
mod client;
mod client_common;
#[cfg(feature = "codex-realtime-voice")]
mod realtime_context;
#[cfg(feature = "codex-realtime-voice")]
mod realtime_conversation;
#[cfg(feature = "codex-realtime-voice")]
mod realtime_prompt;
mod responses_metadata;
mod responses_retry;
pub(crate) mod session;
pub use codex_protocol::turn_input::NotSubmittedReason;
pub use codex_protocol::turn_input::RecoverTurnRequest;
pub use codex_protocol::turn_input::StartIfIdleSubmission;
pub use codex_protocol::turn_input::SteerSubmission;
pub use codex_protocol::turn_input::SuspendTurnOutcome;
pub use codex_protocol::turn_input::TurnInput;
pub use codex_protocol::turn_input::TurnInputRequest;
pub use codex_protocol::turn_input::TurnInputSubmission;
pub use codex_protocol::turn_input::TurnStartOptions;
pub use responses_metadata::CodexResponsesMetadata;
mod codex_thread;
mod compact_remote;
mod compact_remote_history;
mod compact_remote_v2;
mod compact_token_budget;
pub use codex_protocol::mcp_policy::EnvironmentMcpPolicy;
pub use codex_protocol::protocol::EnvironmentConfig;
pub use codex_thread::BackgroundTerminalInfo;
pub use codex_thread::CodexThread;
pub use codex_thread::CodexThreadSettingsOverrides;
pub use codex_thread::ThreadConfigSnapshot;
pub use session::turn_context::TurnContext;
mod agent;
mod agent_communication;
mod approval;
mod command_canonicalization;
pub mod config;
pub mod context;
mod context_manager;
mod current_time;
mod elicitation;
mod environment_selection;
pub mod exec;
pub mod exec_env;
mod exec_policy;
#[cfg(test)]
mod git_info_tests;
mod image_preparation;
pub(crate) mod mcp;
mod mcp_tool_exposure;
pub use mcp::McpManager;
mod original_image_detail;
pub use codex_mcp::SandboxState;
mod mcp_tool_call;
pub(crate) mod mention_syntax;
pub(crate) mod utils;
pub use mention_syntax::TOOL_MENTION_SIGIL;
pub use utils::path_utils;
mod sandbox_tags;
pub mod sandboxing;
mod session_prefix;
mod session_startup_prewarm;
mod skills;
pub(crate) use skills::maybe_emit_implicit_skill_invocation;
pub(crate) use skills::skills_load_input_from_config;
mod stream_events_utils;
#[cfg(test)]
pub mod test_support;
mod unified_exec;
pub mod windows_sandbox;
pub use client::X_RESPONSESAPI_INCLUDE_TIMING_METRICS_HEADER;
pub use codex_protocol::config_types::ModelProviderAuthInfo;
mod event_mapping;
mod thread_manager;
pub(crate) mod web_search;
pub use thread_manager::NewThread;
pub use thread_manager::StartThreadOptions;
pub use thread_manager::ThreadManager;
pub use thread_manager::ThreadShutdownReport;
pub use thread_manager::build_models_manager;
pub use web_search::web_search_action_detail;
pub(crate) mod agents_md;
mod agents_md_manager;
pub use agents_md::DEFAULT_AGENTS_MD_FILENAME;
pub use agents_md::LoadedAgentsMd;
mod function_tool;
mod rollout_budget;
pub(crate) mod safety;
mod session_init_error;
pub mod shell;
pub(crate) mod shell_snapshot;
pub mod spawn;
mod state;
mod tasks;
mod tools;
pub(crate) mod turn_diff_tracker;
mod turn_metadata;
mod turn_timing;
mod user_shell_command;
pub mod util;

pub use client::ModelClient;
pub use client::ModelClientSession;
pub use client::X_CODEX_TURN_METADATA_HEADER;
pub use client_common::Prompt;
pub use client_common::ResponseEvent;
pub use client_common::ResponseStream;
pub use compact::content_items_to_text;
pub use current_time::SleepFuture;
pub use current_time::TimeFuture;
pub use current_time::TimeProvider;
pub use event_mapping::parse_turn_item;
pub use exec_policy::ExecPolicyError;
pub use exec_policy::format_exec_policy_error_with_source;
pub use exec_policy::load_exec_policy;
pub mod compact;
