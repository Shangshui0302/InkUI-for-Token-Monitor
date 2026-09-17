use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use nekodex_device_core::discovery::discover_nearby;
use nekodex_device_core::discovery::{MAX_MDNS_CANDIDATES, MAX_MDNS_HINT_CHARS};
use nekodex_device_core::identity::DeviceIdentity;
use nekodex_device_core::join_transport::{connect_join, join_discovered_invitation_for_profile};
use nekodex_device_core::node::JoinedNode;
use nekodex_device_core::storage::{
    OFFLINE_CACHE_MAX_BYTES, SOURCE_GENERATION_CACHE_KEY, Storage,
};
use nekodex_device_core::types::{AddressCandidate, CandidateSource};
use nekodex_protocol::{
    BusinessCapabilities, EventEnvelope, Operation, ProcessGeneration, Project, ProtocolVersion,
    RequestEnvelope, ResponseEnvelope,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

use crate::p1_network::{
    BusinessHandler, InvitationPresentation, NetworkService, PeerStatus, PendingJoinRequest,
    UnavailableBusinessHandler,
};
use crate::{platform_lifecycle, secret_store};

const DISPLAY_NAME_MAX_CHARS: usize = 128;

#[derive(Default)]
pub struct P1State {
    mutations: Mutex<()>,
    cache_epoch: AtomicU64,
    lifecycle: Mutex<Lifecycle>,
    network: Mutex<Option<Arc<NetworkService>>>,
    network_error: Mutex<Option<String>>,
    platform_error: Mutex<Option<String>>,
    #[cfg(not(target_os = "android"))]
    source: Mutex<Option<Arc<crate::p2_source::DesktopSourceService>>>,
    source_error: Mutex<Option<String>>,
}

#[derive(Default)]
enum Lifecycle {
    #[default]
    Cold,
    Unjoined {
        identity: DeviceIdentity,
        storage: Arc<Storage>,
    },
    Joined(Arc<JoinedNode>),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStateSnapshot {
    phase: &'static str,
    device_id: Option<String>,
    device_name: Option<String>,
    group_id: Option<String>,
    group_name: Option<String>,
    codex_capable: bool,
    background_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyDeviceSnapshot {
    device_hint: Option<String>,
    address: String,
}

impl DeviceStateSnapshot {
    fn cold() -> Self {
        Self {
            phase: "cold",
            device_id: None,
            device_name: None,
            group_id: None,
            group_name: None,
            codex_capable: cfg!(not(target_os = "android")),
            background_error: None,
        }
    }

    fn unjoined(identity: &DeviceIdentity) -> Self {
        Self {
            phase: "unjoined",
            device_id: Some(identity.device_id()),
            device_name: None,
            group_id: None,
            group_name: None,
            codex_capable: cfg!(not(target_os = "android")),
            background_error: None,
        }
    }

    fn joined(node: &JoinedNode, background_error: Option<String>) -> Self {
        Self {
            phase: "joined",
            device_id: Some(node.profile().device_id.clone()),
            device_name: Some(node.device_name()),
            group_id: Some(node.group().group_id.clone()),
            group_name: Some(node.group().name.clone()),
            codex_capable: node.profile().codex_capable,
            background_error,
        }
    }
}

fn snapshot(lifecycle: &Lifecycle, background_error: Option<String>) -> DeviceStateSnapshot {
    let mut snapshot = match lifecycle {
        Lifecycle::Cold => DeviceStateSnapshot::cold(),
        Lifecycle::Unjoined { identity, .. } => DeviceStateSnapshot::unjoined(identity),
        Lifecycle::Joined(node) => DeviceStateSnapshot::joined(node, None),
    };
    snapshot.background_error = background_error;
    snapshot
}

async fn start_platform_service<R: Runtime>(app: &AppHandle<R>, state: &P1State) {
    *state.platform_error.lock().await = platform_lifecycle::start_core_service(app)
        .err()
        .map(|error| format!("后台连接服务启动失败：{error}"));
}

async fn background_error(state: &P1State) -> Option<String> {
    let network = state.network_error.lock().await.clone();
    let platform = state.platform_error.lock().await.clone();
    let errors = [network, platform]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    (!errors.is_empty()).then(|| errors.join("；"))
}

async fn stop_platform_service<R: Runtime>(app: &AppHandle<R>, state: &P1State) {
    *state.platform_error.lock().await = platform_lifecycle::stop_core_service(app)
        .err()
        .map(|error| format!("后台连接服务停止失败：{error}"));
}

#[cfg(desktop)]
pub(crate) async fn shutdown_application(app: &AppHandle, state: &P1State) {
    let _mutation = state.mutations.lock().await;
    if let Some(network) = state.network.lock().await.take() {
        network.disconnect_all().await;
    }
    #[cfg(not(target_os = "android"))]
    if let Some(source) = state.source.lock().await.take() {
        source.shutdown().await;
    }
    stop_platform_service(app, state).await;
}

async fn reconcile_platform_service<R: Runtime>(
    app: &AppHandle<R>,
    state: &P1State,
    services_started: bool,
) {
    if services_started {
        start_platform_service(app, state).await;
    } else {
        stop_platform_service(app, state).await;
    }
}

async fn start_services(node: Arc<JoinedNode>, state: &P1State) -> Result<(), String> {
    start_services_with_source_policy(node, state, false).await
}

async fn start_services_with_source_policy(
    node: Arc<JoinedNode>,
    state: &P1State,
    await_shared_daemon: bool,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    let _ = await_shared_daemon;
    if state.network.lock().await.is_some() {
        *state.network_error.lock().await = None;
        return Ok(());
    }

    #[cfg(not(target_os = "android"))]
    let existing_source = { state.source.lock().await.clone() };
    #[cfg(not(target_os = "android"))]
    let handler: Arc<dyn BusinessHandler> = if let Some(source) = existing_source {
        source
    } else {
        let started = if await_shared_daemon {
            crate::p2_source::start_after_daemon_start(&node).await
        } else {
            crate::p2_source::start(&node).await
        };
        match started {
            Ok(source) => {
                *state.source.lock().await = Some(Arc::clone(&source));
                *state.source_error.lock().await = None;
                source
            }
            Err(error) => {
                *state.source_error.lock().await = Some(error);
                Arc::new(UnavailableBusinessHandler)
            }
        }
    };

    #[cfg(target_os = "android")]
    let handler: Arc<dyn BusinessHandler> = Arc::new(UnavailableBusinessHandler);

    match NetworkService::start(node, handler).await {
        Ok(network) => {
            *state.network.lock().await = Some(Arc::new(network));
            *state.network_error.lock().await = None;
            Ok(())
        }
        Err(error) => {
            *state.network_error.lock().await = Some(format!("局域网服务启动失败：{error}"));
            Err(error)
        }
    }
}

#[tauri::command]
pub async fn p1_state(state: State<'_, P1State>) -> Result<DeviceStateSnapshot, String> {
    let lifecycle = state.lifecycle.lock().await;
    Ok(snapshot(&lifecycle, background_error(&state).await))
}

#[tauri::command]
pub async fn p1_initialize(
    app: AppHandle,
    state: State<'_, P1State>,
) -> Result<DeviceStateSnapshot, String> {
    let _mutation = state.mutations.lock().await;
    let initialized_node = {
        let lifecycle = state.lifecycle.lock().await;
        match &*lifecycle {
            Lifecycle::Cold => None,
            Lifecycle::Joined(node) => Some(Arc::clone(node)),
            Lifecycle::Unjoined { .. } => {
                drop(lifecycle);
                stop_platform_service(&app, &state).await;
                let lifecycle = state.lifecycle.lock().await;
                return Ok(snapshot(&lifecycle, background_error(&state).await));
            }
        }
    };
    if let Some(node) = initialized_node {
        let started = start_services(node, &state).await.is_ok();
        reconcile_platform_service(&app, &state, started).await;
        let lifecycle = state.lifecycle.lock().await;
        return Ok(snapshot(&lifecycle, background_error(&state).await));
    }
    let app_for_worker = app.clone();
    let initialized = tauri::async_runtime::spawn_blocking(move || {
        let data_dir = app_for_worker
            .path()
            .app_data_dir()
            .map_err(|_| "应用数据目录不可用".to_owned())?;
        std::fs::create_dir_all(&data_dir).map_err(|_| "无法创建应用数据目录".to_owned())?;
        let storage = Arc::new(
            Storage::open(data_dir.join("device-v1.sqlite3")).map_err(|error| error.to_string())?,
        );
        let mut secrets = secret_store::open(&app_for_worker).map_err(|error| error.to_string())?;
        let identity = DeviceIdentity::load_or_generate(secrets.as_mut())
            .map_err(|error| error.to_string())?;
        let groups = storage.groups().map_err(|error| error.to_string())?;
        match groups.as_slice() {
            [] => Ok(Lifecycle::Unjoined { identity, storage }),
            [group] => JoinedNode::restore(identity, storage, &group.group_id)
                .map(|node| Lifecycle::Joined(Arc::new(node)))
                .map_err(|error| error.to_string()),
            _ => Err("本机存在多个设备组，拒绝隐式选择".to_owned()),
        }
    })
    .await
    .map_err(|_| "设备核心初始化任务异常结束".to_owned())??;
    let joined_node = {
        let mut lifecycle = state.lifecycle.lock().await;
        *lifecycle = initialized;
        match &*lifecycle {
            Lifecycle::Joined(node) => Some(Arc::clone(node)),
            _ => None,
        }
    };
    if let Some(node) = joined_node {
        let started = start_services(node, &state).await.is_ok();
        reconcile_platform_service(&app, &state, started).await;
    } else {
        stop_platform_service(&app, &state).await;
    }
    let lifecycle = state.lifecycle.lock().await;
    Ok(snapshot(&lifecycle, background_error(&state).await))
}

#[tauri::command]
pub async fn p1_create_group(
    app: AppHandle,
    state: State<'_, P1State>,
    group_name: String,
    device_name: String,
) -> Result<DeviceStateSnapshot, String> {
    let _mutation = state.mutations.lock().await;
    let group_name = validate_display_name(&group_name, "设备组名称")?;
    let device_name = validate_display_name(&device_name, "设备名称")?;
    let mut lifecycle = state.lifecycle.lock().await;
    let previous = std::mem::take(&mut *lifecycle);
    let Lifecycle::Unjoined { identity, storage } = previous else {
        *lifecycle = previous;
        return Err("当前状态不能创建新设备组".to_owned());
    };
    let recovery_identity = identity.clone();
    let recovery_storage = Arc::clone(&storage);
    match JoinedNode::create_group(
        identity,
        storage,
        format!("group:{}", uuid::Uuid::new_v4()),
        group_name,
        format!("event:{}", uuid::Uuid::new_v4()),
        device_name,
        cfg!(not(target_os = "android")),
    ) {
        Ok(node) => {
            let node = Arc::new(node);
            *lifecycle = Lifecycle::Joined(Arc::clone(&node));
            drop(lifecycle);
            let started = start_services(node, &state).await.is_ok();
            reconcile_platform_service(&app, &state, started).await;
            let lifecycle = state.lifecycle.lock().await;
            Ok(snapshot(&lifecycle, background_error(&state).await))
        }
        Err(error) => {
            *lifecycle = Lifecycle::Unjoined {
                identity: recovery_identity,
                storage: recovery_storage,
            };
            Err(error.to_string())
        }
    }
}

#[tauri::command]
pub async fn p1_discover_nearby(
    app: AppHandle,
    state: State<'_, P1State>,
) -> Result<Vec<NearbyDeviceSnapshot>, String> {
    if !matches!(*state.lifecycle.lock().await, Lifecycle::Unjoined { .. }) {
        return Err("只有尚未加入设备组时才能搜索附近邀请".to_owned());
    }

    platform_lifecycle::acquire_discovery_multicast_lock(&app)?;
    let result = tauri::async_runtime::spawn_blocking(|| {
        let observed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        discover_nearby(Duration::from_millis(1800), observed_at)
            .map(|candidates| {
                candidates
                    .into_iter()
                    .take(MAX_MDNS_CANDIDATES)
                    .map(|candidate| NearbyDeviceSnapshot {
                        device_hint: candidate.device_hint.filter(|hint| hint.chars().count() <= MAX_MDNS_HINT_CHARS),
                        address: candidate.address.address.to_string(),
                    })
                    .collect()
            })
            .map_err(|error| format!("附近设备搜索失败：{error}"))
    })
    .await
    .map_err(|_| "附近设备搜索任务异常结束".to_owned());
    let release = platform_lifecycle::release_discovery_multicast_lock(&app);

    match (result, release) {
        (Ok(scan), Ok(())) => scan,
        (Ok(Ok(_)), Err(error)) => Err(format!("附近设备搜索结束，但多播锁释放失败：{error}")),
        (Err(error), _) => Err(error),
        (Ok(Err(error)), _) => Err(error),
    }
}

#[tauri::command]
pub async fn p1_create_invitation(
    state: State<'_, P1State>,
) -> Result<InvitationPresentation, String> {
    let _mutation = state.mutations.lock().await;
    if !matches!(*state.lifecycle.lock().await, Lifecycle::Joined(_)) {
        return Err("尚未加入设备组".to_owned());
    }
    let network = state.network.lock().await;
    let network = network
        .as_ref()
        .ok_or_else(|| "局域网服务未启动".to_owned())?;
    network
        .create_invitation(std::time::Duration::from_secs(10 * 60))
        .await
}

#[tauri::command]
pub async fn p1_pending_join(
    state: State<'_, P1State>,
) -> Result<Option<PendingJoinRequest>, String> {
    let network = state.network.lock().await.clone();
    let Some(network) = network else {
        return Ok(None);
    };
    Ok(network.pending_join_request().await)
}

#[tauri::command]
pub async fn p1_decide_join(
    state: State<'_, P1State>,
    request_id: String,
    accept: bool,
) -> Result<(), String> {
    let _mutation = state.mutations.lock().await;
    let network = state
        .network
        .lock()
        .await
        .clone()
        .ok_or_else(|| "局域网服务未启动".to_owned())?;
    network.decide_join(&request_id, accept).await
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStateSnapshot {
    listening: bool,
    port: Option<u16>,
    online_peers: Vec<String>,
    peer_statuses: std::collections::BTreeMap<String, PeerStatus>,
    recovery_warnings: std::collections::BTreeMap<String, String>,
}

#[tauri::command]
pub async fn p1_network_state(state: State<'_, P1State>) -> Result<NetworkStateSnapshot, String> {
    let network = state.network.lock().await;
    let Some(network) = network.as_ref() else {
        return Ok(NetworkStateSnapshot {
            listening: false,
            port: None,
            online_peers: Vec::new(),
            peer_statuses: std::collections::BTreeMap::new(),
            recovery_warnings: std::collections::BTreeMap::new(),
        });
    };
    Ok(NetworkStateSnapshot {
        listening: true,
        port: Some(network.port()),
        online_peers: network.peers().await,
        peer_statuses: network.peer_statuses().await,
        recovery_warnings: network.recovery_warnings().await,
    })
}

#[tauri::command]
pub async fn p1_resume_revalidate(
    app: AppHandle,
    state: State<'_, P1State>,
) -> Result<NetworkStateSnapshot, String> {
    resume_revalidate(&app, &state).await
}

pub async fn resume_revalidate<R: Runtime>(
    app: &AppHandle<R>,
    state: &P1State,
) -> Result<NetworkStateSnapshot, String> {
    let _mutation = state.mutations.lock().await;
    let mut network = state.network.lock().await.clone();
    if let Some(current) = network.as_ref() {
        if let Err(error) = current.revalidate().await {
            current.disconnect_all().await;
            state.network.lock().await.take();
            *state.network_error.lock().await = Some(format!(
                "局域网服务恢复检查失败，正在重建：{error}"
            ));
            network = None;
        }
    }
    if network.is_none() {
        let node = {
            let lifecycle = state.lifecycle.lock().await;
            match &*lifecycle {
                Lifecycle::Joined(node) => Some(Arc::clone(node)),
                Lifecycle::Cold | Lifecycle::Unjoined { .. } => None,
            }
        };
        let Some(node) = node else {
            reconcile_platform_service(app, state, false).await;
            return Ok(NetworkStateSnapshot {
                listening: false,
                port: None,
                online_peers: Vec::new(),
                peer_statuses: std::collections::BTreeMap::new(),
                recovery_warnings: std::collections::BTreeMap::new(),
            });
        };
        let started = start_services(node, state).await;
        reconcile_platform_service(app, state, started.is_ok()).await;
        started?;
        network = state.network.lock().await.clone();
    }
    let network = network.ok_or_else(|| "局域网服务恢复后仍不可用".to_owned())?;
    Ok(NetworkStateSnapshot {
        listening: true,
        port: Some(network.port()),
        online_peers: network.peers().await,
        peer_statuses: network.peer_statuses().await,
        recovery_warnings: network.recovery_warnings().await,
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberStateSnapshot {
    device_id: String,
    device_name: Option<String>,
    codex_capable: Option<bool>,
    protocol_version: Option<u16>,
    business_protocol: Option<ProtocolVersion>,
    capabilities: Vec<String>,
    local: bool,
    online: bool,
    status: PeerStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberListSnapshot {
    items: Vec<MemberStateSnapshot>,
    total: usize,
    truncated: bool,
}

const MAX_RENDERED_MEMBERS: usize = 500;

#[tauri::command]
pub async fn p1_members(state: State<'_, P1State>) -> Result<MemberListSnapshot, String> {
    let node = {
        let lifecycle = state.lifecycle.lock().await;
        let Lifecycle::Joined(node) = &*lifecycle else {
            return Err("尚未加入设备组".to_owned());
        };
        Arc::clone(node)
    };
    let network = state.network.lock().await.clone();
    let statuses = match network.as_ref() {
        Some(network) => network.peer_statuses().await,
        None => std::collections::BTreeMap::new(),
    };
    let negotiated = match network.as_ref() {
        Some(network) => network.peer_capabilities().await,
        None => std::collections::BTreeMap::new(),
    };
    let profiles: std::collections::BTreeMap<_, _> = node
        .active_member_profiles()
        .await
        .into_iter()
        .map(|profile| (profile.device_id.clone(), profile))
        .collect();
    let members: Vec<_> = node
        .active_member_ids()
        .await
        .into_iter()
        .map(|device_id| {
            let local = device_id == node.profile().device_id;
            let profile = profiles.get(&device_id);
            let protocol_version = profile
                .map(|profile| profile.protocol_version)
                .or_else(|| local.then_some(node.profile().protocol_version));
            let business = negotiated
                .get(&device_id)
                .cloned()
                .or_else(|| local.then(BusinessCapabilities::current));
            let status = if local {
                PeerStatus::Online
            } else {
                statuses
                    .get(&device_id)
                    .copied()
                    .unwrap_or(PeerStatus::Offline)
            };
            MemberStateSnapshot {
                device_name: profile
                    .map(|profile| profile.name.clone())
                    .or_else(|| local.then(|| node.device_name())),
                codex_capable: profile
                    .map(|profile| profile.codex_capable)
                    .or_else(|| local.then_some(node.profile().codex_capable)),
                protocol_version,
                business_protocol: business.as_ref().map(|value| value.protocol),
                capabilities: business
                    .map(|value| value.features.into_iter().collect())
                    .unwrap_or_default(),
                online: status == PeerStatus::Online,
                status,
                local,
                device_id,
            }
        })
        .collect();
    // Keep the local node and actionable peers visible when the list exceeds
    // the transport/display bound. Each priority group remains deterministic.
    let mut members = members;
    members.sort_by(|left, right| {
        let priority = |member: &MemberStateSnapshot| {
            if member.local {
                0
            } else if member.online {
                1
            } else if member.codex_capable == Some(true) {
                2
            } else {
                3
            }
        };
        priority(left)
            .cmp(&priority(right))
            .then_with(|| left.device_id.cmp(&right.device_id))
    });
    let total = members.len();
    let truncated = total > MAX_RENDERED_MEMBERS;
    Ok(MemberListSnapshot {
        items: members.into_iter().take(MAX_RENDERED_MEMBERS).collect(),
        total,
        truncated,
    })
}

#[tauri::command]
pub async fn p1_rename_device(
    state: State<'_, P1State>,
    device_name: String,
) -> Result<DeviceStateSnapshot, String> {
    let _mutation = state.mutations.lock().await;
    let device_name = validate_display_name(&device_name, "设备名称")?;
    let node = {
        let lifecycle = state.lifecycle.lock().await;
        let Lifecycle::Joined(node) = &*lifecycle else {
            return Err("尚未加入设备组".to_owned());
        };
        Arc::clone(node)
    };
    node.rename_local(device_name, format!("event:{}", uuid::Uuid::new_v4()))
        .await
        .map_err(|error| error.to_string())?;
    if let Some(network) = state.network.lock().await.clone() {
        if let Err(error) = network.broadcast_membership().await {
            return Err(format!("设备名称已修改，但部分在线节点等待重连同步：{error}"));
        }
    }
    Ok(DeviceStateSnapshot::joined(
        &node,
        background_error(&state).await,
    ))
}

#[tauri::command]
pub async fn p1_exit_group(
    app: AppHandle,
    state: State<'_, P1State>,
) -> Result<DeviceStateSnapshot, String> {
    let _mutation = state.mutations.lock().await;
    let mut lifecycle = state.lifecycle.lock().await;
    let previous = std::mem::take(&mut *lifecycle);
    let Lifecycle::Joined(node) = previous else {
        *lifecycle = previous;
        return Err("尚未加入设备组".to_owned());
    };
    if let Some(network) = state.network.lock().await.take() {
        network.disconnect_all().await;
    }
    #[cfg(not(target_os = "android"))]
    if let Some(source) = state.source.lock().await.take() {
        source.shutdown().await;
    }
    *state.source_error.lock().await = None;

    let storage = node.storage();
    if let Err(error) = storage.reset_group_state() {
        let restarted = start_services(Arc::clone(&node), &state).await;
        reconcile_platform_service(&app, &state, restarted.is_ok()).await;
        let restart_error = restarted.err();
        *lifecycle = Lifecycle::Joined(node);
        return Err(match restart_error {
            Some(restart) => format!("退出设备组失败：{error}；恢复服务失败：{restart}"),
            None => format!("退出设备组失败：{error}"),
        });
    }
    *state.network_error.lock().await = None;
    let identity = match (|| {
        let mut secrets = secret_store::open(&app).map_err(|error| error.to_string())?;
        DeviceIdentity::load_or_generate(secrets.as_mut()).map_err(|error| error.to_string())
    })() {
        Ok(identity) => identity,
        Err(error) => {
            *lifecycle = Lifecycle::Cold;
            stop_platform_service(&app, &state).await;
            return Err(error);
        }
    };
    *lifecycle = Lifecycle::Unjoined { identity, storage };
    *state.network_error.lock().await = None;
    stop_platform_service(&app, &state).await;
    Ok(snapshot(&lifecycle, background_error(&state).await))
}

#[tauri::command]
pub async fn p1_revoke_member(state: State<'_, P1State>, device_id: String) -> Result<(), String> {
    let _mutation = state.mutations.lock().await;
    let node = {
        let lifecycle = state.lifecycle.lock().await;
        let Lifecycle::Joined(node) = &*lifecycle else {
            return Err("尚未加入设备组".to_owned());
        };
        Arc::clone(node)
    };
    if device_id == node.profile().device_id {
        return Err("不能通过移除成员退出本机；退出设备组需要单独确认".to_owned());
    }
    if !node.is_member(&device_id).await {
        return Err("目标设备不是当前有效成员".to_owned());
    }
    node.revoke_member(
        device_id.clone(),
        format!("event:{}", uuid::Uuid::new_v4()),
    )
        .await
        .map_err(|error| error.to_string())?;
    let network = state.network.lock().await.clone();
    let broadcast_error = match network.as_ref() {
        Some(network) => network.broadcast_membership().await.err(),
        None => None,
    };
    let cleanup_error = node.storage().clear_observed_source(&device_id).err();
    if let Some(network) = network.as_ref() {
        network.disconnect_peer(&device_id).await;
    }
    if let Some(error) = cleanup_error {
        return Err(format!("成员已撤销，但清理其本地监控数据失败：{error}"));
    }
    if let Some(error) = broadcast_error {
        return Err(format!("成员已撤销，但部分在线节点等待重连同步：{error}"));
    }
    Ok(())
}

#[tauri::command]
pub async fn p1_connect_peer(state: State<'_, P1State>, address: String) -> Result<String, String> {
    let _mutation = state.mutations.lock().await;
    let value = address.trim().trim_end_matches('/');
    let value = value.strip_prefix("ws://").unwrap_or(value);
    if value.contains("://") {
        return Err("局域网直连仅支持 ws:// 地址".to_owned());
    }
    let value = value.strip_suffix("/device").unwrap_or(value);
    let address = value
        .parse::<std::net::SocketAddr>()
        .map_err(|_| "设备地址必须包含有效 IP 和端口".to_owned())?;
    let network = state
        .network
        .lock()
        .await
        .clone()
        .ok_or_else(|| "局域网服务未启动".to_owned())?;
    network.connect_manual(address).await
}

#[tauri::command]
pub async fn p1_join_group(
    app: AppHandle,
    state: State<'_, P1State>,
    address: String,
    short_code: String,
    device_name: String,
) -> Result<DeviceStateSnapshot, String> {
    let _mutation = state.mutations.lock().await;
    let device_name = validate_display_name(&device_name, "设备名称")?;
    if short_code.trim().is_empty() {
        return Err("加入码不能为空".to_owned());
    }
    let address = normalize_join_address(&address)?;
    let inviter_address = join_device_address(&address);
    let mut lifecycle = state.lifecycle.lock().await;
    let previous = std::mem::take(&mut *lifecycle);
    let Lifecycle::Unjoined { identity, storage } = previous else {
        *lifecycle = previous;
        return Err("当前状态不能加入设备组".to_owned());
    };
    let mut socket = match connect_join(&address).await {
        Ok(socket) => socket,
        Err(error) => {
            *lifecycle = Lifecycle::Unjoined { identity, storage };
            return Err(error.to_string());
        }
    };
    let member_profile = (&identity.profile(device_name, cfg!(not(target_os = "android")))).into();
    let (offer, trust, sealed) = match join_discovered_invitation_for_profile(
        &mut socket,
        member_profile,
        &identity,
        short_code.trim().as_bytes(),
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            *lifecycle = Lifecycle::Unjoined { identity, storage };
            return Err(error.to_string());
        }
    };
    let node = match JoinedNode::join_group(
        identity.clone(),
        Arc::clone(&storage),
        offer.group_name.clone(),
        device_name,
        cfg!(not(target_os = "android")),
        trust,
        &sealed,
    ) {
        Ok(node) => node,
        Err(error) => {
            *lifecycle = Lifecycle::Unjoined { identity, storage };
            return Err(error.to_string());
        }
    };
    let node = Arc::new(node);
    if let Some(inviter_address) = inviter_address {
        let candidate = AddressCandidate {
            device_id: Some(offer.inviter_id.clone()),
            address: inviter_address,
            source: CandidateSource::Remembered,
            observed_at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        };
        if let Err(error) = node.storage().put_address_candidate(
            &node.group().group_id,
            &offer.inviter_id,
            &candidate,
        ) {
            eprintln!("unable to remember authenticated inviter address: {error}");
        }
    }
    *lifecycle = Lifecycle::Joined(Arc::clone(&node));
    drop(lifecycle);
    let started = start_services(node, &state).await.is_ok();
    reconcile_platform_service(&app, &state, started).await;
    let lifecycle = state.lifecycle.lock().await;
    Ok(snapshot(&lifecycle, background_error(&state).await))
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStateSnapshot {
    capable: bool,
    ready: bool,
    mode: Option<String>,
    error: Option<String>,
}

#[tauri::command]
pub async fn p2_source_state(state: State<'_, P1State>) -> Result<SourceStateSnapshot, String> {
    #[cfg(not(target_os = "android"))]
    let source = state.source.lock().await.clone();
    #[cfg(not(target_os = "android"))]
    let runtime_error = source.as_ref().and_then(|source| source.health().err());
    #[cfg(not(target_os = "android"))]
    let ready = source.is_some() && runtime_error.is_none();
    #[cfg(target_os = "android")]
    let ready = false;

    Ok(SourceStateSnapshot {
        capable: cfg!(not(target_os = "android")),
        ready,
        mode: {
            #[cfg(not(target_os = "android"))]
            {
                source
                    .as_ref()
                    .map(|source| source.connection_mode().as_str().to_owned())
            }
            #[cfg(target_os = "android")]
            {
                None
            }
        },
        error: {
            #[cfg(not(target_os = "android"))]
            {
                runtime_error.or(state.source_error.lock().await.clone())
            }
            #[cfg(target_os = "android")]
            {
                state.source_error.lock().await.clone()
            }
        },
    })
}

#[tauri::command]
pub async fn p2_restart_source(
    app: AppHandle,
    state: State<'_, P1State>,
) -> Result<SourceStateSnapshot, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (&app, &state);
        return Err("Android 监控端不运行 Codex 来源服务".to_owned());
    }

    #[cfg(not(target_os = "android"))]
    {
        let _mutation = state.mutations.lock().await;
        restart_source_inner(&app, &state, false).await?;
        drop(_mutation);
        p2_source_state(state).await
    }
}

#[cfg(not(target_os = "android"))]
async fn restart_source_inner(
    app: &AppHandle,
    state: &P1State,
    await_shared_daemon: bool,
) -> Result<(), String> {
    let node = {
        let lifecycle = state.lifecycle.lock().await;
        let Lifecycle::Joined(node) = &*lifecycle else {
            return Err("尚未加入设备组".to_owned());
        };
        Arc::clone(node)
    };
    if let Some(network) = state.network.lock().await.take() {
        network.disconnect_all().await;
    }
    if let Some(source) = state.source.lock().await.take() {
        source.shutdown().await;
    }
    *state.source_error.lock().await = None;
    let started = start_services_with_source_policy(node, state, await_shared_daemon).await;
    reconcile_platform_service(app, state, started.is_ok()).await;
    started
}

#[tauri::command]
pub async fn p2_start_shared_source(
    app: AppHandle,
    state: State<'_, P1State>,
) -> Result<SourceStateSnapshot, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (&app, &state);
        return Err("Android 监控端不能启动电脑来源服务".to_owned());
    }

    #[cfg(not(target_os = "android"))]
    {
        let _mutation = state.mutations.lock().await;
        {
            let lifecycle = state.lifecycle.lock().await;
            if !matches!(&*lifecycle, Lifecycle::Joined(_)) {
                return Err("尚未加入设备组".to_owned());
            }
        }
        let mut command = tokio::process::Command::new("codex");
        command
            .args(["app-server", "daemon", "start"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(15), command.output())
            .await
            .map_err(|_| "启动共享 App Server daemon 超时".to_owned())?
            .map_err(|error| format!("无法启动共享 App Server daemon：{error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(600)
                .collect::<String>();
            return Err(format!(
                "共享 App Server daemon 启动失败（退出码 {}）{}",
                output
                    .status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "未知".to_owned()),
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("：{detail}")
                }
            ));
        }
        restart_source_inner(&app, &state, true).await?;
        drop(_mutation);
        let mut snapshot = p2_source_state(state).await?;
        if snapshot.ready && snapshot.mode.as_deref() != Some("sharedDaemon") {
            snapshot.error = Some(
                "共享 daemon 启动命令已完成，但代理连接未建立；当前仍使用历史回退来源"
                    .to_owned(),
            );
        }
        Ok(snapshot)
    }
}

#[tauri::command]
pub async fn p2_local_projects(state: State<'_, P1State>) -> Result<Vec<Project>, String> {
    let node = {
        let lifecycle = state.lifecycle.lock().await;
        let Lifecycle::Joined(node) = &*lifecycle else {
            return Err("尚未加入设备组".to_owned());
        };
        Arc::clone(node)
    };
    if !node.profile().codex_capable {
        return Ok(Vec::new());
    }
    node.storage()
        .projects(&node.profile().device_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn p2_add_local_project(
    state: State<'_, P1State>,
    name: String,
    path: String,
    available_models: Vec<String>,
) -> Result<Project, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (state, name, path, available_models);
        return Err("Android 监控端不能配置 Codex 项目".to_owned());
    }

    #[cfg(not(target_os = "android"))]
    {
        let _mutation = state.mutations.lock().await;
        let (name, canonical, models) =
            validate_project_input(name, path, available_models).await?;
        let node = {
            let lifecycle = state.lifecycle.lock().await;
            let Lifecycle::Joined(node) = &*lifecycle else {
                return Err("尚未加入设备组".to_owned());
            };
            Arc::clone(node)
        };
        let projects = node
            .storage()
            .projects(&node.profile().device_id)
            .map_err(|error| error.to_string())?;
        ensure_non_overlapping_project_path(&projects, None, &canonical)?;
        let source = state.source.lock().await.clone();
        let project = Project {
            project_id: format!("project:{}", uuid::Uuid::new_v4()),
            source_device_id: node.profile().device_id.clone(),
            name,
            path: canonical,
            available_models: models,
        };
        let result =
            crate::p2_source::add_project(source.as_deref(), &node.storage(), project).await;
        state.cache_epoch.fetch_add(1, Ordering::AcqRel);
        result
    }
}

#[cfg(not(target_os = "android"))]
async fn validate_project_input(
    name: String,
    path: String,
    available_models: Vec<String>,
) -> Result<(String, String, Vec<String>), String> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err("项目名称不能为空".to_owned());
    }
    let canonical = tauri::async_runtime::spawn_blocking(move || canonical_directory(&path))
        .await
        .map_err(|_| "项目目录校验任务异常结束".to_owned())??;
    let canonical = canonical
        .to_str()
        .ok_or_else(|| "项目路径不是 UTF-8".to_owned())?
        .to_owned();
    let mut models = available_models
        .into_iter()
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok((name, canonical, models))
}

#[cfg(not(target_os = "android"))]
fn canonical_directory(path: &str) -> Result<std::path::PathBuf, String> {
    let candidate = std::path::Path::new(path);
    if !candidate.is_absolute() {
        return Err("项目路径必须是绝对路径".to_owned());
    }
    let canonical =
        std::fs::canonicalize(candidate).map_err(|_| "项目目录不存在或不可访问".to_owned())?;
    if !canonical.is_dir() {
        return Err("项目路径必须是目录".to_owned());
    }
    Ok(normalize_windows_prefix(canonical))
}

#[cfg(not(target_os = "android"))]
fn normalize_windows_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let value = path.as_os_str().to_string_lossy();
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return std::path::PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(drive_path) = value.strip_prefix(r"\\?\") {
            return std::path::PathBuf::from(drive_path);
        }
    }
    path
}

#[cfg(not(target_os = "android"))]
fn ensure_non_overlapping_project_path(
    projects: &[Project],
    current_project_id: Option<&str>,
    candidate: &str,
) -> Result<(), String> {
    let candidate = comparable_project_path(candidate);
    for project in projects {
        if current_project_id == Some(project.project_id.as_str()) {
            continue;
        }
        let existing = comparable_project_path(&project.path);
        if candidate == existing
            || candidate.starts_with(&existing)
            || existing.starts_with(&candidate)
        {
            return Err(format!(
                "项目目录与现有项目“{}”重复或相互包含",
                project.name
            ));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn comparable_project_path(path: &str) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let normalized = normalize_windows_prefix(std::path::PathBuf::from(path));
        return std::path::PathBuf::from(
            normalized
                .to_string_lossy()
                .replace('\\', "/")
                .to_lowercase(),
        );
    }
    #[cfg(not(windows))]
    {
        std::path::PathBuf::from(path)
    }
}

#[tauri::command]
pub async fn p2_update_local_project(
    state: State<'_, P1State>,
    project_id: String,
    name: String,
    path: String,
    available_models: Vec<String>,
) -> Result<Project, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (state, project_id, name, path, available_models);
        return Err("Android 监控端不能配置 Codex 项目".to_owned());
    }

    #[cfg(not(target_os = "android"))]
    {
        let _mutation = state.mutations.lock().await;
        let (name, canonical, models) =
            validate_project_input(name, path, available_models).await?;
        let node = {
            let lifecycle = state.lifecycle.lock().await;
            let Lifecycle::Joined(node) = &*lifecycle else {
                return Err("尚未加入设备组".to_owned());
            };
            Arc::clone(node)
        };
        let projects = node
            .storage()
            .projects(&node.profile().device_id)
            .map_err(|error| error.to_string())?;
        if !projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err("项目不存在或不属于本机来源".to_owned());
        }
        ensure_non_overlapping_project_path(&projects, Some(&project_id), &canonical)?;
        let source = state.source.lock().await.clone();
        let project = Project {
            project_id,
            source_device_id: node.profile().device_id.clone(),
            name,
            path: canonical,
            available_models: models,
        };
        let result =
            crate::p2_source::add_project(source.as_deref(), &node.storage(), project).await;
        state.cache_epoch.fetch_add(1, Ordering::AcqRel);
        result
    }
}

#[tauri::command]
pub async fn p2_delete_local_project(
    state: State<'_, P1State>,
    project_id: String,
) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (state, project_id);
        return Err("Android 监控端不能配置 Codex 项目".to_owned());
    }

    #[cfg(not(target_os = "android"))]
    {
        let _mutation = state.mutations.lock().await;
        let node = {
            let lifecycle = state.lifecycle.lock().await;
            let Lifecycle::Joined(node) = &*lifecycle else {
                return Err("尚未加入设备组".to_owned());
            };
            Arc::clone(node)
        };
        let source = state.source.lock().await.clone();
        let result = crate::p2_source::delete_project(
            source.as_deref(),
            &node.storage(),
            &node.profile().device_id,
            &project_id,
        )
        .await;
        state.cache_epoch.fetch_add(1, Ordering::AcqRel);
        result
    }
}

#[tauri::command]
pub async fn p2_request(
    state: State<'_, P1State>,
    target_device_id: String,
    operation: Operation,
    params: Value,
    request_id: Option<String>,
) -> Result<ResponseEnvelope, String> {
    if !operation.is_monitoring() {
        return Err("当前版本仅提供只读监控，不支持远程控制操作".to_owned());
    }
    let cache_epoch = state.cache_epoch.load(Ordering::Acquire);
    let node = {
        let lifecycle = state.lifecycle.lock().await;
        let Lifecycle::Joined(node) = &*lifecycle else {
            return Err("尚未加入设备组".to_owned());
        };
        Arc::clone(node)
    };
    let request = RequestEnvelope {
        version: ProtocolVersion::V1,
        request_id: request_id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("request:{}", uuid::Uuid::new_v4())),
        source_device_id: node.profile().device_id.clone(),
        target_device_id: target_device_id.clone(),
        operation: operation.clone(),
        params: params.clone(),
    };

    #[cfg(not(target_os = "android"))]
    let mut local_source = None;
    let (response, request_network) = if target_device_id == node.profile().device_id {
        #[cfg(not(target_os = "android"))]
        {
            let source = state
                .source
                .lock()
                .await
                .clone()
                .ok_or_else(|| "Codex 来源服务尚未就绪".to_owned())?;
            let response = source
                .handle_request(&node.profile().device_id, request)
                .await
                .map_err(|error| error.to_string())?;
            local_source = Some(source);
            (response, None)
        }
        #[cfg(target_os = "android")]
        return Err("Android 监控端不提供 Codex 来源服务".to_owned());
    } else {
        let network = state
            .network
            .lock()
            .await
            .clone()
            .ok_or_else(|| "局域网服务未启动".to_owned())?;
        let response = network.request(&target_device_id, request).await?;
        (response, Some(network))
    };
    // The request itself must not block source maintenance. Once it returns,
    // serialize the short validation + cache commit with source restart and
    // group exit so an old response cannot repopulate freshly cleared state.
    let _cache_commit = state.mutations.lock().await;
    if target_device_id == node.profile().device_id
        && state.cache_epoch.load(Ordering::Acquire) != cache_epoch
    {
        return Err("本机监控配置已在请求期间变更".to_owned());
    }
    {
        let lifecycle = state.lifecycle.lock().await;
        if !matches!(&*lifecycle, Lifecycle::Joined(current) if Arc::ptr_eq(current, &node)) {
            return Err("设备组已在请求期间变更".to_owned());
        }
    }
    #[cfg(not(target_os = "android"))]
    if let Some(source) = local_source.as_ref() {
        if !state
            .source
            .lock()
            .await
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, source))
        {
            return Err("本机来源已在请求期间重启".to_owned());
        }
    }
    if let Some(network) = request_network.as_ref() {
        if !state
            .network
            .lock()
            .await
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, network))
        {
            return Err("局域网服务已在请求期间重启".to_owned());
        }
    }
    if response.error.is_none()
        && matches!(
            operation,
            Operation::ProjectsList
                | Operation::StateSubscribe
                | Operation::ThreadsList
                | Operation::ThreadsRead
                | Operation::ApprovalsList
        )
    {
        if operation == Operation::StateSubscribe {
            if let Some(generation) = response
                .result
                .as_ref()
                .and_then(|value| value.get("process_generation"))
                .cloned()
                .and_then(|value| serde_json::from_value::<ProcessGeneration>(value).ok())
            {
                let previous = node
                    .storage()
                    .cached_entry::<ProcessGeneration>(
                        &target_device_id,
                        SOURCE_GENERATION_CACHE_KEY,
                    )
                    .map_err(|error| error.to_string())?
                    .map(|entry| entry.value);
                let observed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                if previous != Some(generation) {
                    node.storage()
                        .begin_source_generation(&target_device_id, generation, observed)
                        .map_err(|error| error.to_string())?;
                } else {
                    node.storage()
                        .put_cache_with_observed(
                            &target_device_id,
                            SOURCE_GENERATION_CACHE_KEY,
                            &generation,
                            observed,
                        )
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        let key = cache_key(&target_device_id, &operation, &params);
        let observed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        node.storage()
            .put_cache_with_observed(&target_device_id, &key, &response, observed)
            .map_err(|e| e.to_string())?;
        node.storage()
            .evict_cache_to(OFFLINE_CACHE_MAX_BYTES)
            .map_err(|e| e.to_string())?;
    }
    Ok(response)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedResponse {
    pub payload: ResponseEnvelope,
    pub observed_at: i64,
}

fn cache_key(target: &str, operation: &Operation, params: &Value) -> String {
    let mut digest = Sha256::new();
    digest.update(target.as_bytes());
    digest.update([0]);
    digest.update(serde_json::to_vec(operation).unwrap_or_default());
    digest.update([0]);
    digest.update(serde_json::to_vec(params).unwrap_or_default());
    format!("request:{:x}", digest.finalize())
}

#[tauri::command]
pub async fn p2_cached_request(
    state: State<'_, P1State>,
    target_device_id: String,
    operation: Operation,
    params: Value,
) -> Result<Option<CachedResponse>, String> {
    if !matches!(
        operation,
        Operation::ProjectsList
            | Operation::StateSubscribe
            | Operation::ThreadsList
            | Operation::ThreadsRead
            | Operation::ApprovalsList
    ) {
        return Err("仅支持只读操作的缓存查询".into());
    }
    let node = {
        let lifecycle = state.lifecycle.lock().await;
        let Lifecycle::Joined(node) = &*lifecycle else {
            return Err("尚未加入设备组".into());
        };
        Arc::clone(node)
    };
    if target_device_id != node.profile().device_id && !node.is_member(&target_device_id).await {
        return Err("目标设备已不是当前设备组的有效成员".into());
    }
    let key = cache_key(&target_device_id, &operation, &params);
    let entry = node
        .storage()
        .cached_entry::<ResponseEnvelope>(&target_device_id, &key)
        .map_err(|e| e.to_string())?;
    Ok(entry.map(|e| CachedResponse {
        payload: e.value,
        observed_at: e.observed_at_unix_seconds,
    }))
}

#[tauri::command]
pub async fn p2_events(state: State<'_, P1State>) -> Result<Vec<EventEnvelope>, String> {
    let node = {
        let lifecycle = state.lifecycle.lock().await;
        let Lifecycle::Joined(node) = &*lifecycle else {
            return Err("尚未加入设备组".to_owned());
        };
        Arc::clone(node)
    };
    let persisted = node
        .storage()
        .source_events(1000)
        .map_err(|error| error.to_string())?;
    let live = match state.network.lock().await.clone() {
        Some(network) => network.cached_events().await,
        None => Vec::new(),
    };
    let mut merged = std::collections::BTreeMap::new();
    for event in persisted.into_iter().chain(live) {
        merged.insert(
            (
                event.source_device_id.clone(),
                event.process_generation,
                event.cursor,
            ),
            event,
        );
    }
    Ok(merged.into_values().collect())
}

#[tauri::command]
pub async fn p2_event_revision(state: State<'_, P1State>) -> Result<u64, String> {
    let network = state.network.lock().await.clone();
    Ok(match network {
        Some(network) => network.event_revision().await,
        None => 0,
    })
}

fn normalize_join_address(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return Err("设备地址不能为空".to_owned());
    }
    let base = if value.starts_with("ws://") {
        value.to_owned()
    } else if value.contains("://") {
        return Err("局域网加入仅支持 ws:// 地址".to_owned());
    } else {
        format!("ws://{value}")
    };
    if base.ends_with("/join") {
        Ok(base)
    } else {
        Ok(format!("{base}/join"))
    }
}

fn validate_display_name<'a>(value: &'a str, label: &str) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label}不能为空"));
    }
    if value.chars().count() > DISPLAY_NAME_MAX_CHARS {
        return Err(format!("{label}不能超过 {DISPLAY_NAME_MAX_CHARS} 个字符"));
    }
    Ok(value)
}

fn join_device_address(value: &str) -> Option<std::net::SocketAddr> {
    let url = url::Url::parse(value).ok()?;
    let port = url.port_or_known_default()?;
    let ip = match url.host()? {
        url::Host::Ipv4(ip) => std::net::IpAddr::V4(ip),
        url::Host::Ipv6(ip) => std::net::IpAddr::V6(ip),
        url::Host::Domain(_) => return None,
    };
    Some(std::net::SocketAddr::new(ip, port))
}
