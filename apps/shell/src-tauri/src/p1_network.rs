use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use nekodex_device_core::business::{BusinessClient, BusinessConnection, BusinessLimits};
use nekodex_device_core::discovery::{MdnsAdvertiser, MdnsAnnouncement, ReconnectPolicy};
use nekodex_device_core::join_transport::{
    JOIN_PROTOCOL_VERSION, JoinCandidate, serve_invitation_with_approval,
};
use nekodex_device_core::membership::{ActiveInvitation, InvitationOffer, ShortCode};
use nekodex_device_core::mesh::{DiscoverySource, MeshError, connect_candidate};
use nekodex_device_core::node::{JoinedNode, NodeInvitationCommitter};
use nekodex_device_core::storage::Storage;
use nekodex_device_core::transport::{
    OnlineSession, TransportError, TransportIo, WebSocketRoute, accept_routed, establish,
};
use nekodex_device_core::types::AddressCandidate;
use nekodex_protocol::{
    BusinessCapabilities, Event, EventEnvelope, EventReplay, Operation, ProcessGeneration,
    ProtocolError, ProtocolVersion, RequestEnvelope, ResponseEnvelope,
};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, RwLock, broadcast, mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};

const EVENT_CACHE_LIMIT: usize = 256;
const LAN_PORT: u16 = 8_788;
static NEXT_CONNECTION_TOKEN: AtomicU64 = AtomicU64::new(1);

struct EventCache {
    entries: VecDeque<EventEnvelope>,
    revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingJoinRequest {
    pub request_id: String,
    pub device_id: String,
    pub device_name: String,
    pub codex_capable: bool,
    pub protocol_version: u16,
}

struct PendingJoinApproval {
    request: PendingJoinRequest,
    decision: oneshot::Sender<bool>,
}

type PendingJoinState = Arc<StdMutex<Option<PendingJoinApproval>>>;

struct PendingInvitation {
    invitation: ActiveInvitation,
    committer: NodeInvitationCommitter,
}

struct ReconnectWorker {
    wake: Arc<Notify>,
    terminal: Arc<AtomicBool>,
}

pub struct NetworkService {
    port: u16,
    addresses: Arc<StdMutex<Vec<String>>>,
    local_addresses: Arc<RwLock<HashSet<IpAddr>>>,
    node: Arc<JoinedNode>,
    invitation: Arc<Mutex<Option<PendingInvitation>>>,
    pending_join: PendingJoinState,
    connections: Arc<RwLock<BTreeMap<String, PeerConnection>>>,
    peer_statuses: Arc<RwLock<BTreeMap<String, PeerStatus>>>,
    events: Arc<Mutex<EventCache>>,
    recovery_warnings: Arc<RwLock<BTreeMap<String, String>>>,
    reconnect_tx: mpsc::Sender<AddressCandidate>,
    discovery_refresh_tx: mpsc::Sender<()>,
    handler: Arc<dyn BusinessHandler>,
    tasks: StdMutex<Vec<JoinHandle<()>>>,
    advertiser: Arc<StdMutex<Option<MdnsAdvertiser>>>,
}

#[derive(Clone)]
struct PeerConnection {
    token: u64,
    client: BusinessClient,
    preferred: bool,
    negotiated_capabilities: BusinessCapabilities,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PeerStatus {
    Offline,
    Connecting,
    Online,
    AuthenticationFailed,
    Incompatible,
}

#[async_trait]
pub trait BusinessHandler: Send + Sync {
    async fn handle(&self, peer_device_id: &str, request: RequestEnvelope) -> ResponseEnvelope;

    fn subscribe_events(&self) -> Option<broadcast::Receiver<EventEnvelope>> {
        None
    }
}

pub struct UnavailableBusinessHandler;

#[async_trait]
impl BusinessHandler for UnavailableBusinessHandler {
    async fn handle(&self, _: &str, request: RequestEnvelope) -> ResponseEnvelope {
        ResponseEnvelope {
            version: request.version,
            request_id: request.request_id,
            result: None,
            error: Some(ProtocolError::SourceUnavailable),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitationPresentation {
    pub offer: InvitationOffer,
    pub short_code: String,
    pub port: u16,
    pub addresses: Vec<String>,
}

impl NetworkService {
    pub async fn start(
        node: Arc<JoinedNode>,
        handler: Arc<dyn BusinessHandler>,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, LAN_PORT))
            .await
            .map_err(|error| error.to_string())?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let invitation = Arc::new(Mutex::new(None));
        let pending_join = Arc::new(StdMutex::new(None));
        let next_join_request_id = Arc::new(AtomicU64::new(1));
        let connections = Arc::new(RwLock::new(BTreeMap::new()));
        let peer_statuses = Arc::new(RwLock::new(BTreeMap::new()));
        let persisted_events = match node.storage().source_events(EVENT_CACHE_LIMIT) {
            Ok(events) => events,
            Err(error) => {
                eprintln!("unable to restore persisted source events: {error}");
                Vec::new()
            }
        };
        let events = Arc::new(Mutex::new(EventCache {
            entries: persisted_events.into_iter().collect(),
            revision: 1,
        }));
        let recovery_warnings = Arc::new(RwLock::new(BTreeMap::new()));
        let (reconnect_tx, reconnect_rx) = mpsc::channel(128);
        let (discovery_refresh_tx, discovery_refresh_rx) = mpsc::channel(4);
        let addresses = lan_addresses();
        let local_addresses = Arc::new(RwLock::new(
            addresses
                .iter()
                .filter_map(|address| address.parse().ok())
                .collect(),
        ));
        let mut tasks = vec![tokio::spawn(accept_loop(
            listener,
            Arc::clone(&node),
            Arc::clone(&invitation),
            Arc::clone(&pending_join),
            Arc::clone(&next_join_request_id),
            Arc::clone(&connections),
            Arc::clone(&peer_statuses),
            Arc::clone(&events),
            Arc::clone(&recovery_warnings),
            Arc::clone(&handler),
        ))];
        if let Some(receiver) = handler.subscribe_events() {
            tasks.push(tokio::spawn(local_event_loop(
                receiver,
                Arc::clone(&connections),
                Arc::clone(&events),
                node.storage(),
                node.profile().device_id.clone(),
            )));
        }
        let remembered = node
            .storage()
            .address_candidates(&node.group().group_id, 100)
            .unwrap_or_default();
        for device_id in remembered
            .iter()
            .filter_map(|candidate| candidate.device_id.clone())
        {
            peer_statuses
                .write()
                .await
                .insert(device_id, PeerStatus::Offline);
        }
        tasks.push(tokio::spawn(discovery_loop(
            DiscoverySource::start().ok(),
            remembered,
            Arc::clone(&node),
            Arc::clone(&connections),
            Arc::clone(&peer_statuses),
            Arc::clone(&events),
            Arc::clone(&recovery_warnings),
            Arc::clone(&handler),
            port,
            Arc::clone(&local_addresses),
            reconnect_rx,
            discovery_refresh_rx,
        )));
        let advertised_addresses: Vec<IpAddr> = addresses
            .iter()
            .filter_map(|value| value.parse().ok())
            .collect();
        let advertiser = (!advertised_addresses.is_empty())
            .then(|| {
                MdnsAdvertiser::start_many(
                    &short_hint(&node.profile().device_id),
                    "nekodex.local.",
                    &advertised_addresses,
                    &MdnsAnnouncement::new(port, Some(node.device_name())),
                )
                .ok()
            })
            .flatten();
        let addresses = Arc::new(StdMutex::new(addresses));
        let advertiser = Arc::new(StdMutex::new(advertiser));
        tasks.push(tokio::spawn(address_monitor_loop(
            Arc::clone(&addresses),
            Arc::clone(&local_addresses),
            Arc::clone(&advertiser),
            Arc::clone(&invitation),
            Arc::clone(&node),
            port,
            discovery_refresh_tx.clone(),
        )));
        Ok(Self {
            port,
            addresses,
            local_addresses,
            node,
            invitation,
            pending_join,
            connections,
            peer_statuses,
            events,
            recovery_warnings,
            reconnect_tx,
            discovery_refresh_tx,
            handler,
            tasks: StdMutex::new(tasks),
            advertiser,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn peers(&self) -> Vec<String> {
        self.connections.read().await.keys().cloned().collect()
    }

    pub async fn peer_statuses(&self) -> BTreeMap<String, PeerStatus> {
        self.peer_statuses.read().await.clone()
    }

    pub async fn peer_capabilities(&self) -> BTreeMap<String, BusinessCapabilities> {
        self.connections
            .read()
            .await
            .iter()
            .map(|(device_id, connection)| {
                (
                    device_id.clone(),
                    connection.negotiated_capabilities.clone(),
                )
            })
            .collect()
    }

    pub async fn recovery_warnings(&self) -> BTreeMap<String, String> {
        self.recovery_warnings.read().await.clone()
    }

    pub async fn pending_join_request(&self) -> Option<PendingJoinRequest> {
        pending_join_snapshot(&self.pending_join)
    }

    pub async fn decide_join(&self, request_id: &str, accept: bool) -> Result<(), String> {
        let result = decide_pending_join(&self.pending_join, request_id, accept);
        if result.is_ok() {
            self.replace_advertisement(None);
        }
        result
    }

    pub async fn request(
        &self,
        target_device_id: &str,
        request: RequestEnvelope,
    ) -> Result<ResponseEnvelope, String> {
        if !self.node.is_member(target_device_id).await {
            return Err("目标设备已不是当前设备组的有效成员".to_owned());
        }
        let (token, client) = {
            let connections = self.connections.read().await;
            let connection = connections
                .get(target_device_id)
                .ok_or_else(|| "目标设备当前未连接".to_owned())?;
            if !connection
                .negotiated_capabilities
                .supports(&request.operation)
            {
                return Err("当前连接未协商该监控操作".to_owned());
            }
            (connection.token, connection.client.clone())
        };
        let response = client
            .request(request, Duration::from_secs(30))
            .await
            .map_err(|error| error.to_string())?;
        if !self.node.is_member(target_device_id).await
            || !self
                .connections
                .read()
                .await
                .get(target_device_id)
                .is_some_and(|connection| connection.token == token)
        {
            return Err("来源连接已在请求期间被替换".to_owned());
        }
        Ok(response)
    }

    pub async fn cached_events(&self) -> Vec<EventEnvelope> {
        self.events.lock().await.entries.iter().cloned().collect()
    }

    pub async fn event_revision(&self) -> u64 {
        self.events.lock().await.revision
    }

    /// Re-enqueue remembered peers after the platform returns to the foreground.
    /// The discovery loop deduplicates candidates that already have an active
    /// reconnect worker. Transient failures keep retrying in that worker, while
    /// terminal authentication or compatibility failures remain stopped until
    /// the network service is recreated after external state changes.
    pub async fn revalidate(&self) -> Result<(), String> {
        self.refresh_addresses().await;
        self.discovery_refresh_tx
            .send(())
            .await
            .map_err(|_| "局域网发现服务已停止".to_owned())?;
        let remembered = self
            .node
            .storage()
            .address_candidates(&self.node.group().group_id, 100)
            .map_err(|error| error.to_string())?;
        for candidate in remembered {
            self.reconnect_tx
                .send(candidate)
                .await
                .map_err(|_| "局域网重连服务已停止".to_owned())?;
        }
        Ok(())
    }

    async fn refresh_addresses(&self) {
        refresh_network_addresses(
            &self.addresses,
            &self.local_addresses,
            &self.advertiser,
            &self.invitation,
            &self.node,
            self.port,
            true,
        )
        .await;
    }

    pub async fn disconnect_all(&self) {
        let clients: Vec<_> = self
            .connections
            .read()
            .await
            .values()
            .map(|connection| connection.client.clone())
            .collect();
        for client in clients {
            client.close();
        }
    }

    pub async fn disconnect_peer(&self, device_id: &str) {
        if let Some(connection) = self.connections.write().await.remove(device_id) {
            connection.client.close();
        }
        self.peer_statuses.write().await.remove(device_id);
        self.recovery_warnings.write().await.remove(device_id);
    }

    pub async fn broadcast_membership(&self) -> Result<(), String> {
        broadcast_membership(&self.node, &self.connections).await
    }

    pub async fn connect_manual(&self, address: std::net::SocketAddr) -> Result<String, String> {
        let candidate = AddressCandidate {
            device_id: None,
            address,
            source: nekodex_device_core::types::CandidateSource::Manual,
            observed_at_unix_seconds: unix_seconds()? as i64,
        };
        let session = connect_candidate(Arc::clone(&self.node), candidate.clone())
            .await
            .map_err(|error| error.to_string())?;
        let peer = session.peer_device_id.clone();
        let mut remembered = candidate;
        remembered.device_id = Some(peer.clone());
        self.node
            .storage()
            .put_address_candidate(&self.node.group().group_id, &peer, &remembered)
            .map_err(|error| error.to_string())?;
        let task = tokio::spawn(run_authenticated_session(
            Arc::clone(&self.node),
            session,
            true,
            Arc::clone(&self.connections),
            Arc::clone(&self.peer_statuses),
            Arc::clone(&self.events),
            Arc::clone(&self.recovery_warnings),
            Arc::clone(&self.handler),
        ));
        match self.tasks.lock() {
            Ok(mut tasks) => tasks.push(task),
            Err(_) => {
                task.abort();
                return Err("连接任务注册失败".to_owned());
            }
        }
        Ok(peer)
    }

    pub async fn create_invitation(
        &self,
        lifetime: Duration,
    ) -> Result<InvitationPresentation, String> {
        let now = unix_seconds()?;
        let expires_at = now
            .checked_add(lifetime.as_secs())
            .ok_or_else(|| "邀请有效期溢出".to_owned())?;
        let invitation_id = format!("invite:{}", uuid::Uuid::new_v4());
        let event_id = format!("event:{}", uuid::Uuid::new_v4());
        let (invitation, short_code): (ActiveInvitation, ShortCode) = self
            .node
            .create_invitation(
                invitation_id,
                now,
                expires_at,
                nekodex_domain::invitation::InvitationPolicy {
                    max_failures: 5,
                    window_limit: 3,
                    window: 60,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        let presentation = InvitationPresentation {
            offer: invitation.offer().clone(),
            short_code: short_code.as_str().to_owned(),
            port: self.port,
            addresses: self
                .addresses
                .lock()
                .map(|addresses| addresses.clone())
                .unwrap_or_default()
                .iter()
                .filter_map(|address| address.parse::<IpAddr>().ok())
                .map(|address| std::net::SocketAddr::new(address, self.port).to_string())
                .collect(),
        };
        clear_pending_join(&self.pending_join);
        *self.invitation.lock().await = Some(PendingInvitation {
            invitation,
            committer: self.node.invitation_committer(event_id),
        });
        self.replace_advertisement(Some(expires_at));
        Ok(presentation)
    }

    fn replace_advertisement(&self, invitation_expires_at: Option<u64>) {
        replace_advertisement(
            &self.addresses,
            &self.advertiser,
            &self.node,
            self.port,
            invitation_expires_at,
        );
    }
}

async fn broadcast_membership(
    node: &JoinedNode,
    connections: &RwLock<BTreeMap<String, PeerConnection>>,
) -> Result<(), String> {
    let bundle = serde_json::to_vec(&node.export_bundle().await)
        .map_err(|error| format!("成员状态编码失败：{error}"))?;
    let clients: Vec<_> = connections
        .read()
        .await
        .values()
        .map(|connection| connection.client.clone())
        .collect();
    let mut failed = 0usize;
    for client in clients {
        if client.queue_membership_update(bundle.clone()).is_err() {
            failed += 1;
            client.close();
        }
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} 个连接未能接收成员更新，已关闭并等待重连"))
    }
}

fn pending_join_snapshot(state: &PendingJoinState) -> Option<PendingJoinRequest> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|pending| pending.request.clone())
}

fn decide_pending_join(
    state: &PendingJoinState,
    request_id: &str,
    accept: bool,
) -> Result<(), String> {
    let pending = {
        let mut current = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(pending) = current.as_ref() else {
            return Err("没有等待确认的设备加入请求".to_owned());
        };
        if pending.request.request_id != request_id {
            return Err("设备加入请求已过期或不匹配".to_owned());
        }
        current.take().expect("pending join checked above")
    };
    pending
        .decision
        .send(accept)
        .map_err(|_| "设备加入请求已结束".to_owned())
}

fn clear_pending_join(state: &PendingJoinState) {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

fn clear_pending_join_if(state: &PendingJoinState, request_id: &str) {
    let mut current = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if current
        .as_ref()
        .is_some_and(|pending| pending.request.request_id == request_id)
    {
        current.take();
    }
}

async fn await_join_decision(
    candidate: JoinCandidate,
    state: PendingJoinState,
    next_request_id: Arc<AtomicU64>,
) -> bool {
    let request_id = format!("join:{}", next_request_id.fetch_add(1, Ordering::Relaxed));
    let profile = candidate.profile;
    let request = PendingJoinRequest {
        request_id: request_id.clone(),
        device_name: profile
            .as_ref()
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| candidate.joiner_id.clone()),
        codex_capable: profile
            .as_ref()
            .is_some_and(|profile| profile.codex_capable),
        protocol_version: profile
            .as_ref()
            .map_or(JOIN_PROTOCOL_VERSION, |profile| profile.protocol_version),
        device_id: candidate.joiner_id,
    };
    let (decision, receiver) = oneshot::channel();
    {
        let mut current = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current.is_some() {
            return false;
        }
        *current = Some(PendingJoinApproval { request, decision });
    }

    let cleanup_state = Arc::clone(&state);
    let cleanup_request_id = request_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(121)).await;
        clear_pending_join_if(&cleanup_state, &cleanup_request_id);
    });

    let accepted = receiver.await.unwrap_or(false);
    clear_pending_join_if(&state, &request_id);
    accepted
}

async fn local_event_loop(
    mut receiver: broadcast::Receiver<EventEnvelope>,
    connections: Arc<RwLock<BTreeMap<String, PeerConnection>>>,
    events: Arc<Mutex<EventCache>>,
    storage: Arc<Storage>,
    source_device_id: String,
) {
    loop {
        let event = match receiver.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if let Ok(restored) = storage.source_events_for(&source_device_id, EVENT_CACHE_LIMIT) {
                    let latest_generation = restored.last().map(|event| event.process_generation);
                    let restored: Vec<_> = restored
                        .into_iter()
                        .filter(|event| Some(event.process_generation) == latest_generation)
                        .collect();
                    let recovery = restored.first().zip(restored.last()).map(|(oldest, newest)| {
                        EventEnvelope {
                            version: newest.version,
                            source_device_id: newest.source_device_id.clone(),
                            process_generation: newest.process_generation,
                            cursor: newest.cursor,
                            event: Event::SnapshotRequired {
                                oldest_available: oldest.cursor,
                            },
                        }
                    });
                    {
                        let mut cache = events.lock().await;
                        cache.entries.retain(|event| event.source_device_id != source_device_id);
                        cache.entries.extend(restored);
                        while cache.entries.len() > EVENT_CACHE_LIMIT {
                            cache.entries.pop_front();
                        }
                        cache.revision = cache.revision.wrapping_add(1);
                    }
                    if let Some(recovery) = recovery {
                        let clients: Vec<_> = connections
                            .read()
                            .await
                            .values()
                            .map(|connection| connection.client.clone())
                            .collect();
                        for client in clients {
                            let _ = client.queue_event(recovery.clone());
                        }
                    }
                }
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        };
        match cache_event(&events, &storage, event.clone()).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                eprintln!("unable to persist local source event: {error}");
                continue;
            }
        }
        let clients: Vec<_> = connections
            .read()
            .await
            .values()
            .map(|connection| connection.client.clone())
            .collect();
        for client in clients {
            let _ = client.queue_event(event.clone());
        }
    }
}

async fn cache_event(
    events: &Mutex<EventCache>,
    storage: &Storage,
    event: EventEnvelope,
) -> Result<bool, String> {
    let observed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let previous_generation = storage
        .cached_entry::<ProcessGeneration>(
            &event.source_device_id,
            nekodex_device_core::storage::SOURCE_GENERATION_CACHE_KEY,
        )
        .map_err(|error| error.to_string())?
        .map(|entry| entry.value);
    if previous_generation != Some(event.process_generation) {
        storage
            .begin_source_generation(
                &event.source_device_id,
                event.process_generation,
                observed_at,
            )
            .map_err(|error| error.to_string())?;
    }
    storage
        .put_source_event(&event, observed_at)
        .map_err(|error| format!("unable to persist authenticated source event: {error}"))?;

    let mut cache = events.lock().await;
    if cache.entries.iter().any(|cached| {
        cached.source_device_id == event.source_device_id
            && cached.process_generation == event.process_generation
            && cached.cursor == event.cursor
    }) {
        return Ok(false);
    }
    if cache.entries.len() >= EVENT_CACHE_LIMIT {
        cache.entries.pop_front();
    }
    cache.entries.push_back(event);
    cache.revision = cache.revision.wrapping_add(1);
    Ok(true)
}

impl Drop for NetworkService {
    fn drop(&mut self) {
        clear_pending_join(&self.pending_join);
        let tasks = self
            .tasks
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for task in tasks {
            task.abort();
        }
        let advertiser = self
            .advertiser
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(advertiser) = advertiser {
            let _ = advertiser.stop();
        }
    }
}

fn replace_advertisement(
    addresses: &StdMutex<Vec<String>>,
    advertiser: &StdMutex<Option<MdnsAdvertiser>>,
    node: &JoinedNode,
    port: u16,
    invitation_expires_at: Option<u64>,
) {
    let addresses: Vec<IpAddr> = addresses
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter_map(|value| value.parse().ok())
        .collect();
    let mut advertiser = advertiser
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(previous) = advertiser.take() {
        let _ = previous.stop();
    }
    *advertiser = (!addresses.is_empty())
        .then(|| {
            let mut announcement = MdnsAnnouncement::new(port, Some(node.device_name()));
            if let Some(expires_at) = invitation_expires_at {
                announcement = announcement.with_active_invitation(expires_at);
            }
            MdnsAdvertiser::start_many(
                &short_hint(&node.profile().device_id),
                "nekodex.local.",
                &addresses,
                &announcement,
            )
            .ok()
        })
        .flatten();
}

async fn refresh_network_addresses(
    addresses: &StdMutex<Vec<String>>,
    local_addresses: &RwLock<HashSet<IpAddr>>,
    advertiser: &StdMutex<Option<MdnsAdvertiser>>,
    invitation: &Mutex<Option<PendingInvitation>>,
    node: &JoinedNode,
    port: u16,
    force: bool,
) -> bool {
    let next = lan_addresses();
    let changed = {
        let mut current = addresses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = *current != next;
        if changed {
            *current = next.clone();
        }
        changed
    };
    if !changed && !force {
        return false;
    }
    *local_addresses.write().await = next
        .iter()
        .filter_map(|address| address.parse().ok())
        .collect();
    let invitation_expires_at = invitation
        .lock()
        .await
        .as_ref()
        .map(|pending| pending.invitation.offer().expires_at);
    replace_advertisement(
        addresses,
        advertiser,
        node,
        port,
        invitation_expires_at,
    );
    true
}

async fn address_monitor_loop(
    addresses: Arc<StdMutex<Vec<String>>>,
    local_addresses: Arc<RwLock<HashSet<IpAddr>>>,
    advertiser: Arc<StdMutex<Option<MdnsAdvertiser>>>,
    invitation: Arc<Mutex<Option<PendingInvitation>>>,
    node: Arc<JoinedNode>,
    port: u16,
    discovery_refresh_tx: mpsc::Sender<()>,
) {
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(60),
        Duration::from_secs(60),
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if refresh_network_addresses(
            &addresses,
            &local_addresses,
            &advertiser,
            &invitation,
            &node,
            port,
            false,
        )
        .await
        {
            let _ = discovery_refresh_tx.try_send(());
        }
    }
}

async fn accept_loop(
    listener: TcpListener,
    node: Arc<JoinedNode>,
    invitation: Arc<Mutex<Option<PendingInvitation>>>,
    pending_join: PendingJoinState,
    next_join_request_id: Arc<AtomicU64>,
    connections: Arc<RwLock<BTreeMap<String, PeerConnection>>>,
    peer_statuses: Arc<RwLock<BTreeMap<String, PeerStatus>>>,
    events: Arc<Mutex<EventCache>>,
    recovery_warnings: Arc<RwLock<BTreeMap<String, String>>>,
    handler: Arc<dyn BusinessHandler>,
) {
    let mut sessions = JoinSet::new();
    loop {
        while sessions.try_join_next().is_some() {}
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let node = Arc::clone(&node);
        let invitation = Arc::clone(&invitation);
        let pending_join = Arc::clone(&pending_join);
        let next_join_request_id = Arc::clone(&next_join_request_id);
        let connections = Arc::clone(&connections);
        let peer_statuses = Arc::clone(&peer_statuses);
        let events = Arc::clone(&events);
        let recovery_warnings = Arc::clone(&recovery_warnings);
        let handler = Arc::clone(&handler);
        sessions.spawn(async move {
            let Ok((mut socket, route)) = accept_routed(stream).await else {
                return;
            };
            match route {
                WebSocketRoute::Device => {
                    let credentials = node.credentials();
                    let authorizer = node.authorizer();
                    let Ok(session) = establish(socket, false, &credentials, &authorizer).await
                    else {
                        return;
                    };
                    run_authenticated_session(
                        node,
                        session,
                        false,
                        connections,
                        peer_statuses,
                        events,
                        recovery_warnings,
                        handler,
                    )
                    .await;
                }
                WebSocketRoute::Join => {
                    let mut invitation = invitation.lock().await;
                    let Some(pending) = invitation.as_mut() else {
                        return;
                    };
                    let now = unix_seconds().unwrap_or(0);
                    if serve_invitation_with_approval(
                        &mut socket,
                        &mut pending.invitation,
                        now,
                        &mut pending.committer,
                        move |candidate| {
                            await_join_decision(candidate, pending_join, next_join_request_id)
                        },
                    )
                    .await
                    .is_ok()
                    {
                        *invitation = None;
                        if let Err(error) = broadcast_membership(&node, &connections).await {
                            eprintln!("unable to broadcast joined membership: {error}");
                        }
                    }
                }
                WebSocketRoute::Unknown => {}
            }
        });
    }
}

async fn discovery_loop(
    mut discovery: Option<DiscoverySource>,
    remembered: Vec<AddressCandidate>,
    node: Arc<JoinedNode>,
    connections: Arc<RwLock<BTreeMap<String, PeerConnection>>>,
    peer_statuses: Arc<RwLock<BTreeMap<String, PeerStatus>>>,
    events: Arc<Mutex<EventCache>>,
    recovery_warnings: Arc<RwLock<BTreeMap<String, String>>>,
    handler: Arc<dyn BusinessHandler>,
    local_port: u16,
    local_addresses: Arc<RwLock<HashSet<IpAddr>>>,
    mut reconnect_rx: mpsc::Receiver<AddressCandidate>,
    mut discovery_refresh_rx: mpsc::Receiver<()>,
) {
    // Entries also act as terminal-failure tombstones. Keeping the address in
    // this map prevents repeated mDNS announcements from spawning fresh workers
    // after an authentication, identity, or compatibility failure.
    let active = Arc::new(Mutex::new(HashMap::new()));
    let mut workers = JoinSet::new();
    let mut background_recovery = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(30 * 60),
        Duration::from_secs(30 * 60),
    );
    background_recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    for candidate in remembered {
        spawn_reconnect_worker(
            &mut workers,
            candidate,
            &node,
            &connections,
            &peer_statuses,
            &events,
            &recovery_warnings,
            &handler,
            &active,
            false,
        )
        .await;
    }
    loop {
        if let Some(source) = discovery.as_mut() {
            tokio::select! {
                candidate = reconnect_rx.recv() => {
                    let Some(candidate) = candidate else { break; };
                    spawn_reconnect_worker(
                        &mut workers,
                        candidate,
                        &node,
                        &connections,
                        &peer_statuses,
                        &events,
                        &recovery_warnings,
                        &handler,
                        &active,
                        true,
                    ).await;
                }
                refresh = discovery_refresh_rx.recv() => {
                    if refresh.is_none() { break; }
                    discovery = DiscoverySource::start().ok();
                }
                _ = background_recovery.tick() => {
                    for candidate in node
                        .storage()
                        .address_candidates(&node.group().group_id, 100)
                        .unwrap_or_default()
                    {
                        spawn_reconnect_worker(
                            &mut workers,
                            candidate,
                            &node,
                            &connections,
                            &peer_statuses,
                            &events,
                            &recovery_warnings,
                            &handler,
                            &active,
                            false,
                        ).await;
                    }
                }
                candidate = source.recv() => {
                    let Some(candidate) = candidate else {
                        discovery = None;
                        continue;
                    };
                    if candidate.address.port() == local_port
                        && local_addresses.read().await.contains(&candidate.address.ip())
                    {
                        continue;
                    }
                    spawn_reconnect_worker(
                        &mut workers,
                        candidate,
                        &node,
                        &connections,
                        &peer_statuses,
                        &events,
                        &recovery_warnings,
                        &handler,
                        &active,
                        false,
                    ).await;
                }
                _ = workers.join_next(), if !workers.is_empty() => {}
            }
        } else {
            tokio::select! {
                refresh = discovery_refresh_rx.recv() => {
                    if refresh.is_none() { break; }
                    discovery = DiscoverySource::start().ok();
                }
                candidate = reconnect_rx.recv() => {
                    let Some(candidate) = candidate else { break; };
                    spawn_reconnect_worker(
                        &mut workers,
                        candidate,
                        &node,
                        &connections,
                        &peer_statuses,
                        &events,
                        &recovery_warnings,
                        &handler,
                        &active,
                        true,
                    ).await;
                }
                _ = background_recovery.tick() => {
                    for candidate in node
                        .storage()
                        .address_candidates(&node.group().group_id, 100)
                        .unwrap_or_default()
                    {
                        spawn_reconnect_worker(
                            &mut workers,
                            candidate,
                            &node,
                            &connections,
                            &peer_statuses,
                            &events,
                            &recovery_warnings,
                            &handler,
                            &active,
                            false,
                        ).await;
                    }
                }
                _ = workers.join_next(), if !workers.is_empty() => {}
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn spawn_reconnect_worker(
    workers: &mut JoinSet<()>,
    candidate: AddressCandidate,
    node: &Arc<JoinedNode>,
    connections: &Arc<RwLock<BTreeMap<String, PeerConnection>>>,
    peer_statuses: &Arc<RwLock<BTreeMap<String, PeerStatus>>>,
    events: &Arc<Mutex<EventCache>>,
    recovery_warnings: &Arc<RwLock<BTreeMap<String, String>>>,
    handler: &Arc<dyn BusinessHandler>,
    active: &Arc<Mutex<HashMap<std::net::SocketAddr, ReconnectWorker>>>,
    allow_terminal_restart: bool,
) {
    let key = candidate.address;
    let (wake, terminal) = {
        let mut active = active.lock().await;
        if let Some(worker) = active.get(&key) {
            if !worker.terminal.load(Ordering::Acquire) || !allow_terminal_restart {
                worker.wake.notify_one();
                return;
            }
            active.remove(&key);
        }
        let wake = Arc::new(Notify::new());
        let terminal = Arc::new(AtomicBool::new(false));
        active.insert(
            key,
            ReconnectWorker {
                wake: Arc::clone(&wake),
                terminal: Arc::clone(&terminal),
            },
        );
        (wake, terminal)
    };
    let node = Arc::clone(node);
    let connections = Arc::clone(connections);
    let peer_statuses = Arc::clone(peer_statuses);
    let events = Arc::clone(events);
    let recovery_warnings = Arc::clone(recovery_warnings);
    let handler = Arc::clone(handler);
    workers.spawn(async move {
        let mut candidate = candidate;
        let mut retry = ReconnectPolicy::new().with_jitter(Duration::from_secs(1));
        loop {
            if let Some(device_id) = candidate.device_id.as_deref() {
                if connections.read().await.contains_key(device_id) {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                        _ = wake.notified() => {}
                    }
                    continue;
                }
                peer_statuses
                    .write()
                    .await
                    .insert(device_id.to_owned(), PeerStatus::Connecting);
            }
            match connect_candidate(Arc::clone(&node), candidate.clone()).await {
                Ok(session) => {
                    let peer = session.peer_device_id.clone();
                    candidate.device_id = Some(peer.clone());
                    let _ = node.storage().put_address_candidate(
                        &node.group().group_id,
                        &peer,
                        &candidate,
                    );
                    retry.record_success();
                    run_authenticated_session(
                        Arc::clone(&node),
                        session,
                        true,
                        Arc::clone(&connections),
                        Arc::clone(&peer_statuses),
                        Arc::clone(&events),
                        Arc::clone(&recovery_warnings),
                        Arc::clone(&handler),
                    )
                    .await;
                    let delay = retry.record_failure();
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = wake.notified() => {}
                    }
                }
                Err(error) => {
                    if let Some(device_id) = candidate.device_id.as_deref() {
                        peer_statuses
                            .write()
                            .await
                            .insert(device_id.to_owned(), status_for_error(&error));
                    }
                    if is_terminal_connect_error(&error) {
                        terminal.store(true, Ordering::Release);
                        break;
                    }
                    let delay = retry.record_failure();
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = wake.notified() => {}
                    }
                }
            }
        }
        // Keep a terminal tombstone so recurring discovery announcements do
        // not retry a peer that needs an external change. Explicit foreground
        // revalidation may replace that tombstone after network or peer state
        // has changed.
    });
}

async fn run_authenticated_session<S: TransportIo + 'static>(
    node: Arc<JoinedNode>,
    session: OnlineSession<S>,
    initiator: bool,
    connections: Arc<RwLock<BTreeMap<String, PeerConnection>>>,
    peer_statuses: Arc<RwLock<BTreeMap<String, PeerStatus>>>,
    events: Arc<Mutex<EventCache>>,
    recovery_warnings: Arc<RwLock<BTreeMap<String, String>>>,
    handler: Arc<dyn BusinessHandler>,
) {
    let peer = session.peer_device_id.clone();
    let negotiated_capabilities = session.negotiated_capabilities().clone();
    let local = &node.profile().device_id;
    if peer == *local {
        return;
    }
    let token = NEXT_CONNECTION_TOKEN.fetch_add(1, Ordering::Relaxed);
    let (client, mut server, mut incoming_events, mut membership_updates) = BusinessConnection::spawn(
        session,
        BusinessLimits::default(),
        Arc::new(node.authorizer()),
    );
    let recovery_client = client.clone();
    let preferred = initiator == (local < &peer);
    let mut current = connections.write().await;
    if let Some(existing) = current.get(&peer) {
        if preferred && !existing.preferred {
            existing.client.close();
        } else {
            return;
        }
    }
    current.insert(
        peer.clone(),
        PeerConnection {
            token,
            client,
            preferred,
            negotiated_capabilities,
        },
    );
    drop(current);
    peer_statuses
        .write()
        .await
        .insert(peer.clone(), PeerStatus::Online);
    let (recovery_tx, mut recovery_rx) = mpsc::channel(1);
    let recovery_node = Arc::clone(&node);
    let recovery_peer = peer.clone();
    let recovery_events = Arc::clone(&events);
    let recovery_status = Arc::clone(&recovery_warnings);
    let recovery_connections = Arc::clone(&connections);
    let recovery_task = tokio::spawn(async move {
        while recovery_rx.recv().await.is_some() {
            match recover_source_events(
                &recovery_client,
                &recovery_node,
                &recovery_peer,
                &recovery_connections,
                token,
                &recovery_events,
            )
            .await
            {
                Ok(()) => {
                    recovery_status.write().await.remove(&recovery_peer);
                }
                Err(error) => {
                    if !recovery_connections
                        .read()
                        .await
                        .get(&recovery_peer)
                        .is_some_and(|connection| connection.token == token)
                    {
                        break;
                    }
                    eprintln!("unable to recover source events from {recovery_peer}: {error}");
                    recovery_status
                        .write()
                        .await
                        .insert(recovery_peer.clone(), error);
                }
            }
        }
    });
    let peer_is_source = node
        .active_member_profiles()
        .await
        .into_iter()
        .any(|profile| profile.device_id == peer && profile.codex_capable);
    if peer_is_source
        && connections
        .read()
        .await
        .get(&peer)
        .is_some_and(|connection| {
            connection.token == token
                && connection
                    .negotiated_capabilities
                    .supports(&Operation::EventsReplay)
        })
    {
        let _ = recovery_tx.try_send(());
    }

    loop {
        tokio::select! {
            request = server.receive_request() => {
                let Ok(request) = request else { break };
                if !node.is_member(&peer).await {
                    break;
                }
                let response = handler.handle(&peer, request).await;
                if let Err(error) = server.send_response(response).await {
                    eprintln!("unable to send monitoring response to {peer}: {error}");
                    break;
                }
            }
            membership = membership_updates.receive() => {
                if membership.is_err() {
                    break;
                }
                let connected: Vec<_> = connections
                    .read()
                    .await
                    .iter()
                    .map(|(device_id, connection)| {
                        (device_id.clone(), connection.client.clone())
                    })
                    .collect();
                for (device_id, client) in connected {
                    if !node.is_member(&device_id).await {
                        client.close();
                    }
                }
                if !node.is_member(&node.profile().device_id).await
                    || !node.is_member(&peer).await
                {
                    break;
                }
            }
            event = incoming_events.receive_event() => {
                let Ok(event) = event else { break };
                let current = connections.read().await;
                if !current
                    .get(&peer)
                    .is_some_and(|connection| connection.token == token)
                {
                    break;
                }
                if matches!(&event.event, Event::SnapshotRequired { .. }) {
                    match recovery_tx.try_send(()) {
                        Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {}
                        Err(mpsc::error::TrySendError::Closed(())) => break,
                    }
                } else {
                    let storage = node.storage();
                    if let Err(error) = cache_event(&events, &storage, event).await {
                        recovery_warnings.write().await.insert(peer.clone(), error);
                        match recovery_tx.try_send(()) {
                            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => {}
                            Err(mpsc::error::TrySendError::Closed(())) => break,
                        }
                    }
                }
                drop(current);
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if !node.is_member(&peer).await {
                    break;
                }
            }
        }
    }
    drop(recovery_tx);
    recovery_task.abort();

    let mut current = connections.write().await;
    if current
        .get(&peer)
        .is_some_and(|connection| connection.token == token)
    {
        current.remove(&peer);
        peer_statuses
            .write()
            .await
            .insert(peer, PeerStatus::Offline);
    }
}

async fn recover_source_events(
    client: &BusinessClient,
    node: &JoinedNode,
    peer: &str,
    connections: &RwLock<BTreeMap<String, PeerConnection>>,
    token: u64,
    events: &Mutex<EventCache>,
) -> Result<(), String> {
    if !connections
        .read()
        .await
        .get(peer)
        .is_some_and(|connection| connection.token == token)
    {
        return Err("event replay belongs to a replaced connection".to_owned());
    }
    let storage = node.storage();
    let position = storage
        .latest_source_position(peer)
        .map_err(|error| error.to_string())?;
    let (mut process_generation, mut after) = position
        .map(|(generation, cursor)| (Some(generation), Some(cursor)))
        .unwrap_or((None, None));
    let mut history_complete = true;
    let mut replayed = 0_usize;
    let mut replay_target = None;
    loop {
        let response = client
            .request(
                RequestEnvelope {
                    version: ProtocolVersion::V1,
                    request_id: format!("replay:{}", uuid::Uuid::new_v4()),
                    source_device_id: node.profile().device_id.clone(),
                    target_device_id: peer.to_owned(),
                    operation: Operation::EventsReplay,
                    params: serde_json::json!({
                        "process_generation": process_generation,
                        "after": after,
                    }),
                },
                Duration::from_secs(30),
            )
            .await
            .map_err(|error| error.to_string())?;
        if let Some(error) = response.error {
            return Err(format!("source rejected event replay: {error:?}"));
        }
        let replay: EventReplay = serde_json::from_value(
            response
                .result
                .ok_or_else(|| "source returned an empty event replay".to_owned())?,
        )
        .map_err(|error| format!("invalid event replay: {error}"))?;
        if !connections
            .read()
            .await
            .get(peer)
            .is_some_and(|connection| connection.token == token)
        {
            return Err("event replay completed on a replaced connection".to_owned());
        }
        if replay.source_device_id != peer {
            return Err("event replay source does not match the authenticated peer".to_owned());
        }
        if replay.events.iter().any(|event| {
            event.source_device_id != replay.source_device_id
                || event.process_generation != replay.process_generation
        }) {
            return Err("event replay contains a mismatched source or process generation".to_owned());
        }
        if replay
            .events
            .windows(2)
            .any(|pair| pair[0].cursor.0 >= pair[1].cursor.0)
        {
            return Err("event replay cursors are not strictly increasing".to_owned());
        }
        if process_generation.is_some_and(|generation| generation == replay.process_generation)
            && replay
                .events
                .first()
                .is_some_and(|event| after.is_some_and(|after| event.cursor.0 <= after.0))
        {
            return Err("event replay did not advance past the requested cursor".to_owned());
        }
        history_complete &= replay.complete;
        if replay_target.is_none() {
            replay_target = replay.newest_available;
        }
        let accepted: Vec<_> = replay
            .events
            .into_iter()
            .take_while(|event| replay_target.is_none_or(|target| event.cursor.0 <= target.0))
            .collect();
        let batch_last = accepted.last().map(|event| event.cursor);
        replayed = replayed.saturating_add(accepted.len());
        if replayed > 1000 {
            return Err("event replay exceeds the bounded history limit".to_owned());
        }
        for event in accepted {
            let current = connections.read().await;
            if !current
                .get(peer)
                .is_some_and(|connection| connection.token == token)
            {
                return Err("event replay was replaced while persisting events".to_owned());
            }
            cache_event(events, &storage, event).await?;
            drop(current);
        }
        if replay_target.is_none()
            || batch_last.is_some_and(|cursor| Some(cursor) == replay_target)
        {
            break;
        }
        let Some(cursor) = batch_last else {
            return Err("event replay did not advance toward the newest cursor".to_owned());
        };
        process_generation = Some(replay.process_generation);
        after = Some(cursor);
    }
    if !history_complete {
        return Err(format!(
            "source event replay from {peer} is incomplete; older activity is unavailable"
        ));
    }
    Ok(())
}

fn status_for_error(error: &MeshError) -> PeerStatus {
    match error {
        MeshError::IdentityMismatch | MeshError::Transport(TransportError::Unauthorized) => {
            PeerStatus::AuthenticationFailed
        }
        MeshError::Transport(
            TransportError::IncompatibleVersion { .. }
            | TransportError::IncompatibleBusinessProtocol,
        ) => PeerStatus::Incompatible,
        _ => PeerStatus::Offline,
    }
}

fn is_terminal_connect_error(error: &MeshError) -> bool {
    matches!(
        error,
        MeshError::IdentityMismatch
            | MeshError::Transport(
                TransportError::Unauthorized
                    | TransportError::IncompatibleVersion { .. }
                    | TransportError::IncompatibleBusinessProtocol
                    | TransportError::InvalidAddress
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn reconnect_error_classification_preserves_transient_retries() {
        assert!(is_terminal_connect_error(&MeshError::IdentityMismatch));
        assert!(is_terminal_connect_error(&MeshError::Transport(
            TransportError::Unauthorized
        )));
        assert!(is_terminal_connect_error(&MeshError::Transport(
            TransportError::IncompatibleVersion {
                expected: 1,
                actual: 2,
            }
        )));
        assert!(is_terminal_connect_error(&MeshError::Transport(
            TransportError::InvalidAddress
        )));

        assert!(!is_terminal_connect_error(&MeshError::Transport(
            TransportError::Timeout
        )));
        assert!(!is_terminal_connect_error(&MeshError::Transport(
            TransportError::Closed
        )));
        assert!(!is_terminal_connect_error(&MeshError::Transport(
            TransportError::Io(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "temporary network loss",
            ))
        )));
    }

    #[test]
    fn pending_join_decision_is_id_scoped_and_one_shot() {
        let (decision, mut receiver) = oneshot::channel();
        let state = Arc::new(StdMutex::new(Some(PendingJoinApproval {
            request: PendingJoinRequest {
                request_id: "join:1".to_owned(),
                device_id: "device-a".to_owned(),
                device_name: "Phone".to_owned(),
                codex_capable: false,
                protocol_version: 1,
            },
            decision,
        })));

        assert!(decide_pending_join(&state, "join:stale", true).is_err());
        assert_eq!(
            pending_join_snapshot(&state).map(|request| request.request_id),
            Some("join:1".to_owned())
        );

        decide_pending_join(&state, "join:1", true).expect("matching decision should be delivered");
        assert_eq!(receiver.try_recv(), Ok(true));
        assert!(pending_join_snapshot(&state).is_none());
        assert!(decide_pending_join(&state, "join:1", true).is_err());
    }
}

fn unix_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "系统时间早于 Unix epoch".to_owned())
}

fn short_hint(device_id: &str) -> String {
    device_id
        .chars()
        .rev()
        .take(12)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn lan_addresses() -> Vec<String> {
    let mut addresses = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|interface| interface.ip())
        .filter(|address| !address.is_loopback())
        .collect::<Vec<IpAddr>>();
    addresses.sort_by_key(|address| !address.is_ipv4());
    addresses.dedup();
    addresses
        .into_iter()
        .map(|address| address.to_string())
        .collect()
}
