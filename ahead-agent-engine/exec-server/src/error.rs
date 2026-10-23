use codex_exec_server_protocol::JSONRPCErrorError;

#[derive(Debug, thiserror::Error)]
pub enum ExecServerError {
    #[error("agent executor protocol error: {0}")]
    Protocol(String),
    #[error("agent executor rejected request ({code}): {message}")]
    Execution { code: i64, message: String },
}

pub(crate) fn invalid_request(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32600,
        data: None,
        message,
    }
}

pub(crate) fn invalid_params(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32602,
        data: None,
        message,
    }
}

pub(crate) fn not_found(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32004,
        data: None,
        message,
    }
}

pub(crate) fn internal_error(message: String) -> JSONRPCErrorError {
    JSONRPCErrorError {
        code: -32603,
        data: None,
        message,
    }
}
