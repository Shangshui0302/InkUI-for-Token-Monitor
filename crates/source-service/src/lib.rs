//! The desktop-side business service.
//!
//! This crate is intentionally transport-free: callers authenticate and check
//! the source/target device context before invoking [`SourceService::handle`].
//! The service performs the project-path and process-generation fences, then
//! delegates App Server work through [`AppServerBackend`].

use async_trait::async_trait;
use nekodex_appserver_adapter::{ApprovalRequest, Incoming, RequestId};
use nekodex_appserver_runtime::{RuntimeClient, RuntimeError};
use nekodex_protocol::{
    AccountQuotaSnapshot, Approval, ApprovalStatus, Cursor, EventEnvelope, EventReplay, Operation,
    Page, PageRequest, ProcessGeneration, Project, ProtocolError, ProtocolVersion, QuotaWindow,
    RequestEnvelope, ResponseEnvelope, ThreadKey,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

// Keep details compatible with legacy byte-array frames as well as the compact
// transport negotiated by current peers.
const THREAD_DETAIL_MAX_BYTES: usize = 12 * 1024;
const THREAD_ITEM_TEXT_MAX_CHARS: usize = 3_000;
const THREAD_DETAIL_TURN_LIMIT_DEFAULT: usize = 20;
const THREAD_DETAIL_TURN_LIMIT_MAX: usize = 20;
const THREAD_TURN_MAX_ITEMS: usize = 30;
const EVENT_REPLAY_MAX_EVENTS: usize = 64;
// Leaves room for the response envelope and Base64 expansion inside the
// authenticated 65,535-byte device transport frame.
const MONITORING_RESULT_MAX_BYTES: usize = 40 * 1024;
const EVENT_REPLAY_MAX_PAYLOAD_BYTES: usize = MONITORING_RESULT_MAX_BYTES;
const PAGE_PAYLOAD_MAX_BYTES: usize = MONITORING_RESULT_MAX_BYTES;
const THREAD_LIST_PAGE_LIMIT_MAX: u32 = 32;
const STATE_SNAPSHOT_MAX_BYTES: usize = MONITORING_RESULT_MAX_BYTES;
const USAGE_DAILY_BUCKET_LIMIT: usize = 366;
const APPROVAL_DESCRIPTION_MAX_CHARS: usize = 512;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid request parameters: {0}")]
    InvalidParams(String),
    #[error("source/target context mismatch")]
    ContextMismatch,
    #[error("request target is not found: {0}")]
    NotFound(String),
    #[error("cursor is expired; oldest available cursor is {0:?}")]
    CursorExpired(nekodex_protocol::Cursor),
    #[error("project {0:?} is not in the source allowlist")]
    UnknownProject(String),
    #[error("project path is not allowed: {0}")]
    ProjectPathNotAllowed(String),
    #[error("unsupported capability: {0}")]
    Unsupported(String),
    #[error("stale process generation: expected {expected:?}, actual {actual:?}")]
    StaleGeneration {
        expected: ProcessGeneration,
        actual: ProcessGeneration,
    },
    #[error("backend: {0}")]
    Backend(String),
    #[error("app-server rejected the request: {0}")]
    Rejected(String),
    #[error("monitoring state store: {0}")]
    Store(String),
    #[error("protocol error: {0:?}")]
    Protocol(ProtocolError),
}

/// The only App Server-facing interface the service needs. Production uses
/// [`RuntimeBackend`]; tests and embedding applications can supply a memory
/// implementation and never launch a Codex process.
#[async_trait]
pub trait AppServerBackend: Send + Sync {
    async fn account_rate_limits_read(&self) -> Result<Value, ServiceError>;
    async fn account_usage_read(&self) -> Result<Value, ServiceError>;
    async fn threads_list(
        &self,
        projects: Vec<Project>,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<Value, ServiceError>;
    async fn threads_read(
        &self,
        thread_id: &str,
        include_turns: bool,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Value, ServiceError>;
}

/// Deterministic backend for service tests and embedders that provide their
/// own App Server transport.
#[derive(Default)]
pub struct MemoryBackend {
    pub calls: Mutex<Vec<String>>,
    pub fail_rate_limits: AtomicBool,
    pub fail_usage: AtomicBool,
    pub thread_items: Mutex<Vec<Value>>,
    pub thread_limits: Mutex<Vec<Option<u32>>>,
    pub thread_detail: Mutex<Option<Value>>,
}

#[async_trait]
impl AppServerBackend for MemoryBackend {
    async fn account_rate_limits_read(&self) -> Result<Value, ServiceError> {
        self.calls
            .lock()
            .await
            .push("account/rateLimits/read".into());
        if self.fail_rate_limits.load(Ordering::Acquire) {
            return Err(ServiceError::Backend("rate limits unavailable".into()));
        }
        Ok(json!({
            "accountId": "memory-account",
            "rateLimits": {
                "limitId": "codex",
                "limitName": "Codex",
                "primary": null,
                "secondary": null
            },
            "rateLimitsByLimitId": null
        }))
    }
    async fn account_usage_read(&self) -> Result<Value, ServiceError> {
        self.calls.lock().await.push("account/usage/read".into());
        if self.fail_usage.load(Ordering::Acquire) {
            return Err(ServiceError::Backend("usage unavailable".into()));
        }
        Ok(json!({ "summary": {}, "dailyUsageBuckets": null }))
    }
    async fn threads_list(
        &self,
        projects: Vec<Project>,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<Value, ServiceError> {
        self.calls.lock().await.push("threads.list".into());
        self.thread_limits.lock().await.push(limit);
        let data = self.thread_items.lock().await.clone();
        Ok(json!({ "data": data, "nextCursor": null, "projects": projects, "cursor": cursor, "limit": limit }))
    }
    async fn threads_read(
        &self,
        thread_id: &str,
        include_turns: bool,
        _cursor: Option<String>,
        _limit: u32,
    ) -> Result<Value, ServiceError> {
        self.calls.lock().await.push("threads.read".into());
        if let Some(detail) = self.thread_detail.lock().await.clone() {
            return Ok(detail);
        }
        Ok(json!({
            "thread": {
                "id": thread_id,
                "turns": if include_turns { json!([]) } else { Value::Null }
            }
        }))
    }
}

/// Runtime-backed production adapter. The runtime actor owns request IDs and
/// serializes writes, while this wrapper supplies the current process fence.
#[derive(Clone)]
pub struct RuntimeBackend {
    client: RuntimeClient,
    generation: ProcessGeneration,
}

impl RuntimeBackend {
    pub fn new(client: RuntimeClient, generation: ProcessGeneration) -> Self {
        Self { client, generation }
    }
    pub fn generation(&self) -> ProcessGeneration {
        self.generation
    }
}

#[async_trait]
impl AppServerBackend for RuntimeBackend {
    async fn account_rate_limits_read(&self) -> Result<Value, ServiceError> {
        self.client
            .request(|adapter| adapter.account_rate_limits_read())
            .await
            .map_err(runtime_error)
    }
    async fn account_usage_read(&self) -> Result<Value, ServiceError> {
        self.client
            .request(|adapter| adapter.account_usage_read())
            .await
            .map_err(runtime_error)
    }
    async fn threads_list(
        &self,
        projects: Vec<Project>,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<Value, ServiceError> {
        let cwd = projects.into_iter().map(|project| project.path).collect();
        let params = nekodex_appserver_adapter::ThreadListParams {
            cursor,
            limit,
            cwd: Some(cwd),
            project_id: None,
        };
        self.client
            .request(move |adapter| adapter.thread_list(params))
            .await
            .map_err(runtime_error)
    }
    async fn threads_read(
        &self,
        thread_id: &str,
        include_turns: bool,
        cursor: Option<String>,
        limit: u32,
    ) -> Result<Value, ServiceError> {
        let metadata_thread_id = thread_id.to_owned();
        let metadata = self.client
            .request(move |adapter| adapter.thread_read(nekodex_appserver_adapter::ThreadReadParams {
                thread_id: metadata_thread_id,
                include_turns: Some(false),
            }))
            .await
            .map_err(runtime_error)?;
        if !include_turns {
            return Ok(metadata);
        }
        let turns_thread_id = thread_id.to_owned();
        let page = self.client
            .request(move |adapter| adapter.thread_turns_list(nekodex_appserver_adapter::ThreadTurnsListParams {
                thread_id: turns_thread_id,
                cursor,
                limit: Some(limit),
                sort_direction: Some("desc".into()),
                items_view: Some("summary".into()),
            }))
            .await;
        let page = match page {
            Ok(page) => page,
            Err(RuntimeError::ServerResponse { code: -32601, .. }) => {
                let fallback_thread_id = thread_id.to_owned();
                return self.client
                    .request(move |adapter| adapter.thread_read(nekodex_appserver_adapter::ThreadReadParams {
                        thread_id: fallback_thread_id,
                        include_turns: Some(true),
                    }))
                    .await
                    .map_err(runtime_error);
            }
            Err(error) => return Err(runtime_error(error)),
        };
        let mut result = metadata;
        let mut turns = page
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| ServiceError::Backend("thread/turns/list returned no data array".into()))?;
        let next_cursor = match page.get("nextCursor") {
            Some(Value::String(cursor)) if !cursor.is_empty() => Value::String(cursor.clone()),
            Some(Value::Null) => Value::Null,
            _ => return Err(ServiceError::Backend(
                "thread/turns/list returned an invalid nextCursor".into(),
            )),
        };
        turns.reverse();
        result["thread"]["turns"] = Value::Array(turns);
        result["_nekodexNextCursor"] = next_cursor;
        Ok(result)
    }
}

fn runtime_error(error: RuntimeError) -> ServiceError {
    match error {
        RuntimeError::ServerResponse {
            method,
            code,
            message,
            ..
        } => ServiceError::Rejected(format!(
            "request {method} failed with server error {code}: {message}"
        )),
        error => ServiceError::Backend(error.to_string()),
    }
}

#[derive(Clone)]
struct PendingApproval {
    domain: Approval,
}

struct CursorRegistry {
    next: u64,
    entries: BTreeMap<u64, CursorEntry>,
    capacity: usize,
}

struct CursorEntry {
    scope: Option<String>,
    value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ThreadDetailCursorContext {
    project_scope: String,
    thread_id: String,
    include_turns: bool,
    turn_limit: usize,
    project_epoch: u64,
}

#[derive(Default)]
struct ThreadDetailCursorRegistry {
    entries: BTreeMap<String, Vec<(ThreadDetailCursorContext, Option<String>)>>,
    capacity: usize,
}

impl ThreadDetailCursorRegistry {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            capacity: 512,
        }
    }

    fn register(
        &mut self,
        turn_id: String,
        context: ThreadDetailCursorContext,
        backend_cursor: Option<String>,
    ) {
        let contexts = self.entries.entry(turn_id).or_default();
        if !contexts.iter().any(|(existing, _)| existing == &context) {
            contexts.push((context, backend_cursor));
        }
        while self.entries.values().map(Vec::len).sum::<usize>() > self.capacity {
            let Some(key) = self.entries.keys().next().cloned() else {
                break;
            };
            let remove_key = self
                .entries
                .get_mut(&key)
                .map(|contexts| {
                    contexts.pop();
                    contexts.is_empty()
                })
                .unwrap_or(false);
            if remove_key {
                self.entries.remove(&key);
            }
        }
    }

    fn resolve(
        &self,
        turn_id: &str,
        context: &ThreadDetailCursorContext,
    ) -> Result<Option<String>, ServiceError> {
        let Some(contexts) = self.entries.get(turn_id) else {
            return Err(ServiceError::InvalidParams(
                "before_turn_id is unknown or expired".into(),
            ));
        };
        contexts
            .iter()
            .find(|(existing, _)| existing == context)
            .map(|(_, cursor)| cursor.clone())
            .ok_or_else(|| ServiceError::InvalidParams(
                "before_turn_id belongs to another thread detail context".into(),
            ))
    }
}

impl Default for CursorRegistry {
    fn default() -> Self {
        Self {
            next: 1,
            entries: BTreeMap::new(),
            capacity: 128,
        }
    }
}

impl CursorRegistry {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            ..Self::default()
        }
    }
    #[cfg(test)]
    fn resolve(&self, cursor: nekodex_protocol::Cursor) -> Result<String, ServiceError> {
        self.entries
            .get(&cursor.0)
            .filter(|entry| entry.scope.is_none())
            .map(|entry| entry.value.clone())
            .ok_or_else(|| {
            let oldest = self.entries.keys().next().copied().unwrap_or(self.next);
            ServiceError::CursorExpired(nekodex_protocol::Cursor(oldest))
        })
    }
    #[cfg(test)]
    fn register(&mut self, value: String) -> nekodex_protocol::Cursor {
        self.register_entry(None, value)
    }
    fn resolve_scoped(
        &self,
        cursor: nekodex_protocol::Cursor,
        scope: &str,
    ) -> Result<String, ServiceError> {
        let entry = self.entries.get(&cursor.0).ok_or_else(|| {
            let oldest = self.entries.keys().next().copied().unwrap_or(self.next);
            ServiceError::CursorExpired(nekodex_protocol::Cursor(oldest))
        })?;
        if entry.scope.as_deref() != Some(scope) {
            return Err(ServiceError::InvalidParams(
                "page.after belongs to another project scope".into(),
            ));
        }
        Ok(entry.value.clone())
    }
    fn register_scoped(&mut self, scope: &str, value: String) -> nekodex_protocol::Cursor {
        self.register_entry(Some(scope.to_owned()), value)
    }
    fn register_entry(
        &mut self,
        scope: Option<String>,
        value: String,
    ) -> nekodex_protocol::Cursor {
        if let Some((key, _)) = self
            .entries
            .iter()
            .find(|(_, current)| current.scope == scope && current.value == value)
        {
            return nekodex_protocol::Cursor(*key);
        }
        let key = self.next;
        self.next = self.next.saturating_add(1);
        self.entries.insert(key, CursorEntry { scope, value });
        while self.entries.len() > self.capacity {
            self.entries.pop_first();
        }
        nekodex_protocol::Cursor(key)
    }
}

pub struct SourceService<B> {
    source_device_id: String,
    generation: ProcessGeneration,
    catalog: RwLock<ProjectCatalog>,
    backend: Arc<B>,
    approvals: Mutex<BTreeMap<String, PendingApproval>>,
    cursors: Mutex<CursorRegistry>,
    approval_cursors: Mutex<CursorRegistry>,
    thread_detail_cursors: Mutex<ThreadDetailCursorRegistry>,
    allowed_threads: RwLock<BTreeSet<(String, String)>>,
    thread_scans: RwLock<BTreeMap<String, (Instant, Arc<Vec<Value>>, bool)>>,
    thread_scan_refresh: Mutex<()>,
    project_epoch: AtomicU64,
    event_replay: StdMutex<VecDeque<EventEnvelope>>,
    state_snapshot: RwLock<Option<(Instant, Value)>>,
    state_refresh: Mutex<()>,
    thread_status_approvals_enabled: AtomicBool,
}

struct ProjectCatalog {
    projects: BTreeMap<String, Project>,
    invalid_project_ids: BTreeSet<String>,
}

const EVENT_REPLAY_CAPACITY: usize = 1_000;
const STATE_SNAPSHOT_TTL: Duration = Duration::from_secs(15);
const THREAD_SCAN_PAGE_SIZE: u32 = 1_000;
const THREAD_SCAN_MAX_PAGES: usize = 20;
const THREAD_SCAN_CACHE_TTL: Duration = Duration::from_secs(2);
const THREAD_SCAN_CACHE_CAPACITY: usize = 4;
const ALLOWED_THREAD_CACHE_CAPACITY: usize = 20_000;
const BUSINESS_PAGE_LIMIT_MAX: u32 = 500;

impl<B> SourceService<B>
where
    B: AppServerBackend + 'static,
{
    pub fn new(
        local_device_id: impl Into<String>,
        generation: ProcessGeneration,
        projects: Vec<Project>,
        backend: Arc<B>,
    ) -> Self {
        let (projects, invalid_project_ids) = normalize_projects(projects);
        Self {
            source_device_id: local_device_id.into(),
            generation,
            catalog: RwLock::new(ProjectCatalog { projects, invalid_project_ids }),
            backend,
            approvals: Mutex::new(BTreeMap::new()),
            cursors: Mutex::new(CursorRegistry::default()),
            approval_cursors: Mutex::new(CursorRegistry::with_capacity(1_024)),
            thread_detail_cursors: Mutex::new(ThreadDetailCursorRegistry::new()),
            allowed_threads: RwLock::new(BTreeSet::new()),
            thread_scans: RwLock::new(BTreeMap::new()),
            thread_scan_refresh: Mutex::new(()),
            project_epoch: AtomicU64::new(0),
            event_replay: StdMutex::new(VecDeque::with_capacity(EVENT_REPLAY_CAPACITY)),
            state_snapshot: RwLock::new(None),
            state_refresh: Mutex::new(()),
            thread_status_approvals_enabled: AtomicBool::new(false),
        }
    }

    pub fn set_thread_status_approvals_enabled(&self, enabled: bool) {
        self.thread_status_approvals_enabled
            .store(enabled, Ordering::Release);
    }

    /// Retain the raw envelope before it is sent to subscribers. The bounded
    /// queue deliberately uses a synchronous mutex because `publish` is sync.
    pub fn record_event(&self, event: EventEnvelope) -> Result<(), ServiceError> {
        if event.source_device_id != self.source_device_id {
            return Err(ServiceError::ContextMismatch);
        }
        if event.process_generation != self.generation {
            return Err(ServiceError::StaleGeneration {
                expected: self.generation,
                actual: event.process_generation,
            });
        }
        let mut events = self
            .event_replay
            .lock()
            .map_err(|_| ServiceError::Store("event replay mutex poisoned".into()))?;
        if events.len() == EVENT_REPLAY_CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
        Ok(())
    }
    pub async fn replace_projects(&self, projects: Vec<Project>) {
        let (projects, invalid_project_ids) = normalize_projects(projects);
        *self.catalog.write().await = ProjectCatalog { projects, invalid_project_ids };
        self.project_epoch.fetch_add(1, Ordering::AcqRel);
        *self.state_snapshot.write().await = None;
        *self.cursors.lock().await = CursorRegistry::default();
        *self.approval_cursors.lock().await = CursorRegistry::with_capacity(1_024);
        *self.thread_detail_cursors.lock().await = ThreadDetailCursorRegistry::new();
        self.allowed_threads.write().await.clear();
        self.thread_scans.write().await.clear();
    }

    /// Apply App Server's `serverRequest/resolved` notification. It is emitted
    /// both when another path answers a request and when the server clears it
    /// because the owning turn ended, so an unanswered local projection must
    /// no longer remain actionable.
    pub async fn expire_resolved_request(&self, original_request_id: &str) -> Option<Approval> {
        let mut approvals = self.approvals.lock().await;
        let pending = approvals.values_mut().find(|pending| {
            pending.domain.original_request_id == original_request_id
                && pending.domain.status == ApprovalStatus::Pending
        })?;
        pending.domain.status = ApprovalStatus::Expired;
        Some(pending.domain.clone())
    }
    pub async fn handle(
        &self,
        peer_device_id: &str,
        request: RequestEnvelope,
    ) -> Result<ResponseEnvelope, ServiceError> {
        if request.version != ProtocolVersion::V1 {
            return Ok(ResponseEnvelope {
                version: ProtocolVersion::V1,
                request_id: request.request_id,
                result: None,
                error: Some(ProtocolError::UnsupportedProtocolVersion {
                    supported: ProtocolVersion::V1,
                    actual: request.version,
                }),
            });
        }
        if request.source_device_id != peer_device_id {
            return Ok(self.error_response(request, ProtocolError::UnauthorizedSource));
        }
        if request.target_device_id != self.source_device_id {
            return Ok(self.error_response(request, ProtocolError::WrongTarget));
        }
        if !request.operation.is_monitoring() {
            let operation = request.operation.clone();
            return Ok(self.error_response(
                request,
                ProtocolError::UnsupportedOperation { operation },
            ));
        }
        let project_epoch = self.project_epoch.load(Ordering::Acquire);
        let value = self.dispatch(&request).await;
        if operation_uses_projects(&request.operation)
            && self.project_epoch.load(Ordering::Acquire) != project_epoch
        {
            return Err(ServiceError::Backend(
                "project allowlist changed while serving the request".into(),
            ));
        }
        let response = match value {
            Ok(value) => ResponseEnvelope {
                version: request.version,
                request_id: request.request_id.clone(),
                result: Some(value),
                error: None,
            },
            Err(error) => {
                if let Some(protocol_error) = protocol_error(&error) {
                    let response = self.error_response(request.clone(), protocol_error.clone());
                    return Ok(response);
                }
                return Err(error);
            }
        };
        Ok(response)
    }
    fn error_response(&self, request: RequestEnvelope, error: ProtocolError) -> ResponseEnvelope {
        ResponseEnvelope {
            version: request.version,
            request_id: request.request_id,
            result: None,
            error: Some(error),
        }
    }
    async fn dispatch(&self, request: &RequestEnvelope) -> Result<Value, ServiceError> {
        match &request.operation {
            Operation::ProjectsList => {
                let p: ProjectsList = parse(request.params.clone())?;
                let mut projects: Vec<_> = self.catalog.read().await.projects.values().cloned().collect();
                projects.sort_by(|left, right| left.project_id.cmp(&right.project_id));

                let start = p.page.after.map(|cursor| cursor.0 as usize).unwrap_or(0);
                if p.page
                    .after
                    .is_some_and(|cursor| cursor.0 > usize::MAX as u64)
                    || start > projects.len()
                {
                    return Err(ServiceError::InvalidParams(
                        "page.after is outside the available project page".into(),
                    ));
                }
                if p.page.limit == Some(0) {
                    return Err(ServiceError::InvalidParams(
                        "page.limit must be greater than zero".into(),
                    ));
                }
                let limit = p
                    .page
                    .limit
                    .unwrap_or(BUSINESS_PAGE_LIMIT_MAX)
                    .min(BUSINESS_PAGE_LIMIT_MAX) as usize;
                let end = bounded_page_end(&projects, start, limit)?;
                let next = (end < projects.len()).then_some(Cursor(end as u64));
                serde_json::to_value(Page {
                    items: projects[start..end].to_vec(),
                    next,
                })
                .map_err(|error| ServiceError::Backend(error.to_string()))
            }
            Operation::StateSubscribe => {
                let _: StateSubscribe = parse(request.params.clone())?;
                if let Some((observed, snapshot)) = self.state_snapshot.read().await.as_ref() {
                    if observed.elapsed() < STATE_SNAPSHOT_TTL {
                        return Ok(snapshot.clone());
                    }
                }
                // One viewer refreshes the App Server snapshot while other
                // viewers wait and reuse the same result. Failed refreshes are
                // not cached, so callers can fall back to their durable cache.
                let _refresh = self.state_refresh.lock().await;
                if let Some((observed, snapshot)) = self.state_snapshot.read().await.as_ref() {
                    if observed.elapsed() < STATE_SNAPSHOT_TTL {
                        return Ok(snapshot.clone());
                    }
                }
                let (rate_limits, usage) = tokio::join!(
                    self.backend.account_rate_limits_read(),
                    self.backend.account_usage_read()
                );
                let observed_at_unix_seconds = observed_at_unix_seconds()?;
                let quota_error = rate_limits.as_ref().err().map(ToString::to_string);
                let quota = rate_limits.as_ref().ok().and_then(|rate_limits| {
                    quota_snapshot(
                        rate_limits,
                        &self.source_device_id,
                        observed_at_unix_seconds,
                    )
                });
                let usage_error = usage.as_ref().err().map(ToString::to_string);
                let usage = usage.as_ref().ok().map(project_usage);
                let projects: Vec<_> = self.catalog.read().await.projects.values().cloned().collect();
                let (thread_counts, threads_error) = if self.any_invalid_project(&projects).await
                {
                    (json!({
                        "running": 0,
                        "attention": 0,
                        "total": 0,
                        "truncated": false,
                        "available": false,
                    }), Some("项目白名单包含当前不可访问的目录".to_owned()))
                } else {
                    match self.scan_threads(projects).await {
                        Ok((threads, complete)) => (thread_counts(&threads, !complete), None),
                        Err(error) => (json!({
                            "running": 0,
                            "attention": 0,
                            "total": 0,
                            "truncated": false,
                            "available": false,
                        }), Some(error.to_string())),
                    }
                };
                let mut snapshot = json!({
                    "source_device_id": self.source_device_id,
                    "process_generation": self.generation,
                    "observed_at_unix_seconds": observed_at_unix_seconds,
                    "quota": quota,
                    "raw_usage": usage,
                    "thread_counts": thread_counts,
                    "errors": {
                        "quota": quota_error,
                        "usage": usage_error,
                        "threads": threads_error,
                    },
                });
                if serde_json::to_vec(&snapshot)
                    .map_err(|error| ServiceError::Backend(error.to_string()))?
                    .len()
                    > STATE_SNAPSHOT_MAX_BYTES
                {
                    snapshot["raw_usage"] = Value::Null;
                    snapshot["errors"]["usage"] = Value::String(
                        "日用量因监控快照超过传输预算而省略".into(),
                    );
                }
                if serde_json::to_vec(&snapshot)
                    .map_err(|error| ServiceError::Backend(error.to_string()))?
                    .len()
                    > STATE_SNAPSHOT_MAX_BYTES
                {
                    return Err(ServiceError::Backend(
                        "source snapshot exceeds the monitoring transport budget".into(),
                    ));
                }
                *self.state_snapshot.write().await = Some((Instant::now(), snapshot.clone()));
                Ok(snapshot)
            }
            Operation::EventsReplay => {
                let p: EventsReplayParams = parse(request.params.clone())?;
                let events = self
                    .event_replay
                    .lock()
                    .map_err(|_| ServiceError::Store("event replay mutex poisoned".into()))?;
                let oldest_available = events.front().map(|event| event.cursor);
                let newest_available = events.back().map(|event| event.cursor);
                let generation_matches = p
                    .process_generation
                    .is_none_or(|generation| generation == self.generation);
                let complete = match (generation_matches, p.after, oldest_available) {
                    (_, _, None) => true,
                    (true, Some(after), Some(oldest)) => after.0.saturating_add(1) >= oldest.0,
                    (_, _, Some(oldest)) => oldest.0 <= 1,
                };
                let candidates: Vec<_> = if generation_matches {
                    events
                        .iter()
                        .filter(|event| p.after.is_none_or(|after| event.cursor.0 > after.0))
                        .cloned()
                        .collect()
                } else {
                    events.iter().cloned().collect()
                };
                let mut replay = Vec::new();
                let mut replay_bytes: usize = 0;
                for event in candidates {
                    let event_bytes = serde_json::to_vec(&event)
                        .map_err(|error| ServiceError::Backend(error.to_string()))?
                        .len();
                    if !replay.is_empty()
                        && (replay.len() >= EVENT_REPLAY_MAX_EVENTS
                            || replay_bytes.saturating_add(event_bytes)
                                > EVENT_REPLAY_MAX_PAYLOAD_BYTES)
                    {
                        break;
                    }
                    if event_bytes > EVENT_REPLAY_MAX_PAYLOAD_BYTES {
                        return Err(ServiceError::Backend(
                            "one source event exceeds the replay transport budget".into(),
                        ));
                    }
                    replay_bytes = replay_bytes.saturating_add(event_bytes);
                    replay.push(event);
                }
                serde_json::to_value(EventReplay {
                    source_device_id: self.source_device_id.clone(),
                    process_generation: self.generation,
                    oldest_available,
                    newest_available,
                    complete,
                    events: replay,
                })
                .map_err(|error| ServiceError::Backend(error.to_string()))
            }
            Operation::ThreadsList => {
                let p: ThreadsList = parse(request.params.clone())?;
                if p.page.limit == Some(0) {
                    return Err(ServiceError::InvalidParams(
                        "page.limit must be greater than zero".into(),
                    ));
                }
                let limit = Some(
                    p.page
                        .limit
                        .unwrap_or(50)
                        .min(THREAD_LIST_PAGE_LIMIT_MAX),
                );
                let projects = if let Some(project_id) = p.project_id {
                    vec![self.project(&project_id).await?]
                } else {
                    let projects: Vec<_> = self.catalog.read().await.projects.values().cloned().collect();
                    for project in &projects {
                        self.validate_project_path(project).await?;
                    }
                    projects
                };
                let project_scope = project_scope(&projects);
                let upstream_cursor = match p.page.after {
                    Some(cursor) => Some(
                        self.cursors
                            .lock()
                            .await
                            .resolve_scoped(cursor, &project_scope)?,
                    ),
                    None => None,
                };
                if projects.is_empty() {
                    return Ok(json!({
                        "items": [],
                        "next": null,
                    }));
                }
                let raw = self
                    .backend
                    .threads_list(projects, upstream_cursor, limit)
                    .await?;
                let (items, upstream_next) = parse_thread_page(&raw, limit.unwrap_or(50) as usize)?;
                let items: Vec<_> = items
                    .into_iter()
                    .filter_map(project_thread_summary)
                    .collect();
                if serde_json::to_vec(&items)
                    .map_err(|error| ServiceError::Backend(error.to_string()))?
                    .len()
                    > PAGE_PAYLOAD_MAX_BYTES
                {
                    return Err(ServiceError::Backend(
                        "thread page exceeds the monitoring transport budget".into(),
                    ));
                }
                let next = match upstream_next {
                    Some(cursor) => Some(
                        self.cursors
                            .lock()
                            .await
                            .register_scoped(&project_scope, cursor),
                    ),
                    None => None,
                };
                self.remember_allowed_threads(&project_scope, &items).await;
                Ok(json!({ "items": items, "next": next }))
            }
            Operation::ThreadsRead => {
                let p: ThreadsRead = parse(request.params.clone())?;
                if p.thread_id.trim().is_empty() {
                    return Err(ServiceError::InvalidParams(
                        "thread_id cannot be empty".into(),
                    ));
                }
                if p
                    .before_turn_id
                    .as_deref()
                    .is_some_and(|before_turn_id| before_turn_id.is_empty())
                {
                    return Err(ServiceError::InvalidParams(
                        "before_turn_id cannot be empty".into(),
                    ));
                }
                if p.turn_limit == Some(0) {
                    return Err(ServiceError::InvalidParams(
                        "turn_limit must be greater than zero".into(),
                    ));
                }
                let turn_limit = p
                    .turn_limit
                    .map(|limit| limit as usize)
                    .unwrap_or(THREAD_DETAIL_TURN_LIMIT_DEFAULT)
                    .min(THREAD_DETAIL_TURN_LIMIT_MAX);
                let projects = if let Some(project_id) = p.project_id {
                    vec![self.project(&project_id).await?]
                } else {
                    let projects: Vec<_> = self.catalog.read().await.projects.values().cloned().collect();
                    for project in &projects {
                        self.validate_project_path(project).await?;
                    }
                    projects
                };
                if projects.is_empty() {
                    return Err(ServiceError::NotFound(format!("thread {}", p.thread_id)));
                }
                let project_scope = project_scope(&projects);
                let mut allowed = self
                    .allowed_threads
                    .read()
                    .await
                    .contains(&(project_scope.clone(), p.thread_id.clone()));
                let mut complete = true;
                if !allowed {
                    let (threads, scan_complete) = self.scan_threads(projects).await?;
                    complete = scan_complete;
                    self.remember_allowed_threads(&project_scope, &threads).await;
                    allowed = threads.iter().any(|thread| {
                        thread.get("id").and_then(Value::as_str) == Some(p.thread_id.as_str())
                    });
                }
                if !allowed {
                    if !complete {
                        return Err(ServiceError::Backend(
                            "thread allowlist scan reached its bounded limit".into(),
                        ));
                    }
                    return Err(ServiceError::NotFound(format!("thread {}", p.thread_id)));
                }
                let cursor_context = ThreadDetailCursorContext {
                    project_scope,
                    thread_id: p.thread_id.clone(),
                    include_turns: p.include_turns,
                    turn_limit,
                    project_epoch: self.project_epoch.load(Ordering::Acquire),
                };
                let backend_cursor = if let Some(before_turn_id) = p.before_turn_id.as_deref() {
                    self.thread_detail_cursors
                        .lock()
                        .await
                        .resolve(before_turn_id, &cursor_context)?
                } else {
                    None
                };
                let detail = self.backend.threads_read(
                    &p.thread_id,
                    p.include_turns,
                    backend_cursor.clone(),
                    turn_limit as u32,
                ).await?;
                let next_backend_cursor = detail
                    .get("_nekodexNextCursor")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let detail = bounded_thread_detail_page(
                    detail,
                    if backend_cursor.is_some() { None } else { p.before_turn_id.as_deref() },
                    turn_limit,
                )?;
                if let Some(next_before_turn_id) = detail
                    .get("next_before_turn_id")
                    .and_then(Value::as_str)
                {
                    self.thread_detail_cursors
                        .lock()
                        .await
                        .register(next_before_turn_id.to_owned(), cursor_context, next_backend_cursor);
                }
                Ok(detail)
            }
            Operation::ApprovalsList => {
                let p: ApprovalList = parse(request.params.clone())?;
                let approval_scope = approval_scope(p.thread.as_ref());
                if p.page.as_ref().and_then(|page| page.limit) == Some(0) {
                    return Err(ServiceError::InvalidParams(
                        "page.limit must be greater than zero".into(),
                    ));
                }
                let limit = p
                    .page
                    .as_ref()
                    .and_then(|page| page.limit)
                    .unwrap_or(100)
                    .min(BUSINESS_PAGE_LIMIT_MAX) as usize;
                let mut items: Vec<_> = self
                    .approvals
                    .lock()
                    .await
                    .values()
                    .filter(|pending| pending.domain.status == ApprovalStatus::Pending)
                    .map(|pending| pending.domain.clone())
                    .filter(|approval| {
                        p.thread
                            .as_ref()
                            .is_none_or(|thread| &approval.thread == thread)
                    })
                    .collect();
                let projects: Vec<_> = self.catalog.read().await.projects.values().cloned().collect();
                if self
                    .thread_status_approvals_enabled
                    .load(Ordering::Acquire)
                    && !self.any_invalid_project(&projects).await
                {
                    {
                        let (threads, complete) = self.scan_threads(projects).await?;
                        let detailed_threads: std::collections::BTreeSet<_> = items
                            .iter()
                            .map(|approval| approval.thread.thread_id.clone())
                            .collect();
                        for approval in waiting_approval_threads(
                            &threads,
                            &self.source_device_id,
                            self.generation,
                        ) {
                            if detailed_threads.contains(&approval.thread.thread_id) {
                                continue;
                            }
                            if p.thread
                                .as_ref()
                                .is_none_or(|thread| &approval.thread == thread)
                            {
                                items.push(approval);
                            }
                        }
                        return self
                            .approval_page(
                                items,
                                p.page.as_ref(),
                                limit,
                                !complete,
                                &approval_scope,
                            )
                            .await;
                    }
                }
                self.approval_page(items, p.page.as_ref(), limit, false, &approval_scope)
                    .await
            }
            _ => Err(ServiceError::Unsupported(format!(
                "operation {:?} is not implemented by source service",
                request.operation
            ))),
        }
    }
    async fn scan_threads(
        &self,
        projects: Vec<Project>,
    ) -> Result<(Arc<Vec<Value>>, bool), ServiceError> {
        if projects.is_empty() {
            return Ok((Arc::new(Vec::new()), true));
        }
        let scope = project_scope(&projects);
        if let Some((observed, threads, complete)) = self.thread_scans.read().await.get(&scope) {
            if observed.elapsed() < THREAD_SCAN_CACHE_TTL {
                return Ok((Arc::clone(threads), *complete));
            }
        }
        let _refresh = self.thread_scan_refresh.lock().await;
        if let Some((observed, threads, complete)) = self.thread_scans.read().await.get(&scope) {
            if observed.elapsed() < THREAD_SCAN_CACHE_TTL {
                return Ok((Arc::clone(threads), *complete));
            }
        }
        let (threads, complete) = self.scan_threads_uncached(projects).await?;
        let threads = Arc::new(threads);
        let mut scans = self.thread_scans.write().await;
        scans.retain(|_, (observed, _, _)| observed.elapsed() < THREAD_SCAN_CACHE_TTL);
        scans.insert(scope, (Instant::now(), Arc::clone(&threads), complete));
        while scans.len() > THREAD_SCAN_CACHE_CAPACITY {
            let Some(oldest) = scans
                .iter()
                .min_by_key(|(_, (observed, _, _))| *observed)
                .map(|(scope, _)| scope.clone())
            else {
                break;
            };
            scans.remove(&oldest);
        }
        Ok((threads, complete))
    }

    async fn scan_threads_uncached(
        &self,
        projects: Vec<Project>,
    ) -> Result<(Vec<Value>, bool), ServiceError> {
        let mut cursor = None;
        let mut seen = std::collections::BTreeSet::new();
        let mut threads = Vec::new();
        for _ in 0..THREAD_SCAN_MAX_PAGES {
            let raw = self
                .backend
                .threads_list(projects.clone(), cursor.clone(), Some(THREAD_SCAN_PAGE_SIZE))
                .await?;
            let (page, next) = parse_thread_page(&raw, THREAD_SCAN_PAGE_SIZE as usize)?;
            threads.extend(page);
            let Some(next) = next else {
                return Ok((threads, true));
            };
            if !seen.insert(next.clone()) {
                return Err(ServiceError::Backend(
                    "threads.list returned a repeated nextCursor".into(),
                ));
            }
            cursor = Some(next);
        }
        Ok((threads, false))
    }
    async fn remember_allowed_threads(&self, project_scope: &str, threads: &[Value]) {
        let mut allowed = self.allowed_threads.write().await;
        allowed.extend(threads.iter().filter_map(|thread| {
            thread
                .get("id")
                .and_then(Value::as_str)
                .map(|thread_id| (project_scope.to_owned(), thread_id.to_owned()))
        }));
        while allowed.len() > ALLOWED_THREAD_CACHE_CAPACITY {
            allowed.pop_first();
        }
    }
    async fn approval_page(
        &self,
        mut items: Vec<Approval>,
        page: Option<&PageRequest>,
        limit: usize,
        incomplete: bool,
        scope: &str,
    ) -> Result<Value, ServiceError> {
        items.sort_by(|left, right| left.approval_id.cmp(&right.approval_id));
        let after_id = match page.and_then(|page| page.after) {
            Some(cursor) => Some(
                self.approval_cursors
                    .lock()
                    .await
                    .resolve_scoped(cursor, scope)?,
            ),
            None => None,
        };
        let start = after_id.as_ref().map_or(0, |after_id| {
            items.partition_point(|approval| approval.approval_id.as_str() <= after_id.as_str())
        });
        let end = bounded_page_end(&items, start, limit)?;
        let next = if end < items.len() {
            Some(
                self.approval_cursors
                    .lock()
                    .await
                    .register_scoped(scope, items[end - 1].approval_id.clone()),
            )
        } else {
            None
        };
        Ok(json!({
            "items": items[start..end].to_vec(),
            "next": next,
            "incomplete": incomplete,
        }))
    }
    async fn project(&self, id: &str) -> Result<Project, ServiceError> {
        let project = self
            .catalog
            .read()
            .await
            .projects
            .get(id)
            .cloned()
            .ok_or_else(|| ServiceError::UnknownProject(id.into()))?;
        self.validate_project_path(&project).await?;
        Ok(project)
    }
    async fn validate_project_path(&self, project: &Project) -> Result<(), ServiceError> {
        if self.catalog.read().await.invalid_project_ids.contains(&project.project_id)
        {
            Err(ServiceError::ProjectPathNotAllowed(project.path.clone()))
        } else {
            Ok(())
        }
    }

    async fn any_invalid_project(&self, projects: &[Project]) -> bool {
        let catalog = self.catalog.read().await;
        projects.iter().any(|project| catalog.invalid_project_ids.contains(&project.project_id))
    }
    pub async fn ingest(&self, incoming: Incoming) -> Result<Option<Approval>, ServiceError> {
        let Incoming::Approval(request) = incoming else {
            return Ok(None);
        };
        let (thread_id, turn_id, original_request_id) = match &request {
            ApprovalRequest::Command { id, context, .. }
            | ApprovalRequest::FileChange { id, context, .. } => (
                context.thread_id.clone(),
                context.turn_id.clone(),
                request_id_string(id),
            ),
        };
        let approval_id = original_request_id.clone();
        let description: String = match &request {
            ApprovalRequest::Command { command, cwd, .. } => {
                format!("command {:?} in {:?}", command, cwd)
            }
            ApprovalRequest::FileChange { grant_root, .. } => {
                format!("file change under {:?}", grant_root)
            }
        }
        .chars()
        .take(APPROVAL_DESCRIPTION_MAX_CHARS)
        .collect();
        let domain = Approval {
            approval_id: approval_id.clone(),
            thread: nekodex_protocol::ThreadKey::new(self.source_device_id.clone(), thread_id),
            turn_id,
            source_process_generation: self.generation,
            original_request_id,
            description,
            // Kept as an empty wire-compatibility field for older viewers.
            // This monitoring-only source never advertises approval actions.
            options: Vec::new(),
            status: ApprovalStatus::Pending,
        };
        self.approvals.lock().await.insert(
            approval_id,
            PendingApproval {
                domain: domain.clone(),
            },
        );
        Ok(Some(domain))
    }
}

fn request_id_string(id: &RequestId) -> String {
    match id {
        RequestId::Number(value) => value.to_string(),
        RequestId::String(value) => value.clone(),
    }
}
fn observed_at_unix_seconds() -> Result<i64, ServiceError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            ServiceError::Backend(format!("system clock is before Unix epoch: {error}"))
        })?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| ServiceError::Backend("observed time exceeds i64 range".into()))
}

fn quota_snapshot(
    rate_limits: &Value,
    source_device_id: &str,
    observed_at_unix_seconds: i64,
) -> Option<AccountQuotaSnapshot> {
    let account_id = rate_limits
        .get("accountId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())?
        .to_owned();
    let mut windows = Vec::new();
    if let Some(by_limit_id) = rate_limits
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
    {
        for (limit_id, snapshot) in by_limit_id {
            append_quota_windows(&mut windows, limit_id, snapshot);
        }
    }
    if windows.is_empty() {
        if let Some(snapshot) = rate_limits.get("rateLimits") {
            let limit_id = snapshot
                .get("limitId")
                .and_then(Value::as_str)
                .unwrap_or("default");
            append_quota_windows(&mut windows, limit_id, snapshot);
        }
    }
    windows.sort_by(|left, right| left.name.cmp(&right.name));
    Some(AccountQuotaSnapshot {
        account_id,
        source_device_id: source_device_id.to_owned(),
        windows,
        observed_at_unix_seconds,
    })
}

fn append_quota_windows(windows: &mut Vec<QuotaWindow>, limit_id: &str, snapshot: &Value) {
    let name = snapshot
        .get("limitName")
        .and_then(Value::as_str)
        .or_else(|| snapshot.get("limitId").and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .unwrap_or(limit_id);
    for (window_name, field) in [("primary", "primary"), ("secondary", "secondary")] {
        let Some(window) = snapshot.get(field).filter(|value| value.is_object()) else {
            continue;
        };
        windows.push(QuotaWindow {
            name: format!("{name}:{window_name}"),
            remaining_percent: window.get("usedPercent").and_then(remaining_percent),
            limit: None,
            used: None,
            resets_at_unix_seconds: window.get("resetsAt").and_then(signed_integer),
        });
    }
}

fn remaining_percent(value: &Value) -> Option<f64> {
    let used = value.as_f64()?;
    if !used.is_finite() || !(0.0..=100.0).contains(&used) {
        return None;
    }
    Some(100.0 - used)
}

fn signed_integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
}

/// Summarize a bounded, possibly multi-page threads.list scan for the overview.
fn thread_counts(threads: &[Value], truncated: bool) -> Value {
    let (running, attention, total, live_status_available) = {
        let mut running = 0;
        let mut attention = 0;
        let mut live_status_available = threads.is_empty();
        for thread in threads {
            let status = thread.get("status");
            let status_type = status
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str);
            if status_type.is_some_and(|status| status != "notLoaded") {
                live_status_available = true;
            }
            if status_type == Some("active") {
                running += 1;
            }
            let waiting = status
                .and_then(|value| value.get("activeFlags"))
                .and_then(Value::as_array)
                .is_some_and(|flags| {
                    flags
                        .iter()
                        .any(|flag| flag.as_str() == Some("waitingOnApproval"))
                });
            if status_type == Some("systemError") || waiting {
                attention += 1;
            }
        }
        (running, attention, threads.len(), live_status_available)
    };
    json!({
        "running": running,
        "attention": attention,
        "total": total,
        "truncated": truncated,
        "available": live_status_available,
    })
}

fn waiting_approval_threads(
    threads: &[Value],
    source_device_id: &str,
    generation: ProcessGeneration,
) -> Vec<Approval> {
    threads
        .iter()
        .filter(|thread| {
            thread
                .get("status")
                .and_then(|status| status.get("activeFlags"))
                .and_then(Value::as_array)
                .is_some_and(|flags| {
                    flags
                        .iter()
                        .any(|flag| flag.as_str() == Some("waitingOnApproval"))
                })
        })
        .filter_map(|thread| thread.get("id").and_then(Value::as_str))
        .map(|thread_id| Approval {
            approval_id: format!("waiting:{thread_id}"),
            thread: ThreadKey::new(source_device_id, thread_id),
            turn_id: String::new(),
            source_process_generation: generation,
            original_request_id: format!("waiting:{thread_id}"),
            description: "会话正在等待来源端审批；详细请求仅对发起该会话的客户端可见".into(),
            options: Vec::new(),
            status: ApprovalStatus::Pending,
        })
        .collect()
}

fn protocol_error(error: &ServiceError) -> Option<ProtocolError> {
    Some(match error {
        ServiceError::InvalidParams(message) => ProtocolError::InvalidRequest {
            message: message.clone(),
        },
        ServiceError::ContextMismatch => ProtocolError::WrongTarget,
        ServiceError::UnknownProject(id) => ProtocolError::NotFound {
            resource: format!("project {id}"),
        },
        ServiceError::ProjectPathNotAllowed(path) => ProtocolError::InvalidRequest {
            message: format!("project path is not allowed: {path}"),
        },
        ServiceError::Unsupported(operation) => ProtocolError::InvalidRequest {
            message: format!("unsupported capability: {operation}"),
        },
        ServiceError::Rejected(message) => ProtocolError::InvalidRequest {
            message: message.clone(),
        },
        ServiceError::StaleGeneration { expected, actual } => {
            ProtocolError::StaleProcessGeneration {
                expected: *expected,
                actual: *actual,
            }
        }
        ServiceError::NotFound(resource) => ProtocolError::NotFound {
            resource: resource.clone(),
        },
        ServiceError::CursorExpired(oldest_available) => ProtocolError::CursorExpired {
            oldest_available: *oldest_available,
        },
        ServiceError::Protocol(error) => error.clone(),
        ServiceError::Backend(_) | ServiceError::Store(_) => return None,
    })
}
fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ServiceError> {
    serde_json::from_value(value).map_err(|e| ServiceError::InvalidParams(e.to_string()))
}
fn parse_thread_page(
    raw: &Value,
    limit: usize,
) -> Result<(Vec<Value>, Option<String>), ServiceError> {
    let items = raw
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ServiceError::Backend("threads.list returned a non-array data field".into()))?;
    if items.len() > limit {
        return Err(ServiceError::Backend(format!(
            "threads.list returned {} items for a page limited to {limit}",
            items.len()
        )));
    }
    let next = match raw.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) => Some(cursor.clone()),
        Some(_) => {
            return Err(ServiceError::Backend(
                "threads.list returned a non-string nextCursor".into(),
            ));
        }
    };
    Ok((items.clone(), next))
}

fn project_scope(projects: &[Project]) -> String {
    let mut entries = projects
        .iter()
        .map(|project| format!("{}\0{}", project.project_id, project_path_key(&project.path)))
        .collect::<Vec<_>>();
    entries.sort_unstable();
    entries.join("\u{1}")
}

fn normalize_projects(projects: Vec<Project>) -> (BTreeMap<String, Project>, BTreeSet<String>) {
    let mut normalized = BTreeMap::new();
    let mut invalid = BTreeSet::new();
    for mut project in projects {
        match canonical_project_path(&project.path) {
            Ok(path) => project.path = path.to_string_lossy().into_owned(),
            Err(()) => {
                invalid.insert(project.project_id.clone());
            }
        }
        normalized.insert(project.project_id.clone(), project);
    }
    (normalized, invalid)
}

fn canonical_project_path(path: &str) -> Result<std::path::PathBuf, ()> {
    let candidate = std::path::Path::new(path);
    if path.is_empty() || !candidate.is_absolute() {
        return Err(());
    }
    let canonical = std::fs::canonicalize(candidate).map_err(|_| ())?;
    if !canonical.is_dir() {
        return Err(());
    }
    Ok(normalize_windows_prefix(canonical))
}

fn project_path_key(path: &str) -> String {
    let value = std::path::Path::new(path)
        .to_string_lossy()
        .replace('\\', "/");
    #[cfg(windows)]
    {
        value.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}

fn normalize_windows_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return std::path::PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(drive_path) = value.strip_prefix(r"\\?\") {
            return std::path::PathBuf::from(drive_path);
        }
    }
    path
}

fn operation_uses_projects(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::ProjectsList
            | Operation::StateSubscribe
            | Operation::ThreadsList
            | Operation::ThreadsRead
            | Operation::ApprovalsList
    )
}

fn approval_scope(thread: Option<&ThreadKey>) -> String {
    match thread {
        Some(thread) => format!(
            "thread:{}:{}:{}:{}",
            thread.device_id.len(),
            thread.device_id,
            thread.thread_id.len(),
            thread.thread_id,
        ),
        None => "all".to_owned(),
    }
}

#[cfg(test)]
fn bounded_thread_detail(raw: Value) -> Result<Value, ServiceError> {
    bounded_thread_detail_page(raw, None, THREAD_DETAIL_TURN_LIMIT_DEFAULT)
}

fn bounded_thread_detail_page(
    raw: Value,
    before_turn_id: Option<&str>,
    turn_limit: usize,
) -> Result<Value, ServiceError> {
    let source = raw
        .get("thread")
        .and_then(Value::as_object)
        .ok_or_else(|| ServiceError::Backend("thread/read returned no thread object".into()))?;
    let mut truncated = false;
    let mut thread = serde_json::Map::new();
    copy_field(source, &mut thread, "id");
    copy_limited_string(source, &mut thread, "preview", 256, &mut truncated);
    copy_field(source, &mut thread, "status");

    let mut page_start = 0;
    let mut turns_have_ids = true;
    if let Some(turns) = source.get("turns").and_then(Value::as_array) {
        let mut turn_ids = BTreeSet::new();
        for turn in turns {
            let id = turn.get("id").and_then(Value::as_str).filter(|id| !id.is_empty());
            if let Some(id) = id {
                if !turn_ids.insert(id.to_owned()) {
                    return Err(ServiceError::Backend(
                        "thread/read returned duplicate turn IDs".into(),
                    ));
                }
            } else {
                turns_have_ids = false;
            }
        }
        let page_end = if let Some(before_turn_id) = before_turn_id {
            let end = turns
                .iter()
                .position(|turn| turn.get("id").and_then(Value::as_str) == Some(before_turn_id))
                .ok_or_else(|| {
                    ServiceError::InvalidParams(
                        "before_turn_id is not present in this thread detail".into(),
                    )
                })?;
            if !turns_have_ids {
                return Err(ServiceError::Backend(
                    "thread/read returned a turn without an ID".into(),
                ));
            }
            end
        } else {
            turns.len()
        };
        page_start = page_end.saturating_sub(turn_limit);
        if page_start > 0 && !turns_have_ids {
            return Err(ServiceError::Backend(
                "thread/read returned a turn without an ID; pagination cannot continue".into(),
            ));
        }
        truncated |= page_start > 0 || page_end < turns.len();
        let projected = turns[page_start..page_end]
            .iter()
            .filter_map(|turn| project_turn(turn, &mut truncated))
            .collect();
        thread.insert("turns".into(), Value::Array(projected));
    }

    let mut result = json!({
        "thread": thread,
        "truncated": truncated,
        "next_before_turn_id": Value::Null,
    });
    let upstream_has_more = raw
        .get("_nekodexNextCursor")
        .and_then(Value::as_str)
        .is_some();
    while serde_json::to_vec(&result)
        .map_err(|error| ServiceError::Backend(error.to_string()))?
        .len()
        > THREAD_DETAIL_MAX_BYTES
    {
        let turns = result
            .pointer_mut("/thread/turns")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| ServiceError::Backend("thread detail exceeds transport limit".into()))?;
        if turns.len() > 1 {
            turns.remove(0);
            result["truncated"] = Value::Bool(true);
            continue;
        }
        let items = turns
            .first_mut()
            .and_then(|turn| turn.get_mut("items"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| ServiceError::Backend("thread detail exceeds transport limit".into()))?;
        if items.len() > 1 {
            items.remove(0);
            result["truncated"] = Value::Bool(true);
            continue;
        }
        return Err(ServiceError::Backend(
            "thread detail cannot fit the monitoring transport frame".into(),
        ));
    }
    if (page_start > 0 || upstream_has_more) && turns_have_ids {
        let next_before_turn_id = result
            .pointer("/thread/turns")
            .and_then(Value::as_array)
            .and_then(|turns| turns.first())
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        result["next_before_turn_id"] = next_before_turn_id
            .map_or(Value::Null, |id| Value::String(id.to_owned()));
    }
    Ok(result)
}

fn project_turn(value: &Value, truncated: &mut bool) -> Option<Value> {
    let source = value.as_object()?;
    let mut turn = serde_json::Map::new();
    copy_field(source, &mut turn, "id");
    copy_field(source, &mut turn, "status");
    if let Some(items) = source.get("items").and_then(Value::as_array) {
        let start = items.len().saturating_sub(THREAD_TURN_MAX_ITEMS);
        *truncated |= start > 0;
        let projected = items[start..]
            .iter()
            .filter_map(|item| project_thread_item(item, truncated))
            .collect();
        turn.insert("items".into(), Value::Array(projected));
    }
    Some(Value::Object(turn))
}

fn project_thread_summary(value: Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut summary = serde_json::Map::new();
    copy_field(source, &mut summary, "id");
    let mut ignored = false;
    copy_limited_string(source, &mut summary, "name", 64, &mut ignored);
    copy_limited_string(source, &mut summary, "preview", 128, &mut ignored);
    copy_field(source, &mut summary, "status");
    copy_field(source, &mut summary, "managed");
    Some(Value::Object(summary))
}

fn project_usage(raw: &Value) -> Value {
    let buckets = raw
        .get("dailyUsageBuckets")
        .and_then(Value::as_array)
        .map(|buckets| {
            let start = buckets.len().saturating_sub(USAGE_DAILY_BUCKET_LIMIT);
            buckets[start..]
                .iter()
                .filter_map(|bucket| {
                    let start_date = bucket.get("startDate")?.as_str()?;
                    let tokens = bucket.get("tokens")?;
                    if !tokens.is_number() {
                        return None;
                    }
                    Some(json!({
                        "startDate": start_date.chars().take(32).collect::<String>(),
                        "tokens": tokens,
                    }))
                })
                .collect::<Vec<_>>()
        });
    json!({ "dailyUsageBuckets": buckets })
}

fn project_thread_item(value: &Value, truncated: &mut bool) -> Option<Value> {
    let source = value.as_object()?;
    let mut item = serde_json::Map::new();
    copy_field(source, &mut item, "id");
    copy_field(source, &mut item, "type");
    copy_field(source, &mut item, "status");
    let mut remaining = THREAD_ITEM_TEXT_MAX_CHARS;
    for field in ["text", "command", "aggregatedOutput"] {
        copy_budgeted_string(source, &mut item, field, &mut remaining, truncated);
    }
    for field in ["content", "summary"] {
        let Some(values) = source.get(field).and_then(Value::as_array) else {
            continue;
        };
        let mut projected = Vec::new();
        for value in values {
            if remaining == 0 {
                *truncated = true;
                break;
            }
            if field == "summary" {
                if let Some(text) = value.as_str() {
                    projected.push(Value::String(take_text(text, &mut remaining, truncated)));
                }
            } else if let Some(part) = value.as_object() {
                let mut projected_part = serde_json::Map::new();
                copy_field(part, &mut projected_part, "type");
                copy_budgeted_string(
                    part,
                    &mut projected_part,
                    "text",
                    &mut remaining,
                    truncated,
                );
                projected.push(Value::Object(projected_part));
            }
        }
        item.insert(field.into(), Value::Array(projected));
    }
    Some(Value::Object(item))
}

fn copy_field(
    source: &serde_json::Map<String, Value>,
    target: &mut serde_json::Map<String, Value>,
    field: &str,
) {
    if let Some(value) = source.get(field) {
        target.insert(field.into(), value.clone());
    }
}

fn copy_limited_string(
    source: &serde_json::Map<String, Value>,
    target: &mut serde_json::Map<String, Value>,
    field: &str,
    limit: usize,
    truncated: &mut bool,
) {
    let Some(value) = source.get(field).and_then(Value::as_str) else {
        return;
    };
    let mut remaining = limit;
    target.insert(
        field.into(),
        Value::String(take_text(value, &mut remaining, truncated)),
    );
}

fn copy_budgeted_string(
    source: &serde_json::Map<String, Value>,
    target: &mut serde_json::Map<String, Value>,
    field: &str,
    remaining: &mut usize,
    truncated: &mut bool,
) {
    let Some(value) = source.get(field).and_then(Value::as_str) else {
        return;
    };
    target.insert(
        field.into(),
        Value::String(take_text(value, remaining, truncated)),
    );
}

fn take_text(value: &str, remaining: &mut usize, truncated: &mut bool) -> String {
    let mut chars = value.chars();
    let text: String = chars.by_ref().take(*remaining).collect();
    let used = text.chars().count();
    *remaining = remaining.saturating_sub(used);
    if chars.next().is_some() {
        *truncated = true;
    }
    text
}

fn bounded_page_end<T: serde::Serialize>(
    items: &[T],
    start: usize,
    limit: usize,
) -> Result<usize, ServiceError> {
    let requested_end = start.saturating_add(limit).min(items.len());
    let mut end = start;
    let mut bytes = 0_usize;
    while end < requested_end {
        let item_bytes = serde_json::to_vec(&items[end])
            .map_err(|error| ServiceError::Backend(error.to_string()))?
            .len();
        if item_bytes > PAGE_PAYLOAD_MAX_BYTES {
            return Err(ServiceError::Backend(
                "one page item exceeds the monitoring transport budget".into(),
            ));
        }
        if end > start && bytes.saturating_add(item_bytes) > PAGE_PAYLOAD_MAX_BYTES {
            break;
        }
        bytes = bytes.saturating_add(item_bytes);
        end += 1;
    }
    Ok(end)
}
#[derive(Default, Deserialize)]
struct ApprovalList {
    thread: Option<ThreadKey>,
    page: Option<PageRequest>,
}
#[derive(Deserialize)]
struct ThreadsList {
    page: PageRequest,
    project_id: Option<String>,
}
#[derive(Deserialize)]
struct ProjectsList {
    page: PageRequest,
}
#[derive(Deserialize)]
struct ThreadsRead {
    thread_id: String,
    project_id: Option<String>,
    #[serde(default)]
    include_turns: bool,
    before_turn_id: Option<String>,
    turn_limit: Option<u32>,
}
#[derive(Default, Deserialize)]
struct EventsReplayParams {
    process_generation: Option<ProcessGeneration>,
    after: Option<Cursor>,
}

#[derive(Default, Deserialize)]
struct StateSubscribe {
    #[serde(default, rename = "after")]
    _after: Option<Cursor>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nekodex_appserver_adapter::{ApprovalContext, RequestId};
    use nekodex_protocol::{Event, Operation, ProtocolVersion};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_PROJECT: AtomicU64 = AtomicU64::new(1);

    fn request(id: &str, operation: Operation, params: Value) -> RequestEnvelope {
        RequestEnvelope {
            version: ProtocolVersion::V1,
            request_id: id.into(),
            source_device_id: "peer".into(),
            target_device_id: "desktop".into(),
            operation,
            params,
        }
    }

    // Project validation intentionally checks the live filesystem. Keep test
    // projects backed by real, disposable directories rather than placeholders.
    fn test_project_path(label: &str) -> String {
        let path = std::env::temp_dir().join(format!(
            "nekodex-source-service-{label}-{}-{}",
            std::process::id(),
            NEXT_TEST_PROJECT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&path).expect("create test project directory");
        path.to_str().expect("test path is UTF-8").to_owned()
    }

    #[test]
    fn thread_counts_classifies_running_and_attention() {
        let counts = thread_counts(
            &[
                json!({ "status": { "type": "active" } }),
                json!({ "status": { "type": "systemError" } }),
                json!({ "status": { "type": "active", "activeFlags": ["waitingOnApproval"] } }),
                json!({ "status": { "type": "idle" } }),
            ],
            true,
        );
        assert_eq!(counts["running"], 2);
        assert_eq!(counts["attention"], 2);
        assert_eq!(counts["total"], 4);
        assert_eq!(counts["truncated"], true);
        assert_eq!(counts["available"], true);
    }

    #[test]
    fn thread_counts_do_not_claim_live_status_for_unloaded_history() {
        let counts = thread_counts(
            &[
                json!({ "status": { "type": "notLoaded" } }),
                json!({ "status": { "type": "notLoaded" } }),
            ],
            false,
        );
        assert_eq!(counts["running"], 0);
        assert_eq!(counts["attention"], 0);
        assert_eq!(counts["total"], 2);
        assert_eq!(counts["available"], false);
    }

    #[test]
    fn malformed_or_oversized_thread_pages_are_rejected() {
        assert!(matches!(
            parse_thread_page(&json!({ "data": {}, "nextCursor": null }), 10),
            Err(ServiceError::Backend(_))
        ));
        assert!(matches!(
            parse_thread_page(&json!({ "data": [], "nextCursor": 7 }), 10),
            Err(ServiceError::Backend(_))
        ));
        assert!(matches!(
            parse_thread_page(&json!({ "data": [1, 2], "nextCursor": null }), 1),
            Err(ServiceError::Backend(_))
        ));
        assert_eq!(
            parse_thread_page(&json!({ "data": [{ "id": "thread" }], "nextCursor": "next" }), 1)
                .unwrap()
                .1
                .as_deref(),
            Some("next")
        );
    }

    #[test]
    fn long_thread_details_fit_the_monitoring_transport_and_report_truncation() {
        let large = "长".repeat(20_000);
        let detail = bounded_thread_detail(json!({
            "thread": {
                "id": "thread-1",
                "preview": large,
                "status": { "type": "idle" },
                "turns": (0..120).map(|index| json!({
                    "id": format!("turn-{index}"),
                    "status": "completed",
                    "items": [{
                        "id": format!("item-{index}"),
                        "type": "agentMessage",
                        "text": "答".repeat(20_000),
                        "unusedRawField": "x".repeat(20_000)
                    }]
                })).collect::<Vec<_>>()
            }
        }))
        .unwrap();

        assert_eq!(detail["truncated"], true);
        assert!(serde_json::to_vec(&detail).unwrap().len() <= THREAD_DETAIL_MAX_BYTES);
        assert!(detail.pointer("/thread/turns/0/items/0/unusedRawField").is_none());
        assert_eq!(
            detail["thread"]["turns"].as_array().unwrap().last().unwrap()["id"],
            "turn-119"
        );
        assert!(detail["next_before_turn_id"].is_string());
    }

    #[test]
    fn thread_detail_pagination_rejects_duplicate_or_missing_cursor_ids() {
        assert!(matches!(
            bounded_thread_detail_page(
                json!({ "thread": { "id": "thread", "turns": [
                    { "id": "turn-1" }, { "id": "turn-1" }
                ] } }),
                None,
                20,
            ),
            Err(ServiceError::Backend(_))
        ));
        assert!(matches!(
            bounded_thread_detail_page(
                json!({ "thread": { "id": "thread", "turns": [
                    { "id": "turn-1" }, { "id": "turn-2" }
                ] } }),
                Some("turn-unknown"),
                20,
            ),
            Err(ServiceError::InvalidParams(_))
        ));
    }

    #[test]
    fn usage_snapshot_keeps_only_bounded_daily_buckets() {
        let projected = project_usage(&json!({
            "summary": { "large": "x".repeat(100_000) },
            "threadUsage": { "large": "x".repeat(100_000) },
            "dailyUsageBuckets": (0..400).map(|day| json!({
                "startDate": format!("2026-01-{day:03}"),
                "tokens": day,
                "unused": "x".repeat(1_000),
            })).collect::<Vec<_>>()
        }));

        let buckets = projected["dailyUsageBuckets"].as_array().unwrap();
        assert_eq!(buckets.len(), USAGE_DAILY_BUCKET_LIMIT);
        assert!(buckets[0].get("unused").is_none());
        assert!(serde_json::to_vec(&projected).unwrap().len() < PAGE_PAYLOAD_MAX_BYTES);
    }

    #[test]
    fn byte_bounded_pages_continue_at_the_actual_end() {
        let items = vec!["x".repeat(10_000); 10];
        let end = bounded_page_end(&items, 0, items.len()).unwrap();
        assert!(end > 0 && end < items.len());
        assert_eq!(bounded_page_end(&items, end, items.len()).unwrap(), end * 2);
    }

    #[tokio::test]
    async fn event_replay_is_split_into_transport_safe_batches() {
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![],
            Arc::new(MemoryBackend::default()),
        );
        for cursor in 1..=100 {
            service
                .record_event(EventEnvelope {
                    version: ProtocolVersion::V1,
                    source_device_id: "desktop".into(),
                    process_generation: ProcessGeneration(4),
                    cursor: Cursor(cursor),
                    event: Event::SnapshotRequired {
                        oldest_available: Cursor(1),
                    },
                })
                .unwrap();
        }

        let first = service
            .handle(
                "peer",
                request(
                    "replay-1",
                    Operation::EventsReplay,
                    json!({ "process_generation": 4, "after": null }),
                ),
            )
            .await
            .unwrap();
        let first: EventReplay = serde_json::from_value(first.result.unwrap()).unwrap();
        assert_eq!(first.events.len(), EVENT_REPLAY_MAX_EVENTS);
        assert_eq!(first.newest_available, Some(Cursor(100)));

        let second = service
            .handle(
                "peer",
                request(
                    "replay-2",
                    Operation::EventsReplay,
                    json!({
                        "process_generation": 4,
                        "after": first.events.last().unwrap().cursor,
                    }),
                ),
            )
            .await
            .unwrap();
        let second: EventReplay = serde_json::from_value(second.result.unwrap()).unwrap();
        assert_eq!(second.events.len(), 100 - EVENT_REPLAY_MAX_EVENTS);
        assert_eq!(second.events.last().unwrap().cursor, Cursor(100));
    }

    #[test]
    fn derives_monitoring_only_approval_from_waiting_thread_status() {
        let approvals = waiting_approval_threads(
            &[
                json!({ "id": "thread-1", "status": { "type": "active", "activeFlags": ["waitingOnApproval"] } }),
                json!({ "id": "thread-2", "status": { "type": "active", "activeFlags": [] } }),
            ],
            "desktop",
            ProcessGeneration(7),
        );
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].thread.thread_id, "thread-1");
        assert!(approvals[0].options.is_empty());
        assert_eq!(approvals[0].status, ApprovalStatus::Pending);
    }

    #[tokio::test]
    async fn control_operations_are_rejected_by_the_monitoring_service() {
        let backend = Arc::new(MemoryBackend::default());
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![],
            backend.clone(),
        );
        let operations = [
            Operation::ThreadsCreate,
            Operation::TurnsSend,
            Operation::TurnsSteer,
            Operation::TurnsInterrupt,
            Operation::ApprovalsResolve,
            Operation::CommandsGet,
        ];
        for (index, operation) in operations.into_iter().enumerate() {
            let response = service
                .handle("peer", request(&format!("control-{index}"), operation.clone(), json!({})))
                .await
                .unwrap();
            assert_eq!(
                response.error,
                Some(ProtocolError::UnsupportedOperation { operation })
            );
        }
        assert!(backend.calls.lock().await.is_empty());
    }

    #[tokio::test]
    async fn thread_details_are_limited_to_allowlisted_projects() {
        let backend = Arc::new(MemoryBackend::default());
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![Project {
                project_id: "project-1".into(),
                source_device_id: "desktop".into(),
                name: "project".into(),
                path: test_project_path("thread-read-allowlist"),
                available_models: Vec::new(),
            }],
            Arc::clone(&backend),
        );

        let response = service
            .handle(
                "peer",
                request(
                    "thread-read",
                    Operation::ThreadsRead,
                    json!({
                        "thread_id": "thread-outside-allowlist",
                        "project_id": "project-1",
                        "include_turns": true
                    }),
                ),
            )
            .await
            .unwrap();

        assert!(matches!(
            response.error,
            Some(ProtocolError::NotFound { .. })
        ));
        assert_eq!(backend.calls.lock().await.as_slice(), ["threads.list"]);
    }

    #[tokio::test]
    async fn empty_project_allowlist_never_queries_unscoped_threads() {
        let backend = Arc::new(MemoryBackend::default());
        backend
            .thread_items
            .lock()
            .await
            .push(json!({ "id": "must-not-leak" }));
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![],
            Arc::clone(&backend),
        );

        let response = service
            .handle(
                "peer",
                request(
                    "threads-empty-allowlist",
                    Operation::ThreadsList,
                    json!({ "project_id": null, "page": { "after": null, "limit": 50 } }),
                ),
            )
            .await
            .unwrap();

        assert!(response.error.is_none());
        assert!(response.result.unwrap()["items"].as_array().unwrap().is_empty());
        assert!(backend.calls.lock().await.is_empty());
    }

    #[tokio::test]
    async fn thread_page_limits_are_bounded_before_backend_dispatch() {
        let backend = Arc::new(MemoryBackend::default());
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![Project {
                project_id: "project-1".into(),
                source_device_id: "desktop".into(),
                name: "project".into(),
                path: test_project_path("thread-page-limit"),
                available_models: Vec::new(),
            }],
            Arc::clone(&backend),
        );

        let bounded = service
            .handle(
                "peer",
                request(
                    "threads-large-page",
                    Operation::ThreadsList,
                    json!({ "project_id": "project-1", "page": { "after": null, "limit": u32::MAX } }),
                ),
            )
            .await
            .unwrap();
        let bounded_result = bounded.result.unwrap();
        assert!(bounded_result.get("raw").is_none());
        assert!(bounded_result["items"].is_array());

        let zero = service
            .handle(
                "peer",
                request(
                    "threads-zero-page",
                    Operation::ThreadsList,
                    json!({ "project_id": "project-1", "page": { "after": null, "limit": 0 } }),
                ),
            )
            .await
            .unwrap();
        assert!(matches!(
            zero.error,
            Some(ProtocolError::InvalidRequest { .. })
        ));
        assert_eq!(backend.calls.lock().await.as_slice(), ["threads.list"]);
        assert_eq!(
            backend.thread_limits.lock().await.as_slice(),
            [Some(THREAD_LIST_PAGE_LIMIT_MAX)]
        );
    }

    #[tokio::test]
    async fn listed_threads_skip_a_second_allowlist_scan_for_details() {
        let backend = Arc::new(MemoryBackend::default());
        backend
            .thread_items
            .lock()
            .await
            .push(json!({ "id": "thread-allowed" }));
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![Project {
                project_id: "project-1".into(),
                source_device_id: "desktop".into(),
                name: "project".into(),
                path: test_project_path("thread-read-cache"),
                available_models: Vec::new(),
            }],
            Arc::clone(&backend),
        );

        let listed = service
            .handle(
                "peer",
                request(
                    "thread-list",
                    Operation::ThreadsList,
                    json!({ "project_id": "project-1", "page": { "after": null, "limit": 50 } }),
                ),
            )
            .await
            .unwrap();
        assert!(listed.error.is_none());

        let detail = service
            .handle(
                "peer",
                request(
                    "thread-read",
                    Operation::ThreadsRead,
                    json!({
                        "thread_id": "thread-allowed",
                        "project_id": "project-1",
                        "include_turns": true
                    }),
                ),
            )
            .await
            .unwrap();
        assert!(detail.error.is_none());
        assert_eq!(
            backend.calls.lock().await.as_slice(),
            ["threads.list", "threads.read"]
        );
    }

    #[tokio::test]
    async fn thread_detail_pages_use_turn_ids_without_duplicates_or_gaps() {
        let backend = Arc::new(MemoryBackend::default());
        backend
            .thread_items
            .lock()
            .await
            .push(json!({ "id": "thread-page" }));
        *backend.thread_detail.lock().await = Some(json!({
            "thread": {
                "id": "thread-page",
                "turns": (0..45).map(|index| json!({
                    "id": format!("turn-{index:02}"),
                    "status": "completed",
                    "items": [{ "id": format!("item-{index:02}"), "text": format!("text-{index}") }]
                })).collect::<Vec<_>>()
            }
        }));
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![Project {
                project_id: "project-1".into(),
                source_device_id: "desktop".into(),
                name: "project".into(),
                path: test_project_path("thread-detail-pages"),
                available_models: Vec::new(),
            }],
            Arc::clone(&backend),
        );

        let read_page = |request_id: &str, before_turn_id: Option<&str>| {
            let mut params = json!({
                "thread_id": "thread-page",
                "project_id": "project-1",
                "include_turns": true,
                "turn_limit": 20,
            });
            if let Some(before_turn_id) = before_turn_id {
                params["before_turn_id"] = Value::String(before_turn_id.into());
            }
            service.handle("peer", request(request_id, Operation::ThreadsRead, params))
        };
        let first = read_page("thread-page-1", None).await.unwrap();
        let first = first.result.unwrap();
        let first_turns = first["thread"]["turns"].as_array().unwrap();
        assert_eq!(first_turns.len(), 20);
        assert_eq!(first_turns.first().unwrap()["id"], "turn-25");
        assert_eq!(first_turns.last().unwrap()["id"], "turn-44");
        assert_eq!(first["next_before_turn_id"], "turn-25");

        let second = read_page(
            "thread-page-2",
            first["next_before_turn_id"].as_str(),
        )
        .await
        .unwrap()
        .result
        .unwrap();
        let second_turns = second["thread"]["turns"].as_array().unwrap();
        assert_eq!(second_turns.len(), 20);
        assert_eq!(second_turns.first().unwrap()["id"], "turn-05");
        assert_eq!(second_turns.last().unwrap()["id"], "turn-24");
        assert_eq!(second["next_before_turn_id"], "turn-05");

        let third = read_page(
            "thread-page-3",
            second["next_before_turn_id"].as_str(),
        )
        .await
        .unwrap()
        .result
        .unwrap();
        let third_turns = third["thread"]["turns"].as_array().unwrap();
        assert_eq!(third_turns.len(), 5);
        assert_eq!(third_turns.first().unwrap()["id"], "turn-00");
        assert_eq!(third_turns.last().unwrap()["id"], "turn-04");
        assert!(third["next_before_turn_id"].is_null());

        // The client opens the newest page first, then prepends each older
        // page. Verify the resulting chronological view rather than network
        // request order.
        let ids = third_turns
            .iter()
            .chain(second_turns)
            .chain(first_turns)
            .map(|turn| turn["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(ids, (0..45).map(|index| format!("turn-{index:02}")).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn thread_detail_rejects_unknown_and_cross_context_cursors() {
        let backend = Arc::new(MemoryBackend::default());
        backend
            .thread_items
            .lock()
            .await
            .push(json!({ "id": "thread-page" }));
        *backend.thread_detail.lock().await = Some(json!({
            "thread": {
                "id": "thread-page",
                "turns": (0..25).map(|index| json!({ "id": format!("turn-{index}") })).collect::<Vec<_>>()
            }
        }));
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![Project {
                project_id: "project-1".into(),
                source_device_id: "desktop".into(),
                name: "project".into(),
                path: test_project_path("thread-detail-cursor-errors"),
                available_models: Vec::new(),
            }],
            Arc::clone(&backend),
        );
        let read = |request_id: &str, params: Value| {
            service.handle("peer", request(request_id, Operation::ThreadsRead, params))
        };
        let first = read(
            "thread-cursor-1",
            json!({ "thread_id": "thread-page", "project_id": "project-1", "include_turns": true }),
        )
        .await
        .unwrap()
        .result
        .unwrap();
        let cursor = first["next_before_turn_id"].as_str().unwrap();
        let unknown = read(
            "thread-cursor-unknown",
            json!({ "thread_id": "thread-page", "project_id": "project-1", "include_turns": true, "before_turn_id": "not-issued" }),
        )
        .await
        .unwrap();
        assert!(matches!(unknown.error, Some(ProtocolError::InvalidRequest { .. })));
        let changed_limit = read(
            "thread-cursor-context",
            json!({ "thread_id": "thread-page", "project_id": "project-1", "include_turns": true, "turn_limit": 10, "before_turn_id": cursor }),
        )
        .await
        .unwrap();
        assert!(matches!(changed_limit.error, Some(ProtocolError::InvalidRequest { .. })));
    }

    #[tokio::test]
    async fn historical_fallback_does_not_infer_live_approvals() {
        let backend = Arc::new(MemoryBackend::default());
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![Project {
                project_id: "project-1".into(),
                source_device_id: "desktop".into(),
                name: "project".into(),
                path: test_project_path("approval-mode"),
                available_models: Vec::new(),
            }],
            Arc::clone(&backend),
        );
        service
            .handle(
                "peer",
                request(
                    "approvals",
                    Operation::ApprovalsList,
                    json!({ "thread": null, "page": { "after": null, "limit": 100 } }),
                ),
            )
            .await
            .unwrap();
        assert!(backend.calls.lock().await.is_empty());

        service.set_thread_status_approvals_enabled(true);
        service
            .handle(
                "peer",
                request(
                    "approvals-shared",
                    Operation::ApprovalsList,
                    json!({ "thread": null, "page": { "after": null, "limit": 100 } }),
                ),
            )
            .await
            .unwrap();
        assert_eq!(backend.calls.lock().await.as_slice(), ["threads.list"]);
    }

    #[tokio::test]
    async fn approvals_list_returns_stable_pages_without_dropping_items() {
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![],
            Arc::new(MemoryBackend::default()),
        );
        for index in 0..101 {
            service
                .ingest(Incoming::Approval(ApprovalRequest::FileChange {
                    id: RequestId::Number(index),
                    context: ApprovalContext {
                        thread_id: format!("thread-{index:03}"),
                        turn_id: "turn".into(),
                        item_id: "item".into(),
                        started_at_ms: index,
                        reason: None,
                    },
                    grant_root: None,
                }))
                .await
                .unwrap();
        }

        let first = service
            .handle(
                "peer",
                request(
                    "approvals-1",
                    Operation::ApprovalsList,
                    json!({ "thread": null, "page": { "after": null, "limit": 100 } }),
                ),
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 100);
        let next = first["next"].as_u64().expect("first page cursor");

        // Removing an item before the cursor must not shift the second page
        // and hide its remaining approval.
        service.expire_resolved_request("0").await.unwrap();

        let second = service
            .handle(
                "peer",
                request(
                    "approvals-2",
                    Operation::ApprovalsList,
                    json!({ "thread": null, "page": { "after": next, "limit": 100 } }),
                ),
            )
            .await
            .unwrap()
            .result
            .unwrap();
        assert_eq!(second["items"].as_array().unwrap().len(), 1);
        assert!(second["next"].is_null());
    }

    #[tokio::test]
    async fn concurrent_viewers_reuse_the_recent_state_snapshot() {
        let backend = Arc::new(MemoryBackend::default());
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![Project {
                project_id: "project-1".into(),
                source_device_id: "desktop".into(),
                name: "project".into(),
                path: test_project_path("state-snapshot-cache"),
                available_models: Vec::new(),
            }],
            Arc::clone(&backend),
        );
        for id in ["state-1", "state-2"] {
            let response = service
                .handle(
                    "peer",
                    request(id, Operation::StateSubscribe, json!({ "after": null })),
                )
                .await
                .unwrap();
            assert!(response.error.is_none());
        }
        assert_eq!(
            backend.calls.lock().await.as_slice(),
            [
                "account/rateLimits/read",
                "account/usage/read",
                "threads.list"
            ]
        );
    }

    #[tokio::test]
    async fn overlapping_monitor_views_share_one_thread_scan() {
        let backend = Arc::new(MemoryBackend::default());
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![Project {
                project_id: "project-1".into(),
                source_device_id: "desktop".into(),
                name: "project".into(),
                path: test_project_path("shared-thread-scan"),
                available_models: Vec::new(),
            }],
            Arc::clone(&backend),
        );
        service.set_thread_status_approvals_enabled(true);

        service
            .handle(
                "peer",
                request("state", Operation::StateSubscribe, json!({ "after": null })),
            )
            .await
            .unwrap();
        service
            .handle(
                "peer",
                request(
                    "approvals",
                    Operation::ApprovalsList,
                    json!({ "thread": null, "page": { "after": null, "limit": 100 } }),
                ),
            )
            .await
            .unwrap();

        assert_eq!(
            backend
                .calls
                .lock()
                .await
                .iter()
                .filter(|call| call.as_str() == "threads.list")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn state_snapshot_preserves_usage_when_quota_is_unavailable() {
        let backend = Arc::new(MemoryBackend::default());
        backend.fail_rate_limits.store(true, Ordering::Release);
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![],
            backend,
        );

        let response = service
            .handle(
                "peer",
                request("partial-state", Operation::StateSubscribe, json!({ "after": null })),
            )
            .await
            .unwrap();
        let snapshot = response.result.unwrap();
        assert!(snapshot["quota"].is_null());
        assert!(snapshot["raw_usage"].is_object());
        assert!(snapshot["errors"]["quota"].as_str().is_some());
        assert!(snapshot["errors"]["usage"].is_null());
    }

    #[tokio::test]
    async fn server_resolved_notification_expires_pending_approval_once() {
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(4),
            vec![],
            Arc::new(MemoryBackend::default()),
        );
        service
            .ingest(Incoming::Approval(ApprovalRequest::FileChange {
                id: RequestId::Number(42),
                context: ApprovalContext {
                    thread_id: "thread".into(),
                    turn_id: "turn".into(),
                    item_id: "item".into(),
                    started_at_ms: 1,
                    reason: None,
                },
                grant_root: None,
            }))
            .await
            .unwrap();

        let expired = service
            .expire_resolved_request("42")
            .await
            .expect("pending approval");
        assert_eq!(expired.status, ApprovalStatus::Expired);
        assert!(service.expire_resolved_request("42").await.is_none());
    }

    #[tokio::test]
    async fn projects_list_returns_sorted_pages() {
        let service = SourceService::new(
            "desktop",
            ProcessGeneration(1),
            vec![
                Project {
                    project_id: "z".into(),
                    source_device_id: "desktop".into(),
                    name: "Z".into(),
                    path: test_project_path("z"),
                    available_models: vec![],
                },
                Project {
                    project_id: "a".into(),
                    source_device_id: "desktop".into(),
                    name: "A".into(),
                    path: test_project_path("a"),
                    available_models: vec![],
                },
                Project {
                    project_id: "m".into(),
                    source_device_id: "desktop".into(),
                    name: "M".into(),
                    path: test_project_path("m"),
                    available_models: vec![],
                },
            ],
            Arc::new(MemoryBackend::default()),
        );
        let response = service
            .handle(
                "peer",
                request(
                    "page-1",
                    Operation::ProjectsList,
                    json!({ "page": { "limit": 2 } }),
                ),
            )
            .await
            .unwrap();
        let page: Page<Project> = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|p| p.project_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "m"]
        );
        assert_eq!(page.next, Some(Cursor(2)));
    }

    #[test]
    fn opaque_cursors_are_stable_and_expire_when_evicted() {
        let mut registry = CursorRegistry {
            next: 1,
            entries: BTreeMap::new(),
            capacity: 2,
        };
        let first = registry.register("upstream-a".into());
        assert_eq!(registry.register("upstream-a".into()), first);
        let second = registry.register("upstream-b".into());
        registry.register("upstream-c".into());
        assert!(matches!(
            registry.resolve(first),
            Err(ServiceError::CursorExpired(oldest)) if oldest == second
        ));
        assert_eq!(registry.resolve(second).unwrap(), "upstream-b");
    }

    #[test]
    fn thread_cursors_are_bound_to_their_project_scope() {
        let mut registry = CursorRegistry::default();
        let project_a = registry.register_scoped("project-a", "same-upstream".into());
        let project_b = registry.register_scoped("project-b", "same-upstream".into());

        assert_ne!(project_a, project_b);
        assert_eq!(
            registry.resolve_scoped(project_a, "project-a").unwrap(),
            "same-upstream"
        );
        assert!(matches!(
            registry.resolve_scoped(project_a, "project-b"),
            Err(ServiceError::InvalidParams(_))
        ));
    }

    #[test]
    fn normalized_projects_share_scope_for_directory_aliases() {
        let root = std::env::temp_dir().join(format!(
            "nekodex-source-scope-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let alias = root.join("nested").join("..");
        let project = |path: std::path::PathBuf| Project {
            project_id: "same".into(),
            source_device_id: "desktop".into(),
            name: "project".into(),
            path: path.to_string_lossy().into_owned(),
            available_models: vec![],
        };
        let (root_projects, root_invalid) = normalize_projects(vec![project(root.clone())]);
        let (alias_projects, alias_invalid) = normalize_projects(vec![project(alias)]);
        assert!(root_invalid.is_empty());
        assert!(alias_invalid.is_empty());
        assert_eq!(
            project_scope(&root_projects.into_values().collect::<Vec<_>>()),
            project_scope(&alias_projects.into_values().collect::<Vec<_>>())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalization_keeps_invalid_projects_for_validation() {
        let project = Project {
            project_id: "missing".into(),
            source_device_id: "desktop".into(),
            name: "missing".into(),
            path: "/definitely/not-a-project".into(),
            available_models: vec![],
        };
        let (projects, invalid) = normalize_projects(vec![project]);
        assert!(invalid.contains("missing"));
        assert_eq!(projects["missing"].path, "/definitely/not-a-project");
    }
}
