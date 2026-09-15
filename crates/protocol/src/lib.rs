//! Stable business protocol types and state-machine guards.
//!
//! This crate deliberately contains no transport or persistence code.  It is
//! the contract shared by a source computer and its viewers; adapters can
//! serialize the envelopes and persist the small guard records later.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub type DeviceId = String;
pub type GroupId = String;
pub type ProjectId = String;
pub type AccountId = String;
pub type ThreadId = String;
pub type TurnId = String;
pub type ApprovalId = String;
pub type RequestId = String;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ThreadKey {
    pub device_id: DeviceId,
    pub thread_id: ThreadId,
}

impl ThreadKey {
    pub fn new(device_id: impl Into<DeviceId>, thread_id: impl Into<ThreadId>) -> Self {
        Self {
            device_id: device_id.into(),
            thread_id: thread_id.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub device_id: DeviceId,
    pub group_id: GroupId,
    pub name: String,
    pub codex_capable: bool,
    pub status: DeviceStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DeviceStatus {
    Online,
    Offline,
    Connecting,
    AuthenticationFailed,
    Incompatible,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub group_id: GroupId,
    pub name: String,
    pub members: Vec<DeviceId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub project_id: ProjectId,
    pub source_device_id: DeviceId,
    pub name: String,
    pub path: String,
    pub available_models: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountQuota {
    pub account_id: AccountId,
    pub source_device_id: DeviceId,
    pub remaining_percent: Option<f64>,
    pub window_seconds: Option<u64>,
    pub resets_at_unix_seconds: Option<i64>,
    pub observed_at_unix_seconds: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub key: ThreadKey,
    pub project_id: Option<ProjectId>,
    pub title: Option<String>,
    pub status: ThreadStatus,
    pub process_generation: ProcessGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ThreadStatus {
    Idle,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub key: ThreadKey,
    pub turn_id: TurnId,
    pub prompt: String,
    pub status: TurnStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TurnStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Interrupted,
    ResultUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    pub approval_id: ApprovalId,
    pub thread: ThreadKey,
    pub turn_id: TurnId,
    pub source_process_generation: ProcessGeneration,
    pub original_request_id: RequestId,
    pub description: String,
    pub options: Vec<ApprovalOption>,
    pub status: ApprovalStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovalOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApprovalStatus {
    Pending,
    Resolved { option_id: String },
    Expired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub request_id: RequestId,
    pub state: CommandState,
    pub value: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommandState {
    Accepted,
    Succeeded,
    Failed,
    ResultUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ProcessGeneration(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Cursor(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const V1: Self = Self { major: 1, minor: 0 };
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub version: ProtocolVersion,
    pub request_id: RequestId,
    pub source_device_id: DeviceId,
    pub target_device_id: DeviceId,
    pub operation: Operation,
    pub params: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Operation {
    ProjectsList,
    StateSubscribe,
    ThreadsList,
    ThreadsRead,
    ThreadsCreate,
    TurnsSend,
    TurnsSteer,
    TurnsInterrupt,
    ApprovalsList,
    ApprovalsResolve,
    CommandsGet,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub version: ProtocolVersion,
    pub request_id: RequestId,
    pub result: Option<Value>,
    pub error: Option<ProtocolError>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub version: ProtocolVersion,
    pub source_device_id: DeviceId,
    pub process_generation: ProcessGeneration,
    pub cursor: Cursor,
    pub event: Event,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Snapshot {
        devices: Vec<Device>,
        threads: Vec<Thread>,
    },
    ThreadChanged(Thread),
    TurnChanged(Turn),
    ApprovalChanged(Approval),
    CommandResult(CommandResult),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProtocolError {
    RequestIdConflict {
        request_id: RequestId,
    },
    CursorExpired {
        oldest_available: Cursor,
    },
    StaleProcessGeneration {
        expected: ProcessGeneration,
        actual: ProcessGeneration,
    },
    ApprovalAlreadyResolved,
    UnknownApproval,
}

/// A stable digest of the operation and parameters, excluding the request ID.
pub fn request_digest(request: &RequestEnvelope) -> String {
    let material = (
        &request.source_device_id,
        &request.target_device_id,
        &request.operation,
        &request.params,
    );
    let bytes = serde_json::to_vec(&material).expect("protocol request is serializable");
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyOutcome {
    New,
    Replayed(ResponseEnvelope),
    Conflict,
}

#[derive(Default)]
pub struct IdempotencyLedger {
    entries: BTreeMap<RequestId, (String, ResponseEnvelope)>,
}

impl IdempotencyLedger {
    pub fn accept(&mut self, request: &RequestEnvelope) -> IdempotencyOutcome {
        let digest = request_digest(request);
        match self.entries.get(&request.request_id) {
            None => IdempotencyOutcome::New,
            Some((known, response)) if known == &digest => {
                IdempotencyOutcome::Replayed(response.clone())
            }
            Some(_) => IdempotencyOutcome::Conflict,
        }
    }

    pub fn record(&mut self, request: &RequestEnvelope, response: ResponseEnvelope) {
        self.entries.insert(
            request.request_id.clone(),
            (request_digest(request), response),
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorDecision {
    Incremental,
    SnapshotRequired { oldest_available: Cursor },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorWindow {
    pub oldest_available: Cursor,
    pub newest: Cursor,
}

impl Default for CursorWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorWindow {
    pub fn new() -> Self {
        Self {
            oldest_available: Cursor(1),
            newest: Cursor(0),
        }
    }
    pub fn push(&mut self) -> Cursor {
        self.newest = Cursor(self.newest.0 + 1);
        self.newest
    }
    pub fn retain_from(&mut self, oldest: Cursor) {
        self.oldest_available = oldest.min(Cursor(self.newest.0 + 1));
    }
    pub fn check(&self, after: Option<Cursor>) -> CursorDecision {
        match after {
            None => CursorDecision::SnapshotRequired {
                oldest_available: self.oldest_available,
            },
            Some(cursor) if cursor.0 + 1 < self.oldest_available.0 => {
                CursorDecision::SnapshotRequired {
                    oldest_available: self.oldest_available,
                }
            }
            Some(_) => CursorDecision::Incremental,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalResolution {
    Resolved(Approval),
    Rejected(ProtocolError),
}

pub fn resolve_approval(
    approval: &mut Approval,
    generation: ProcessGeneration,
    request_id: &str,
    option_id: &str,
) -> ApprovalResolution {
    if approval.source_process_generation != generation {
        approval.status = ApprovalStatus::Expired;
        return ApprovalResolution::Rejected(ProtocolError::StaleProcessGeneration {
            expected: approval.source_process_generation,
            actual: generation,
        });
    }
    if approval.status != ApprovalStatus::Pending {
        return ApprovalResolution::Rejected(ProtocolError::ApprovalAlreadyResolved);
    }
    if approval.original_request_id != request_id
        || !approval.options.iter().any(|option| option.id == option_id)
    {
        return ApprovalResolution::Rejected(ProtocolError::UnknownApproval);
    }
    approval.status = ApprovalStatus::Resolved {
        option_id: option_id.to_owned(),
    };
    ApprovalResolution::Resolved(approval.clone())
}

pub fn decode<T: DeserializeOwned>(value: &Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str, params: Value) -> RequestEnvelope {
        RequestEnvelope {
            version: ProtocolVersion::V1,
            request_id: id.into(),
            source_device_id: "a".into(),
            target_device_id: "b".into(),
            operation: Operation::ThreadsList,
            params,
        }
    }
    fn approval() -> Approval {
        Approval {
            approval_id: "ap".into(),
            thread: ThreadKey::new("b", "t"),
            turn_id: "turn".into(),
            source_process_generation: ProcessGeneration(7),
            original_request_id: "resolve-1".into(),
            description: "run".into(),
            options: vec![ApprovalOption {
                id: "allow".into(),
                label: "Allow".into(),
            }],
            status: ApprovalStatus::Pending,
        }
    }

    #[test]
    fn same_request_id_and_parameters_replays_response() {
        let mut ledger = IdempotencyLedger::default();
        let req = request("r1", serde_json::json!({"page": 1}));
        assert_eq!(ledger.accept(&req), IdempotencyOutcome::New);
        let response = ResponseEnvelope {
            version: ProtocolVersion::V1,
            request_id: "r1".into(),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        };
        ledger.record(&req, response.clone());
        assert_eq!(ledger.accept(&req), IdempotencyOutcome::Replayed(response));
    }
    #[test]
    fn same_request_id_with_other_parameters_is_rejected() {
        let mut ledger = IdempotencyLedger::default();
        let a = request("r1", serde_json::json!({"page": 1}));
        ledger.record(
            &a,
            ResponseEnvelope {
                version: ProtocolVersion::V1,
                request_id: "r1".into(),
                result: None,
                error: None,
            },
        );
        assert_eq!(
            ledger.accept(&request("r1", serde_json::json!({"page": 2}))),
            IdempotencyOutcome::Conflict
        );
    }
    #[test]
    fn expired_cursor_requires_snapshot() {
        let mut window = CursorWindow::new();
        for _ in 0..4 {
            window.push();
        }
        window.retain_from(Cursor(3));
        assert_eq!(
            window.check(Some(Cursor(1))),
            CursorDecision::SnapshotRequired {
                oldest_available: Cursor(3)
            }
        );
        assert_eq!(window.check(Some(Cursor(3))), CursorDecision::Incremental);
    }
    #[test]
    fn approval_from_old_process_generation_expires() {
        let mut value = approval();
        assert!(matches!(
            resolve_approval(&mut value, ProcessGeneration(8), "resolve-1", "allow"),
            ApprovalResolution::Rejected(ProtocolError::StaleProcessGeneration { .. })
        ));
        assert_eq!(value.status, ApprovalStatus::Expired);
    }
}
