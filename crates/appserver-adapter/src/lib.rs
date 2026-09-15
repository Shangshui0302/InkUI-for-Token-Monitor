//! Transport-neutral JSONL adapter for the Codex App Server protocol.
//!
//! The adapter deliberately owns only framing, request correlation, and event
//! routing. A caller can write [`Outbound::line`] to an app-server stdio stream
//! and feed each stdout line back through [`Adapter::receive_line`].

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
    pub id: Option<RequestId>,
    pub method: String,
    pub line: String,
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_turns: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_turns: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserInput {
    Text {
        text: String,
        #[serde(default)]
        text_elements: Vec<Value>,
    },
    Image {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    LocalImage {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

impl UserInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            text_elements: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    pub expected_turn_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    pub turn_id: String,
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
        kind: String,
        approval_id: Option<String>,
        command: Option<String>,
        cwd: Option<String>,
        params: Value,
    },
    FileChange {
        id: RequestId,
        context: ApprovalContext,
        grant_root: Option<String>,
        params: Value,
    },
}

impl ApprovalRequest {
    pub fn id(&self) -> &RequestId {
        match self {
            Self::Command { id, .. } | Self::FileChange { id, .. } => id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

impl ApprovalDecision {
    fn wire_value(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::AcceptForSession => "acceptForSession",
            Self::Decline => "decline",
            Self::Cancel => "cancel",
        }
    }
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

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn initialize(&mut self, params: InitializeParams) -> Result<Outbound, AdapterError> {
        self.request("initialize", params)
    }

    pub fn initialized(&self) -> Result<Outbound, AdapterError> {
        self.notification("initialized", json!({}))
    }

    pub fn thread_start(&mut self, params: ThreadStartParams) -> Result<Outbound, AdapterError> {
        self.request("thread/start", params)
    }

    pub fn thread_read(&mut self, params: ThreadReadParams) -> Result<Outbound, AdapterError> {
        self.request("thread/read", params)
    }

    pub fn thread_resume(&mut self, params: ThreadResumeParams) -> Result<Outbound, AdapterError> {
        self.request("thread/resume", params)
    }

    pub fn turn_start(&mut self, params: TurnStartParams) -> Result<Outbound, AdapterError> {
        self.request("turn/start", params)
    }

    pub fn turn_steer(&mut self, params: TurnSteerParams) -> Result<Outbound, AdapterError> {
        self.request("turn/steer", params)
    }

    pub fn turn_interrupt(
        &mut self,
        params: TurnInterruptParams,
    ) -> Result<Outbound, AdapterError> {
        self.request("turn/interrupt", params)
    }

    pub fn approval_response(
        &self,
        request: &ApprovalRequest,
        decision: ApprovalDecision,
    ) -> Result<Outbound, AdapterError> {
        self.response(
            request.id().clone(),
            json!({ "decision": decision.wire_value() }),
        )
    }

    pub fn server_request_error(
        &self,
        id: RequestId,
        code: i64,
        message: impl Into<String>,
    ) -> Result<Outbound, AdapterError> {
        let value = json!({
            "id": &id,
            "error": { "code": code, "message": message.into() }
        });
        Ok(Outbound {
            id: Some(id),
            method: String::new(),
            line: encode_line(&value)?,
        })
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

    fn response(&self, id: RequestId, result: Value) -> Result<Outbound, AdapterError> {
        let value = json!({ "id": &id, "result": result });
        Ok(Outbound {
            id: Some(id),
            method: String::new(),
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
                    #[serde(default = "default_command_kind")]
                    kind: String,
                    #[serde(default)]
                    approval_id: Option<String>,
                    #[serde(default)]
                    command: Option<String>,
                    #[serde(default)]
                    cwd: Option<String>,
                }
                let parsed: CommandParams = serde_json::from_value(params.clone())?;
                Ok(Incoming::Approval(ApprovalRequest::Command {
                    id,
                    context: parsed.context,
                    kind: parsed.kind,
                    approval_id: parsed.approval_id,
                    command: parsed.command,
                    cwd: parsed.cwd,
                    params,
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
                    params,
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

fn default_command_kind() -> String {
    "command".to_owned()
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
                    name: "nekodex_p0".into(),
                    title: Some("Nekodex P0".into()),
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
    fn builds_lifecycle_requests_with_current_wire_names() {
        let mut adapter = Adapter::new();
        let start = adapter
            .thread_start(ThreadStartParams {
                cwd: Some("/workspace".into()),
                approval_policy: Some("untrusted".into()),
                approvals_reviewer: Some("user".into()),
                sandbox: Some("workspace-write".into()),
                ephemeral: Some(true),
                service_name: Some("nekodex_p0".into()),
            })
            .unwrap();
        let read = adapter
            .thread_read(ThreadReadParams {
                thread_id: "thread-1".into(),
                include_turns: Some(false),
            })
            .unwrap();
        let resume = adapter
            .thread_resume(ThreadResumeParams {
                thread_id: "thread-1".into(),
                exclude_turns: Some(true),
            })
            .unwrap();
        let turn = adapter
            .turn_start(TurnStartParams {
                thread_id: "thread-1".into(),
                input: vec![UserInput::text("hello")],
            })
            .unwrap();
        let steer = adapter
            .turn_steer(TurnSteerParams {
                thread_id: "thread-1".into(),
                input: vec![UserInput::text("more")],
                expected_turn_id: "turn-1".into(),
            })
            .unwrap();
        let interrupt = adapter
            .turn_interrupt(TurnInterruptParams {
                thread_id: "thread-1".into(),
                turn_id: "turn-1".into(),
            })
            .unwrap();

        let frames: Vec<Value> = [start, read, resume, turn, steer, interrupt]
            .into_iter()
            .map(|outbound| serde_json::from_str(&outbound.line).unwrap())
            .collect();
        assert_eq!(frames[0]["method"], "thread/start");
        assert_eq!(frames[0]["params"]["approvalsReviewer"], "user");
        assert_eq!(frames[1]["method"], "thread/read");
        assert_eq!(frames[2]["method"], "thread/resume");
        assert_eq!(frames[3]["method"], "turn/start");
        assert_eq!(frames[3]["params"]["input"][0]["text_elements"], json!([]));
        assert_eq!(frames[4]["params"]["expectedTurnId"], "turn-1");
        assert_eq!(frames[5]["method"], "turn/interrupt");
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
                ref kind,
                ref command,
                ref context,
                ..
            } if kind == "command"
                && command.as_deref() == Some("printf ok")
                && context.item_id == "item-1"
        ));
        let response = adapter
            .approval_response(&command, ApprovalDecision::Accept)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response.line).unwrap(),
            json!({"id":"approval-1","result":{"decision":"accept"}})
        );

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
