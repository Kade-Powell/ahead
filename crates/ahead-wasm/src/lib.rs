use ahead_core::{
    ApplyInput, Capability, CreateRunInput, ENGINE_API_VERSION, Run, apply, create_run,
    derive_state, tool_allowed, validate_run, workflow,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Request {
    api_version: String,
    operation: String,
    #[serde(default)]
    input: Value,
}

#[derive(Debug, Serialize)]
struct Response<T: Serialize> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ahead_core::AheadError>,
}

#[derive(Debug, Deserialize)]
struct WorkflowInput {
    workflow_id: String,
}

#[derive(Debug, Deserialize)]
struct RunInput {
    run: Run,
}

#[derive(Debug, Deserialize)]
struct ApplyOperationInput {
    run: Run,
    event: ApplyInput,
}

#[derive(Debug, Deserialize)]
struct ToolInput {
    run: Run,
    capability: Capability,
}

#[unsafe(no_mangle)]
pub extern "C" fn ahead_alloc(length: usize) -> *mut u8 {
    let mut bytes = Vec::<u8>::with_capacity(length);
    let pointer = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    pointer
}

#[unsafe(no_mangle)]
/// Releases a buffer previously returned by this module.
///
/// # Safety
///
/// `pointer` and `length` must describe a live allocation returned by `ahead_alloc`
/// or `ahead_dispatch`, and the allocation must not be released more than once.
pub unsafe extern "C" fn ahead_dealloc(pointer: *mut u8, length: usize) {
    if !pointer.is_null() && length > 0 {
        // SAFETY: callers must pass pointers and lengths returned by this module's ABI.
        unsafe {
            drop(Vec::from_raw_parts(pointer, length, length));
        }
    }
}

#[unsafe(no_mangle)]
/// Dispatches one UTF-8 JSON request and returns a packed output pointer and length.
///
/// # Safety
///
/// `pointer` must refer to at least `length` initialized, readable bytes in this
/// module's linear memory for the duration of the call.
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

fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> ahead_core::Result<T> {
    serde_json::from_value(value).map_err(|error| ahead_core::AheadError {
        code: "invalid_input".to_owned(),
        message: error.to_string(),
    })
}

fn to_value<T: Serialize>(value: T) -> ahead_core::Result<Value> {
    serde_json::to_value(value).map_err(|error| ahead_core::AheadError {
        code: "serialization_failed".to_owned(),
        message: error.to_string(),
    })
}

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

fn leak_output(bytes: Vec<u8>) -> u64 {
    let mut bytes = bytes.into_boxed_slice();
    let pointer = bytes.as_mut_ptr() as u32;
    let length = bytes.len() as u32;
    std::mem::forget(bytes);
    ((pointer as u64) << 32) | length as u64
}
