use anyhow::{Result, anyhow};
use serde::de::DeserializeOwned;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct RpcObject(pub Value);

pub type RequestId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
/// An RPC call, which may be either a notification or a request.
pub enum Call<N, R> {
    /// An id and an RPC Request
    Request(RequestId, R),
    /// An RPC Notification
    Notification(N),
}

impl RpcObject {
    pub fn get_id(&self) -> Option<RequestId> {
        self.0.get("id").and_then(Value::as_u64)
    }

    pub fn is_response(&self) -> bool {
        self.0.get("id").is_some() && self.0.get("method").is_none()
    }

    pub fn into_rpc<N, R>(self) -> Result<Call<N, R>>
    where
        N: DeserializeOwned,
        R: DeserializeOwned,
    {
        let id = self.get_id();
        match id {
            Some(id) => match serde_json::from_value::<R>(self.0) {
                Ok(resp) => Ok(Call::Request(id, resp)),
                Err(err) => Err(anyhow!(err)),
            },
            None => {
                let result = serde_json::from_value::<N>(self.0)?;
                Ok(Call::Notification(result))
            }
        }
    }

    /// Splits a response object into its result or error payload.
    ///
    /// # Panics
    ///
    /// Panics if the object is not a JSON object (protocol violation that
    /// `into_call` already rejects for well-formed inputs).
    pub fn into_response(mut self) -> Result<Result<Value, Value>, String> {
        let _ = self
            .get_id()
            .ok_or_else(|| "Response requires 'id' field.".to_string())?;

        if self.0.get("result").is_some() == self.0.get("error").is_some() {
            return Err("RPC response must contain exactly one of\
                        'error' or 'result' fields."
                .into());
        }
        let result = self.0.as_object_mut().and_then(|obj| obj.remove("result"));

        match result {
            Some(r) => Ok(Ok(r)),
            None => match self.0.as_object_mut().and_then(|obj| obj.remove("error"))
            {
                Some(error) => Ok(Err(error)),
                None => Err("RPC response is neither result nor error.".to_string()),
            },
        }
    }
}

impl From<Value> for RpcObject {
    fn from(v: Value) -> RpcObject {
        RpcObject(v)
    }
}
