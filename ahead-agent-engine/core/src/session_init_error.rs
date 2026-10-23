use codex_protocol::error::CodexErr;
use codex_thread_store::ThreadStoreError;

pub(crate) fn map_session_init_error(err: &anyhow::Error) -> CodexErr {
    if let Some(store_error) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<ThreadStoreError>())
    {
        match store_error {
            ThreadStoreError::Unsupported { operation } => {
                return CodexErr::UnsupportedOperation(format!("{operation} is not supported yet"));
            }
            ThreadStoreError::Conflict { message } => {
                return CodexErr::InvalidRequest(message.clone());
            }
            ThreadStoreError::ThreadNotFound { .. }
            | ThreadStoreError::InvalidRequest { .. }
            | ThreadStoreError::Internal { .. } => {}
        }
    }

    CodexErr::Fatal(format!("Failed to initialize session: {err:#}"))
}
