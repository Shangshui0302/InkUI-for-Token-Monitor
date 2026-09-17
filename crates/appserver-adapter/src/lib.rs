//! Transport-neutral JSONL adapter for the Codex App Server protocol.
//!
//! The adapter deliberately owns only framing, request correlation, and event
//! routing. A caller can write [`Outbound::line`] to an app-server stdio stream
//! and feed each stdout line back through [`Adapter::receive_line`].

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// JSON-RPC-style request identifiers accepted by Codex App Server.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(u64),
    String(String),
}

/// A JSONL frame to write to Codex App Server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Outbound {
    id: Option<RequestId>,
    method: String,
    line: String,
}

impl Outbound {
    pub fn request_id(&self) -> Option<&RequestId> {
        self.id.as_ref()
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn line(&self) -> &str {
        &self.line
    }
}

/// The method associated with an outstanding client request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRequest {
    pub method: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub title: Option<String>,
    pub version: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeCapabilities {
    pub experimental_api: bool,
    pub request_attestation: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opt_out_notification_methods: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    pub capabilities: Option<InitializeCapabilities>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_turns: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadTurnsListParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_view: Option<String>,
}

/// Minimal stable subset used by the source service. Unknown server response
/// fields are intentionally left to the caller as JSON.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// Context shared by command and file-change approval prompts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalContext {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub started_at_ms: u64,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalRequest {
    Command {
        id: RequestId,
        context: ApprovalContext,
        command: Option<String>,
        cwd: Option<String>,
    },
    FileChange {
        id: RequestId,
        context: ApprovalContext,
        grant_root: Option<String>,
    },
}

/// An app-server response error body. Unknown fields stay available in `data`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Response {
        id: RequestId,
        method: String,
        result: Value,
    },
    ErrorResponse {
        id: RequestId,
        method: String,
        error: ResponseError,
    },
    Notification {
        method: String,
        params: Value,
    },
    Approval(ApprovalRequest),
    UnknownServerRequest {
        id: RequestId,
        method: String,
        params: Value,
    },
    UnknownResponse {
        id: RequestId,
        result: Option<Value>,
        error: Option<ResponseError>,
    },
}

#[derive(Debug)]
pub enum AdapterError {
    Json(serde_json::Error),
    InvalidFrame(&'static str),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid JSONL frame: {error}"),
            Self::InvalidFrame(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for AdapterError {}

impl From<serde_json::Error> for AdapterError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

/// Stateful request builder and incoming frame router.
#[derive(Debug)]
pub struct Adapter {
    next_id: u64,
    pending: HashMap<RequestId, PendingRequest>,
}

impl Default for Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl Adapter {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            pending: HashMap::new(),
        }
    }

    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn initialize(&mut self, params: InitializeParams) -> Result<Outbound, AdapterError> {
        self.request("initialize", params)
    }

    pub fn initialized(&self) -> Result<Outbound, AdapterError> {
        self.notification("initialized", json!({}))
    }

    pub fn thread_read(&mut self, params: ThreadReadParams) -> Result<Outbound, AdapterError> {
        self.request("thread/read", params)
    }

    pub fn thread_turns_list(
        &mut self,
        params: ThreadTurnsListParams,
    ) -> Result<Outbound, AdapterError> {
        self.request("thread/turns/list", params)
    }

    pub fn thread_list(&mut self, params: ThreadListParams) -> Result<Outbound, AdapterError> {
        self.request("thread/list", params)
    }

    pub fn account_rate_limits_read(&mut self) -> Result<Outbound, AdapterError> {
        self.request("account/rateLimits/read", json!({}))
    }

    pub fn account_usage_read(&mut self) -> Result<Outbound, AdapterError> {
        self.request("account/usage/read", json!({}))
    }

    pub fn receive_line(&mut self, line: &str) -> Result<Incoming, AdapterError> {
        let value: Value = serde_json::from_str(line.trim_end_matches(['\r', '\n']))?;
        let object = value.as_object().ok_or(AdapterError::InvalidFrame(
            "app-server frame must be a JSON object",
        ))?;
        let id = object
            .get("id")
            .cloned()
            .map(serde_json::from_value::<RequestId>)
            .transpose()?;
        let method = object.get("method").and_then(Value::as_str);

        match (id, method) {
            (Some(id), Some(method)) => {
                let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
                self.route_server_request(id, method, params)
            }
            (None, Some(method)) => Ok(Incoming::Notification {
                method: method.to_owned(),
                params: object.get("params").cloned().unwrap_or(Value::Null),
            }),
            (Some(id), None) => self.route_response(id, object),
            (None, None) => Err(AdapterError::InvalidFrame(
                "app-server frame has neither method nor id",
            )),
        }
    }

    fn request<T: Serialize>(&mut self, method: &str, params: T) -> Result<Outbound, AdapterError> {
        let id = RequestId::Number(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(AdapterError::InvalidFrame("request id counter overflowed"))?;
        let value = json!({ "method": method, "id": id, "params": params });
        let line = encode_line(&value)?;
        self.pending.insert(
            id.clone(),
            PendingRequest {
                method: method.to_owned(),
            },
        );
        Ok(Outbound {
            id: Some(id),
            method: method.to_owned(),
            line,
        })
    }

    fn notification<T: Serialize>(
        &self,
        method: &str,
        params: T,
    ) -> Result<Outbound, AdapterError> {
        let value = json!({ "method": method, "params": params });
        Ok(Outbound {
            id: None,
            method: method.to_owned(),
            line: encode_line(&value)?,
        })
    }

    fn route_response(
        &mut self,
        id: RequestId,
        object: &serde_json::Map<String, Value>,
    ) -> Result<Incoming, AdapterError> {
        let result = object.get("result").cloned();
        let error = object
            .get("error")
            .cloned()
            .map(serde_json::from_value::<ResponseError>)
            .transpose()?;
        let Some(pending) = self.pending.get(&id) else {
            return Ok(Incoming::UnknownResponse { id, result, error });
        };
        if result.is_none() && error.is_none() {
            return Err(AdapterError::InvalidFrame(
                "response has neither result nor error",
            ));
        }
        let method = pending.method.clone();
        self.pending.remove(&id);
        if let Some(error) = error {
            return Ok(Incoming::ErrorResponse { id, method, error });
        }
        Ok(Incoming::Response {
            id,
            method,
            // Presence was checked before consuming the pending request.
            result: result.expect("checked above"),
        })
    }

    fn route_server_request(
        &self,
        id: RequestId,
        method: &str,
        params: Value,
    ) -> Result<Incoming, AdapterError> {
        match method {
            "item/commandExecution/requestApproval" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct CommandParams {
                    #[serde(flatten)]
                    context: ApprovalContext,
                    #[serde(default)]
                    command: Option<String>,
                    #[serde(default)]
                    cwd: Option<String>,
                }
                let parsed: CommandParams = serde_json::from_value(params.clone())?;
                Ok(Incoming::Approval(ApprovalRequest::Command {
                    id,
                    context: parsed.context,
                    command: parsed.command,
                    cwd: parsed.cwd,
                }))
            }
            "item/fileChange/requestApproval" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct FileParams {
                    #[serde(flatten)]
                    context: ApprovalContext,
                    #[serde(default)]
                    grant_root: Option<String>,
                }
                let parsed: FileParams = serde_json::from_value(params.clone())?;
                Ok(Incoming::Approval(ApprovalRequest::FileChange {
                    id,
                    context: parsed.context,
                    grant_root: parsed.grant_root,
                }))
            }
            _ => Ok(Incoming::UnknownServerRequest {
                id,
                method: method.to_owned(),
                params,
            }),
        }
    }
}

fn encode_line(value: &Value) -> Result<String, AdapterError> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter_with_initialize() -> (Adapter, Outbound) {
        let mut adapter = Adapter::new();
        let request = adapter
            .initialize(InitializeParams {
                client_info: ClientInfo {
                    name: "nekodex_adapter_test".into(),
                    title: Some("Nekodex Adapter Test".into()),
                    version: "0.1.0".into(),
                },
                capabilities: Some(InitializeCapabilities {
                    experimental_api: true,
                    request_attestation: false,
                    opt_out_notification_methods: vec![],
                }),
            })
            .unwrap();
        (adapter, request)
    }

    #[test]
    fn correlates_response_with_original_request() {
        let (mut adapter, request) = adapter_with_initialize();
        assert_eq!(adapter.pending_count(), 1);
        assert_eq!(request.id, Some(RequestId::Number(1)));
        assert!(request.line.ends_with('\n'));

        let incoming = adapter
            .receive_line(
                r#"{"id":1,"result":{"userAgent":"codex","codexHome":"/tmp/codex","platformFamily":"unix","platformOs":"linux"}}"#,
            )
            .unwrap();
        assert_eq!(adapter.pending_count(), 0);
        assert!(matches!(
            incoming,
            Incoming::Response { id: RequestId::Number(1), ref method, .. }
                if method == "initialize"
        ));
    }

    #[test]
    fn builds_monitoring_requests_with_current_wire_names() {
        let mut adapter = Adapter::new();
        let read = adapter
            .thread_read(ThreadReadParams {
                thread_id: "thread-1".into(),
                include_turns: Some(false),
            })
            .unwrap();
        let list = adapter
            .thread_list(ThreadListParams {
                cwd: Some(vec!["/workspace".into()]),
                limit: Some(50),
                ..ThreadListParams::default()
            })
            .unwrap();
        let turns = adapter
            .thread_turns_list(ThreadTurnsListParams {
                thread_id: "thread-1".into(),
                limit: Some(20),
                sort_direction: Some("desc".into()),
                items_view: Some("summary".into()),
                ..ThreadTurnsListParams::default()
            })
            .unwrap();

        let frames: Vec<Value> = [read, list, turns]
            .into_iter()
            .map(|outbound| serde_json::from_str(&outbound.line).unwrap())
            .collect();
        assert_eq!(frames[0]["method"], "thread/read");
        assert_eq!(frames[1]["method"], "thread/list");
        assert_eq!(frames[1]["params"]["cwd"], json!(["/workspace"]));
        assert_eq!(frames[2]["method"], "thread/turns/list");
        assert_eq!(frames[2]["params"]["itemsView"], "summary");
    }

    #[test]
    fn routes_notification_without_consuming_pending_request() {
        let (mut adapter, _) = adapter_with_initialize();
        let incoming = adapter
            .receive_line(r#"{"method":"turn/completed","params":{"turn":{"id":"turn-1","status":"interrupted"}}}"#)
            .unwrap();
        assert_eq!(adapter.pending_count(), 1);
        assert_eq!(
            incoming,
            Incoming::Notification {
                method: "turn/completed".into(),
                params: json!({"turn":{"id":"turn-1","status":"interrupted"}}),
            }
        );
    }

    #[test]
    fn routes_command_and_file_approval_requests() {
        let mut adapter = Adapter::new();
        let command = adapter
            .receive_line(r#"{"id":"approval-1","method":"item/commandExecution/requestApproval","params":{"kind":"command","threadId":"thread-1","turnId":"turn-1","itemId":"item-1","startedAtMs":42,"approvalId":null,"command":"printf ok","cwd":"/workspace","reason":"write output"}}"#)
            .unwrap();
        let file = adapter
            .receive_line(r#"{"id":9,"method":"item/fileChange/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-2","startedAtMs":43,"reason":null,"grantRoot":"/workspace"}}"#)
            .unwrap();

        let Incoming::Approval(command) = command else {
            panic!("command approval was not routed")
        };
        assert!(matches!(
            command,
            ApprovalRequest::Command {
                ref command,
                ref context,
                ..
            } if command.as_deref() == Some("printf ok")
                && context.item_id == "item-1"
        ));
        assert!(matches!(
            file,
            Incoming::Approval(ApprovalRequest::FileChange {
                id: RequestId::Number(9),
                ref grant_root,
                ..
            }) if grant_root.as_deref() == Some("/workspace")
        ));
    }

    #[test]
    fn surfaces_unknown_response_without_stealing_another_request() {
        let (mut adapter, _) = adapter_with_initialize();
        let incoming = adapter
            .receive_line(r#"{"id":999,"result":{"unexpected":true}}"#)
            .unwrap();
        assert_eq!(adapter.pending_count(), 1);
        assert_eq!(
            incoming,
            Incoming::UnknownResponse {
                id: RequestId::Number(999),
                result: Some(json!({"unexpected": true})),
                error: None,
            }
        );
    }

    #[test]
    fn initialized_is_a_notification() {
        let adapter = Adapter::new();
        let initialized = adapter.initialized().unwrap();
        assert_eq!(initialized.id, None);
        assert_eq!(
            serde_json::from_str::<Value>(&initialized.line).unwrap(),
            json!({"method":"initialized","params":{}})
        );
    }
}
