//! Minimal JSON-over-memory WebAssembly ABI for the AHEAD workflow engine.
//!
//! Hosts allocate request memory, dispatch a versioned JSON operation, decode the packed output
//! pointer and length, and return both allocations through [`ahead_dealloc`].

use ahead_core::{
    ApplyInput, Capability, CreateRunInput, ENGINE_API_VERSION, Run, apply, create_run,
    derive_state, tool_allowed, validate_run, workflow, workflows,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Versioned operation request received from a WebAssembly host.
#[derive(Debug, Deserialize)]
struct Request {
    /// Requested engine protocol version.
    api_version: String,
    /// Operation identifier to dispatch.
    operation: String,
    /// Operation-specific JSON input.
    #[serde(default)]
    input: Value,
}

/// JSON response envelope returned to a WebAssembly host.
#[derive(Debug, Serialize)]
struct Response<T> {
    /// Whether the operation completed successfully.
    ok: bool,
    /// Successful operation result, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    /// Structured engine error, when the operation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ahead_core::AheadError>,
}

/// Input for the `get_workflow` operation.
#[derive(Debug, Deserialize)]
struct WorkflowInput {
    /// Workflow identifier to load.
    workflow_id: String,
}

/// Input shared by operations that inspect an existing run.
#[derive(Debug, Deserialize)]
struct RunInput {
    /// Run to validate or derive.
    run: Run,
}

/// Input for the `apply_event` operation.
#[derive(Debug, Deserialize)]
struct ApplyOperationInput {
    /// Existing run to which the event will be appended.
    run: Run,
    /// Attributed event input to append.
    event: ApplyInput,
}

/// Input for the `tool_allowed` operation.
#[derive(Debug, Deserialize)]
struct ToolInput {
    /// Run whose active phase controls the capability.
    run: Run,
    /// AI capability to evaluate.
    capability: Capability,
}

/// Allocates WebAssembly memory for a host-written request.
///
/// The host must initialize all `length` bytes before passing the returned pointer to
/// [`ahead_dispatch`], then release the allocation with [`ahead_dealloc`].
#[unsafe(no_mangle)]
pub extern "C" fn ahead_alloc(length: usize) -> *mut u8 {
    let mut bytes = vec![0_u8; length].into_boxed_slice();
    let pointer = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    pointer
}

/// Releases a buffer previously returned by this module.
///
/// # Safety
///
/// `pointer` and `length` must describe a live allocation returned by `ahead_alloc`
/// or `ahead_dispatch`, and the allocation must not be released more than once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ahead_dealloc(pointer: *mut u8, length: usize) {
    if !pointer.is_null() && length > 0 {
        // SAFETY: callers must pass pointers and lengths returned by this module's ABI.
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                pointer, length,
            )));
        }
    }
}

/// Dispatches one UTF-8 JSON request and returns a packed output pointer and length.
///
/// # Safety
///
/// `pointer` must refer to at least `length` initialized, readable bytes in this
/// module's linear memory for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ahead_dispatch(pointer: *const u8, length: usize) -> u64 {
    let output = if pointer.is_null() || length == 0 {
        error_json("invalid_request", "request body cannot be empty")
    } else {
        // SAFETY: the host writes `length` initialized bytes into memory from ahead_alloc.
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
        match std::str::from_utf8(bytes) {
            Ok(json) => dispatch_json(json),
            Err(error) => error_json("invalid_utf8", &error.to_string()),
        }
    };
    leak_output(output)
}

/// Decodes, validates, and dispatches one JSON request.
fn dispatch_json(json: &str) -> Vec<u8> {
    let request = match serde_json::from_str::<Request>(json) {
        Ok(request) => request,
        Err(error) => return error_json("invalid_request", &error.to_string()),
    };
    if request.api_version != ENGINE_API_VERSION {
        return error_json(
            "unsupported_engine_version",
            &format!("expected {ENGINE_API_VERSION}, got {}", request.api_version),
        );
    }

    let result = match request.operation.as_str() {
        "list_workflows" => workflows().and_then(to_value),
        "get_workflow" => decode::<WorkflowInput>(request.input)
            .and_then(|input| workflow(&input.workflow_id))
            .and_then(to_value),
        "create_run" => decode::<CreateRunInput>(request.input)
            .and_then(create_run)
            .and_then(to_value),
        "derive_state" | "validate_run" => decode::<RunInput>(request.input)
            .and_then(|input| {
                if request.operation == "validate_run" {
                    validate_run(&input.run)
                } else {
                    derive_state(&input.run)
                }
            })
            .and_then(to_value),
        "apply_event" => decode::<ApplyOperationInput>(request.input)
            .and_then(|input| apply(&input.run, input.event))
            .and_then(to_value),
        "tool_allowed" => decode::<ToolInput>(request.input)
            .and_then(|input| tool_allowed(&input.run, input.capability))
            .and_then(to_value),
        operation => Err(ahead_core::AheadError {
            code: "unknown_operation".to_owned(),
            message: format!("unknown operation: {operation}"),
        }),
    };

    match result {
        Ok(result) => serde_json::to_vec(&Response {
            ok: true,
            result: Some(result),
            error: None,
        })
        .unwrap_or_else(|error| error_json("serialization_failed", &error.to_string())),
        Err(error) => serde_json::to_vec(&Response::<Value> {
            ok: false,
            result: None,
            error: Some(error),
        })
        .unwrap_or_else(|serialize_error| {
            error_json("serialization_failed", &serialize_error.to_string())
        }),
    }
}

/// Deserializes one operation's input or returns a structured engine error.
fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> ahead_core::Result<T> {
    serde_json::from_value(value).map_err(|error| ahead_core::AheadError {
        code: "invalid_input".to_owned(),
        message: error.to_string(),
    })
}

/// Serializes a successful operation result into a generic JSON value.
fn to_value<T: Serialize>(value: T) -> ahead_core::Result<Value> {
    serde_json::to_value(value).map_err(|error| ahead_core::AheadError {
        code: "serialization_failed".to_owned(),
        message: error.to_string(),
    })
}

/// Serializes a structured error response, with an infallible fallback payload.
fn error_json(code: &str, message: &str) -> Vec<u8> {
    serde_json::to_vec(&Response::<Value> {
        ok: false,
        result: None,
        error: Some(ahead_core::AheadError {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
    })
    .unwrap_or_else(|_| br#"{\"ok\":false}"#.to_vec())
}

/// Leaks an output buffer to the host and packs its pointer and length into a `u64`.
fn leak_output(bytes: Vec<u8>) -> u64 {
    let mut bytes = bytes.into_boxed_slice();
    let pointer_address = bytes.as_mut_ptr() as usize;
    let (Ok(pointer), Ok(length)) = (u32::try_from(pointer_address), u32::try_from(bytes.len()))
    else {
        return 0;
    };
    std::mem::forget(bytes);
    (u64::from(pointer) << 32) | u64::from(length)
}
