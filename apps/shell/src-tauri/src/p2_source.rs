//! Desktop-only Codex source service. Android builds never link App Server.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex as StdMutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use async_trait::async_trait;
use nekodex_appserver_adapter::{ClientInfo, InitializeCapabilities, InitializeParams};
use nekodex_appserver_runtime::{AppServerRuntime, RuntimeConfig, RuntimeEvent};
use nekodex_device_core::node::JoinedNode;
use nekodex_device_core::storage::Storage;
use nekodex_protocol::{
    Activity, ActivityKind, ActivitySnapshot, Cursor, Event, EventEnvelope, ProcessGeneration,
    Project, ProtocolError, ProtocolVersion, RequestEnvelope, ResponseEnvelope, ThreadKey,
};
use nekodex_source_service::{RuntimeBackend, ServiceError, SourceService};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::p1_network::BusinessHandler;

type InnerSourceService = SourceService<RuntimeBackend>;
// App Server may emit a large initialized-state or thread response before the
// source layer can apply its much smaller LAN and UI budgets. Keep this desktop
// ingestion bound finite without widening any cross-device frame.
const SOURCE_APP_SERVER_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

pub struct DesktopSourceService {
    inner: Arc<InnerSourceService>,
    source_device_id: String,
    storage: Arc<Storage>,
    generation: ProcessGeneration,
    next_cursor: AtomicU64,
    events: broadcast::Sender<EventEnvelope>,
    ready: AtomicBool,
    termination_error: StdMutex<Option<String>>,
    shutdown: StdMutex<Option<oneshot::Sender<()>>>,
    event_task: StdMutex<Option<JoinHandle<()>>>,
    connection_mode: SourceConnectionMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceConnectionMode {
    SharedDaemon,
    IsolatedFallback,
}

impl SourceConnectionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SharedDaemon => "sharedDaemon",
            Self::IsolatedFallback => "isolatedFallback",
        }
    }
}

impl DesktopSourceService {
    pub(crate) async fn shutdown(&self) {
        let sender = self
            .shutdown
            .lock()
            .expect("source shutdown mutex poisoned")
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(());
        }
        let task = self
            .event_task
            .lock()
            .expect("source event task mutex poisoned")
            .take();
        if let Some(mut task) = task {
            if tokio::time::timeout(std::time::Duration::from_secs(6), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
    }

    pub(crate) async fn handle_request(
        &self,
        peer_device_id: &str,
        request: RequestEnvelope,
    ) -> Result<ResponseEnvelope, ServiceError> {
        self.health()
            .map_err(|_| ServiceError::Protocol(ProtocolError::SourceUnavailable))?;
        let state_snapshot = matches!(
            &request.operation,
            nekodex_protocol::Operation::StateSubscribe
        );
        let mut response = self.inner.handle(peer_device_id, request).await?;
        if state_snapshot && response.error.is_none() {
            if let Some(result) = response.result.as_mut().and_then(Value::as_object_mut) {
                result.insert(
                    "source_mode".to_owned(),
                    Value::String(self.connection_mode.as_str().to_owned()),
                );
            }
        }
        Ok(response)
    }

    fn publish(&self, event: Event) {
        let envelope = EventEnvelope {
            version: ProtocolVersion::V1,
            source_device_id: self.source_device_id.clone(),
            process_generation: self.generation,
            cursor: Cursor(self.next_cursor.fetch_add(1, Ordering::Relaxed)),
            event,
        };
        // Keep the raw envelope before broadcast so slow consumers can replay
        // events that fell out of the bounded broadcast channel.
        let _ = self.inner.record_event(envelope.clone());
        if let Err(error) = self.storage.put_source_event(&envelope, unix_seconds()) {
            eprintln!("unable to persist local source event: {error}");
        }
        let _ = self.events.send(envelope);
    }

    pub(crate) fn health(&self) -> Result<(), String> {
        if self.ready.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(self
                .termination_error
                .lock()
                .expect("source health mutex poisoned")
                .clone()
                .unwrap_or_else(|| "Codex App Server 已停止".to_owned()))
        }
    }

    pub(crate) fn connection_mode(&self) -> SourceConnectionMode {
        self.connection_mode
    }
}

#[async_trait]
impl BusinessHandler for DesktopSourceService {
    async fn handle(&self, peer_device_id: &str, request: RequestEnvelope) -> ResponseEnvelope {
        let version = request.version;
        let request_id = request.request_id.clone();
        match self.handle_request(peer_device_id, request).await {
            Ok(response) => response,
            Err(error) => {
                eprintln!("monitoring source request failed: {error}");
                ResponseEnvelope {
                version,
                request_id,
                result: None,
                error: Some(ProtocolError::SourceUnavailable),
                }
            }
        }
    }

    fn subscribe_events(&self) -> Option<broadcast::Receiver<EventEnvelope>> {
        Some(self.events.subscribe())
    }
}

pub async fn start(node: &Arc<JoinedNode>) -> Result<Arc<DesktopSourceService>, String> {
    start_with_shared_attempts(node, 1).await
}

pub async fn start_after_daemon_start(
    node: &Arc<JoinedNode>,
) -> Result<Arc<DesktopSourceService>, String> {
    start_with_shared_attempts(node, 4).await
}

async fn start_with_shared_attempts(
    node: &Arc<JoinedNode>,
    shared_attempts: usize,
) -> Result<Arc<DesktopSourceService>, String> {
    let shared_attempts = shared_attempts.max(1);
    let mut shared_runtime = None;
    for attempt in 0..shared_attempts {
        let mut shared = RuntimeConfig::new("codex", ["app-server", "proxy"]);
        shared.request_timeout = Duration::from_secs(3);
        shared.write_timeout = Duration::from_secs(2);
        shared.max_frame_bytes = SOURCE_APP_SERVER_MAX_FRAME_BYTES;
        if let Ok(runtime) = start_initialized_runtime(shared).await {
            shared_runtime = Some(runtime);
            break;
        }
        if attempt + 1 < shared_attempts {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let mut fallback = RuntimeConfig::default();
    fallback.max_frame_bytes = SOURCE_APP_SERVER_MAX_FRAME_BYTES;
    let (mut runtime, connection_mode) = match shared_runtime {
        Some(runtime) => (runtime, SourceConnectionMode::SharedDaemon),
        None => (
            start_initialized_runtime(fallback).await?,
            SourceConnectionMode::IsolatedFallback,
        ),
    };
    let client = runtime.client();

    let generation = new_process_generation();
    let storage = node.storage();
    let projects = storage
        .projects(&node.profile().device_id)
        .map_err(|error| error.to_string())?;
    let source_device_id = node.profile().device_id.clone();
    let inner = Arc::new(SourceService::new(
        source_device_id.clone(),
        generation,
        projects,
        Arc::new(RuntimeBackend::new(client, generation)),
    ));
    inner.set_thread_status_approvals_enabled(connection_mode == SourceConnectionMode::SharedDaemon);
    let (events, _) = broadcast::channel(64);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let service = Arc::new(DesktopSourceService {
        inner,
        source_device_id,
        storage,
        generation,
        next_cursor: AtomicU64::new(1),
        events,
        ready: AtomicBool::new(true),
        termination_error: StdMutex::new(None),
        shutdown: StdMutex::new(Some(shutdown_tx)),
        event_task: StdMutex::new(None),
        connection_mode,
    });
    service
        .storage
        .begin_source_generation(&service.source_device_id, generation, unix_seconds())
        .map_err(|error| error.to_string())?;
    let event_service = Arc::downgrade(&service);
    let event_task = tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                _ = &mut shutdown_rx => {
                    if let Some(service) = event_service.upgrade() {
                        service.ready.store(false, Ordering::Release);
                    }
                    let _ = runtime.shutdown().await;
                    break;
                }
                event = runtime.recv_event() => {
                    let Some(event) = event else {
                        if let Some(service) = event_service.upgrade() {
                            service.ready.store(false, Ordering::Release);
                            *service
                                .termination_error
                                .lock()
                                .expect("source health mutex poisoned") =
                                Some("Codex App Server 事件通道意外关闭".to_owned());
                        }
                        break;
                    };
                    event
                }
            };
            let Some(event_service) = event_service.upgrade() else {
                let _ = runtime.shutdown().await;
                break;
            };
            match event {
                RuntimeEvent::Incoming(incoming) => match incoming {
                    nekodex_appserver_adapter::Incoming::Notification { method, params } => {
                        if method == "serverRequest/resolved" {
                            if let Some(request_id) = resolved_request_id(&params) {
                                if let Some(approval) = event_service
                                    .inner
                                    .expire_resolved_request(&request_id)
                                    .await
                                {
                                    event_service.publish(Event::ApprovalChanged(approval));
                                }
                            }
                        }
                        if let Some(activity) =
                            notification_activity(&event_service.source_device_id, &method, &params)
                        {
                            event_service.publish(Event::ActivitySnapshot(ActivitySnapshot {
                                source_device_id: event_service.source_device_id.clone(),
                                activities: vec![activity],
                                observed_at_unix_seconds: unix_seconds(),
                                next: None,
                            }));
                        }
                    }
                    incoming => {
                        if let Ok(Some(approval)) = event_service.inner.ingest(incoming).await {
                            event_service.publish(Event::ApprovalChanged(approval));
                        }
                    }
                },
                RuntimeEvent::Terminated(error) => {
                    event_service.ready.store(false, Ordering::Release);
                    *event_service
                        .termination_error
                        .lock()
                        .expect("source health mutex poisoned") = Some(error.to_string());
                    break;
                }
            }
        }
    });
    *service
        .event_task
        .lock()
        .expect("source event task mutex poisoned") = Some(event_task);
    Ok(service)
}

async fn start_initialized_runtime(config: RuntimeConfig) -> Result<AppServerRuntime, String> {
    let runtime = AppServerRuntime::spawn(config)
        .await
        .map_err(|error| error.to_string())?;
    let client = runtime.client();
    client
        .request(|adapter| {
            adapter.initialize(InitializeParams {
                client_info: ClientInfo {
                    name: "nekodex_source_service".into(),
                    title: Some("Nekodex Source Service".into()),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
                capabilities: Some(InitializeCapabilities {
                    experimental_api: true,
                    request_attestation: false,
                    opt_out_notification_methods: Vec::new(),
                }),
            })
        })
        .await
        .map_err(|error| error.to_string())?;
    client
        .initialized()
        .await
        .map_err(|error| error.to_string())?;
    Ok(runtime)
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().try_into().unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn new_process_generation() -> ProcessGeneration {
    // The value crosses the JSON/WebView boundary and therefore stays within
    // JavaScript's exact integer range. Randomness avoids reusing a generation
    // when the system clock moves backwards between source restarts.
    const MAX_JS_INTEGER: u64 = (1_u64 << 53) - 1;
    let value = (uuid::Uuid::new_v4().as_u128() as u64) & MAX_JS_INTEGER;
    ProcessGeneration(value.max(1))
}

fn resolved_request_id(params: &Value) -> Option<String> {
    match params.get("requestId")? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn notification_activity(source_device_id: &str, method: &str, params: &Value) -> Option<Activity> {
    let item_type = string_at(params, &["item.type"]);
    let kind = match (method, item_type) {
        ("item/completed", Some("agentMessage")) => ActivityKind::Message,
        ("thread/started" | "turn/started" | "item/started", _) => ActivityKind::Started,
        ("thread/status/changed", _) => ActivityKind::Progress,
        ("item/completed" | "turn/completed", _) => ActivityKind::Finished,
        ("error" | "warning", _) => ActivityKind::Error,
        (_, _) if method.ends_with("/error") || method.ends_with("/warning") => {
            ActivityKind::Error
        }
        (_, _) if method.contains("/delta") || method.ends_with("/updated") => return None,
        _ => return None,
    };
    let thread_id = string_at(params, &["threadId", "thread.id", "thread.thread.id"])?;
    let turn_id = string_at(params, &["turnId", "turn.id", "turn.turn.id"]);
    let item_id = string_at(params, &["itemId", "item.id", "item.item.id"]);
    let summary = string_at(
        params,
        &[
            "message",
            "error.message",
            "warning.message",
            "status.type",
            "status",
            "item.text",
            "item.type",
        ],
    )
    .map(|value| limit_string(value, 256));
    let suffix = item_id.or(turn_id).unwrap_or(thread_id);
    Some(Activity {
        source_device_id: source_device_id.to_owned(),
        activity_id: format!("{method}:{suffix}:{}", unix_nanos()),
        thread: ThreadKey::new(source_device_id, thread_id),
        turn_id: turn_id.map(str::to_owned),
        kind,
        message: summary.or_else(|| Some(limit_string(method, 128))),
        occurred_at_unix_seconds: unix_seconds(),
    })
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn string_at<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths.iter().find_map(|path| {
        let pointer = format!("/{}", path.replace('.', "/"));
        value.pointer(&pointer).and_then(Value::as_str)
    })
}

fn limit_string(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projects_thread_and_turn_notifications() {
        let activity = notification_activity(
            "desktop",
            "turn/completed",
            &json!({"threadId":"thread-1", "turn":{"id":"turn-1","status":"completed"}}),
        )
        .expect("activity");
        assert_eq!(activity.thread.thread_id, "thread-1");
        assert_eq!(activity.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(activity.kind, ActivityKind::Finished);
    }

    #[test]
    fn filters_unknown_and_delta_notifications() {
        let params = json!({"threadId":"thread-1"});
        assert!(notification_activity("desktop", "turn/delta", &params).is_none());
        assert!(notification_activity("desktop", "thread/unknown", &params).is_none());
        assert!(notification_activity("desktop", "warning", &json!({})).is_none());
    }

    #[test]
    fn completed_agent_message_uses_authoritative_final_text() {
        let activity = notification_activity(
            "desktop",
            "item/completed",
            &json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "id": "item-1",
                    "type": "agentMessage",
                    "text": "最终回答"
                }
            }),
        )
        .expect("activity");
        assert_eq!(activity.kind, ActivityKind::Message);
        assert_eq!(activity.message.as_deref(), Some("最终回答"));
    }

    #[test]
    fn resolved_server_request_accepts_string_and_numeric_ids() {
        assert_eq!(
            resolved_request_id(&json!({ "requestId": "approval-1" })).as_deref(),
            Some("approval-1")
        );
        assert_eq!(
            resolved_request_id(&json!({ "requestId": 42 })).as_deref(),
            Some("42")
        );
        assert!(resolved_request_id(&json!({ "requestId": null })).is_none());
    }
}

pub async fn add_project(
    service: Option<&DesktopSourceService>,
    storage: &Storage,
    project: Project,
) -> Result<Project, String> {
    storage
        .clear_cache_source(&project.source_device_id)
        .map_err(|error| error.to_string())?;
    storage
        .put_project(&project)
        .map_err(|error| error.to_string())?;
    if let Some(service) = service {
        let projects = storage
            .projects(&project.source_device_id)
            .map_err(|error| error.to_string())?;
        service.inner.replace_projects(projects).await;
    }
    Ok(project)
}

pub async fn delete_project(
    service: Option<&DesktopSourceService>,
    storage: &Storage,
    source_device_id: &str,
    project_id: &str,
) -> Result<bool, String> {
    storage
        .clear_cache_source(source_device_id)
        .map_err(|error| error.to_string())?;
    let deleted = storage
        .delete_project(source_device_id, project_id)
        .map_err(|error| error.to_string())?;
    if let Some(service) = service {
        let projects = storage
            .projects(source_device_id)
            .map_err(|error| error.to_string())?;
        service.inner.replace_projects(projects).await;
    }
    Ok(deleted)
}
