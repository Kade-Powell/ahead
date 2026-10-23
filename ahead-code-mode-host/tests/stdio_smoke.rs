use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;

use codex_code_mode::CellId;
use codex_code_mode::CodeModeNestedToolCall;
use codex_code_mode::CodeModeSessionDelegate;
use codex_code_mode::CodeModeSessionProvider;
use codex_code_mode::CodeModeToolKind;
use codex_code_mode::ExecuteRequest;
use codex_code_mode::FunctionCallOutputContentItem;
use codex_code_mode::NotificationFuture;
use codex_code_mode::ProcessOwnedCodeModeSessionProvider;
use codex_code_mode::RuntimeResponse;
use codex_code_mode::ToolDefinition;
use codex_code_mode::ToolInvocationFuture;
use codex_protocol::ToolName;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct EchoDelegate {
    calls: Mutex<Vec<CodeModeNestedToolCall>>,
}

impl CodeModeSessionDelegate for EchoDelegate {
    fn invoke_tool<'a>(
        &'a self,
        call: CodeModeNestedToolCall,
        _cancellation: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
        Box::pin(async { Ok(json!({ "value": "tool ready" })) })
    }

    fn notify<'a>(
        &'a self,
        _call_id: String,
        _cell_id: CellId,
        _text: String,
        _cancellation: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell_id: &CellId) {}
}

#[tokio::test]
async fn ahead_host_executes_a_cell_over_the_managed_stdio_protocol() {
    let provider = ProcessOwnedCodeModeSessionProvider::with_host_program(
        env!("CARGO_BIN_EXE_ahead-code-mode-host").into(),
    );
    let delegate = Arc::new(EchoDelegate::default());
    let session = tokio::time::timeout(
        Duration::from_secs(10),
        provider.create_session(delegate.clone()),
    )
    .await
    .expect("host handshake timed out")
    .expect("host handshake failed");
    let started = session
        .execute(ExecuteRequest {
            tool_call_id: "ahead-stdio-smoke".to_string(),
            enabled_tools: Vec::new(),
            source: "text('host ready')".to_string(),
            yield_time_ms: None,
            max_output_tokens: None,
        })
        .await
        .expect("host rejected cell");
    let response =
        tokio::time::timeout(Duration::from_secs(10), started.initial_response())
            .await
            .expect("host cell timed out")
            .expect("host cell failed");
    match response {
        RuntimeResponse::Result {
            content_items,
            error_text,
            ..
        } => {
            assert_eq!(error_text, None);
            assert_eq!(
                content_items,
                vec![FunctionCallOutputContentItem::InputText {
                    text: "host ready".to_string(),
                }]
            );
        }
        other => panic!("unexpected host response: {other:?}"),
    }
    let started = session
        .execute(ExecuteRequest {
            tool_call_id: "ahead-tool-smoke".to_string(),
            enabled_tools: vec![ToolDefinition {
                name: "echo".to_string(),
                tool_name: ToolName::plain("echo"),
                description: String::new(),
                kind: CodeModeToolKind::Function,
                input_schema: None,
                output_schema: None,
            }],
            source:
                "const result = await tools.echo({ value: 42 }); text(result.value)"
                    .to_string(),
            yield_time_ms: None,
            max_output_tokens: None,
        })
        .await
        .expect("host rejected nested tool cell");
    let response =
        tokio::time::timeout(Duration::from_secs(10), started.initial_response())
            .await
            .expect("nested tool cell timed out")
            .expect("nested tool cell failed");
    match response {
        RuntimeResponse::Result {
            content_items,
            error_text,
            ..
        } => {
            assert_eq!(error_text, None);
            assert_eq!(
                content_items,
                vec![FunctionCallOutputContentItem::InputText {
                    text: "tool ready".to_string(),
                }]
            );
        }
        other => panic!("unexpected nested tool response: {other:?}"),
    }
    let calls = delegate
        .calls
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].input, Some(json!({ "value": 42 })));
    drop(calls);
    session.shutdown().await.expect("host session shutdown");
}
