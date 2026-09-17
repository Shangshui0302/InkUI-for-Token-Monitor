//! Stable business protocol types and state-machine guards.
//!
//! This crate deliberately contains no transport or persistence code.  It is
//! the contract shared by a source computer and its viewers; adapters can
//! serialize the envelopes and persist the small guard records later.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeSet;

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

/// Cursor-based pagination shared by all list/read operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageRequest {
    pub after: Option<Cursor>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<Cursor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub message_id: String,
    pub role: MessageRole,
    pub text: String,
    pub created_at_unix_seconds: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    pub source_device_id: DeviceId,
    pub activity_id: String,
    pub thread: ThreadKey,
    pub turn_id: Option<TurnId>,
    pub kind: ActivityKind,
    pub message: Option<String>,
    pub occurred_at_unix_seconds: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ActivityKind {
    Started,
    Progress,
    Message,
    Finished,
    Error,
}

/// Compatibility name for the canonical, source-scoped quota snapshot.
pub type AccountQuota = AccountQuotaSnapshot;

/// A provider may expose one or more independently resetting quota windows.
/// Missing values remain `None` and are never synthesized from another source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    pub name: String,
    pub remaining_percent: Option<f64>,
    pub limit: Option<f64>,
    pub used: Option<f64>,
    pub resets_at_unix_seconds: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountQuotaSnapshot {
    pub account_id: AccountId,
    pub source_device_id: DeviceId,
    pub windows: Vec<QuotaWindow>,
    pub observed_at_unix_seconds: i64,
}

// Protocol values are compared structurally; providers are expected to emit
// finite percentages, while serde still preserves any unusual value losslessly.
impl Eq for AccountQuotaSnapshot {}

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

/// Capabilities negotiated after the authenticated transport proof. Unknown
/// feature identifiers are intentionally harmless: negotiation uses set
/// intersection, so both peers must explicitly advertise a feature.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BusinessCapabilities {
    pub protocol: ProtocolVersion,
    pub features: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityNegotiationError {
    IncompatibleMajor { local: u16, remote: u16 },
    MissingRequiredFeature,
}

impl BusinessCapabilities {
    pub const CURRENT_FEATURE: &'static str = "codex.business.v1";
    pub const COMPACT_PAYLOAD_FEATURE: &'static str =
        "codex.business.transport.base64-payload.v1";
    pub const OPERATION_FEATURE_PREFIX: &'static str = "codex.business.operation.";

    pub fn baseline() -> Self {
        Self {
            protocol: ProtocolVersion::V1,
            features: [Self::CURRENT_FEATURE.to_owned()].into_iter().collect(),
        }
    }

    pub fn current() -> Self {
        let mut capabilities = Self::baseline();
        capabilities
            .features
            .insert(Self::COMPACT_PAYLOAD_FEATURE.to_owned());
        capabilities.features.extend(
            Operation::MONITORING
                .iter()
                .map(|operation| operation.capability_id().to_owned()),
        );
        capabilities
    }

    pub fn supports(&self, operation: &Operation) -> bool {
        self.features.contains(operation.capability_id())
    }

    pub fn negotiate(&self, remote: &Self) -> Result<Self, CapabilityNegotiationError> {
        if self.protocol.major != remote.protocol.major {
            return Err(CapabilityNegotiationError::IncompatibleMajor {
                local: self.protocol.major,
                remote: remote.protocol.major,
            });
        }
        let features: BTreeSet<String> = self
            .features
            .intersection(&remote.features)
            .cloned()
            .collect();
        if !features.contains(Self::CURRENT_FEATURE) {
            return Err(CapabilityNegotiationError::MissingRequiredFeature);
        }
        Ok(Self {
            protocol: ProtocolVersion {
                major: self.protocol.major,
                minor: self.protocol.minor.min(remote.protocol.minor),
            },
            features,
        })
    }
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
#[serde(rename_all = "camelCase")]
pub enum Operation {
    ProjectsList,
    StateSubscribe,
    EventsReplay,
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

impl Operation {
    /// Operations exposed by the monitoring-only product. Control operations
    /// remain wire-decodable for compatibility, but are never advertised.
    pub const MONITORING: &'static [Self] = &[
        Self::ProjectsList,
        Self::StateSubscribe,
        Self::EventsReplay,
        Self::ThreadsList,
        Self::ThreadsRead,
        Self::ApprovalsList,
    ];

    /// The wire-stable capability advertised for this operation. Keep these
    /// identifiers immutable: adding an operation must require both peers to
    /// advertise its capability, even when the protocol minor is unchanged.
    pub const ALL: &'static [Self] = &[
        Self::ProjectsList,
        Self::StateSubscribe,
        Self::EventsReplay,
        Self::ThreadsList,
        Self::ThreadsRead,
        Self::ThreadsCreate,
        Self::TurnsSend,
        Self::TurnsSteer,
        Self::TurnsInterrupt,
        Self::ApprovalsList,
        Self::ApprovalsResolve,
        Self::CommandsGet,
    ];

    pub const fn capability_id(&self) -> &'static str {
        match self {
            Self::ProjectsList => "codex.business.operation.projects-list.v1",
            Self::StateSubscribe => "codex.business.operation.state-subscribe.v1",
            Self::EventsReplay => "codex.business.operation.events-replay.v1",
            Self::ThreadsList => "codex.business.operation.threads-list.v1",
            Self::ThreadsRead => "codex.business.operation.threads-read.v1",
            Self::ThreadsCreate => "codex.business.operation.threads-create.v1",
            Self::TurnsSend => "codex.business.operation.turns-send.v1",
            Self::TurnsSteer => "codex.business.operation.turns-steer.v1",
            Self::TurnsInterrupt => "codex.business.operation.turns-interrupt.v1",
            Self::ApprovalsList => "codex.business.operation.approvals-list.v1",
            Self::ApprovalsResolve => "codex.business.operation.approvals-resolve.v1",
            Self::CommandsGet => "codex.business.operation.commands-get.v1",
        }
    }

    pub const fn is_monitoring(&self) -> bool {
        matches!(
            self,
            Self::ProjectsList
                | Self::StateSubscribe
                | Self::EventsReplay
                | Self::ThreadsList
                | Self::ThreadsRead
                | Self::ApprovalsList
        )
    }
}

/// Typed parameters for the stable business operations.  These are converted
/// to/from the envelope's JSON `params` at the transport boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RequestParams {
    ProjectsList {
        page: PageRequest,
    },
    StateSubscribe {
        after: Option<Cursor>,
    },
    EventsReplay {
        process_generation: Option<ProcessGeneration>,
        after: Option<Cursor>,
    },
    ThreadsList {
        page: PageRequest,
        project_id: Option<ProjectId>,
    },
    ThreadsRead {
        thread: ThreadKey,
        after: Option<Cursor>,
    },
    ApprovalsList {
        thread: Option<ThreadKey>,
        page: PageRequest,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ResponseResult {
    Projects(Page<Project>),
    Quota(AccountQuotaSnapshot),
    Subscription(StateSnapshot),
    EventsReplay(EventReplay),
    Threads(Page<Thread>),
    Thread(ThreadDetail),
    Approvals(Page<Approval>),
    Activities(ActivitySnapshot),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub devices: Vec<Device>,
    pub groups: Vec<Group>,
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    pub cursor: Cursor,
}

/// A bounded source-event replay used after a slow consumer observes a gap.
/// `complete` is false when the requested cursor predates the retained window.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventReplay {
    pub source_device_id: DeviceId,
    pub process_generation: ProcessGeneration,
    pub oldest_available: Option<Cursor>,
    pub newest_available: Option<Cursor>,
    pub complete: bool,
    pub events: Vec<EventEnvelope>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThreadDetail {
    pub thread: Thread,
    pub messages: Vec<Message>,
    pub activities: Vec<Activity>,
    pub next: Option<Cursor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActivitySnapshot {
    pub source_device_id: DeviceId,
    pub activities: Vec<Activity>,
    pub observed_at_unix_seconds: i64,
    pub next: Option<Cursor>,
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

/// Wire-level frame.  The envelopes retain source/request identifiers so a
/// receiver can route by `(device_id, thread_id)` and safely replay responses.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum BusinessFrame {
    Request(RequestEnvelope),
    Response(ResponseEnvelope),
    Event(EventEnvelope),
    MembershipUpdate(Vec<u8>),
    Ping { sequence: u64 },
    Pong { sequence: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Event {
    SnapshotRequired {
        oldest_available: Cursor,
    },
    Snapshot {
        devices: Vec<Device>,
        threads: Vec<Thread>,
    },
    ThreadChanged(Thread),
    TurnChanged(Turn),
    ApprovalChanged(Approval),
    QuotaChanged(AccountQuotaSnapshot),
    ActivitySnapshot(ActivitySnapshot),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProtocolError {
    UnsupportedProtocolVersion {
        supported: ProtocolVersion,
        actual: ProtocolVersion,
    },
    InvalidRequest {
        message: String,
    },
    UnauthorizedSource,
    WrongTarget,
    UnsupportedOperation {
        operation: Operation,
    },
    MissingCapability {
        operation: Operation,
        capability: String,
    },
    NotFound {
        resource: String,
    },
    SourceUnavailable,
    CursorExpired {
        oldest_available: Cursor,
    },
    StaleProcessGeneration {
        expected: ProcessGeneration,
        actual: ProcessGeneration,
    },
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
        self.newest = Cursor(self.newest.0.saturating_add(1));
        self.newest
    }
    pub fn retain_from(&mut self, oldest: Cursor) {
        self.oldest_available = oldest.min(Cursor(self.newest.0.saturating_add(1)));
    }
    pub fn check(&self, after: Option<Cursor>) -> CursorDecision {
        match after {
            None => CursorDecision::SnapshotRequired {
                oldest_available: self.oldest_available,
            },
            Some(cursor) if cursor.0.saturating_add(1) < self.oldest_available.0 => {
                CursorDecision::SnapshotRequired {
                    oldest_available: self.oldest_available,
                }
            }
            Some(_) => CursorDecision::Incremental,
        }
    }
}

pub fn decode<T: DeserializeOwned>(value: &Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_capabilities_advertise_only_monitoring_operations() {
        let capabilities = BusinessCapabilities::current();
        assert!(
            capabilities
                .features
                .contains(BusinessCapabilities::CURRENT_FEATURE)
        );
        assert!(
            capabilities
                .features
                .contains(BusinessCapabilities::COMPACT_PAYLOAD_FEATURE)
        );
        for operation in Operation::MONITORING {
            assert!(capabilities.supports(operation));
            assert!(
                operation
                    .capability_id()
                    .starts_with(BusinessCapabilities::OPERATION_FEATURE_PREFIX)
            );
        }
        for operation in Operation::ALL {
            assert_eq!(capabilities.supports(operation), operation.is_monitoring());
        }
    }

    #[test]
    fn negotiation_preserves_operation_subset() {
        let local = BusinessCapabilities::current();
        let current = local.negotiate(&BusinessCapabilities::current()).unwrap();
        assert!(current
            .features
            .contains(BusinessCapabilities::COMPACT_PAYLOAD_FEATURE));
        let mut remote = BusinessCapabilities::baseline();
        remote
            .features
            .insert(Operation::ProjectsList.capability_id().to_owned());
        let negotiated = local.negotiate(&remote).unwrap();
        assert!(negotiated.supports(&Operation::ProjectsList));
        assert!(!negotiated.supports(&Operation::ThreadsList));
        assert!(!negotiated
            .features
            .contains(BusinessCapabilities::COMPACT_PAYLOAD_FEATURE));
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
}
