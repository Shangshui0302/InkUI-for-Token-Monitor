mod discovery;
mod http;
mod pairing;
mod registry;

use codex_monitor_codex_client::CodexCollector;
use codex_monitor_protocol::{
    default_identity_path, default_pairing_identity_path, discovery_interface_v4,
    pairing_acceptance_message, pairing_code_proof, pairing_transcript, validate_node_identity,
    verify_pairing_signature, CollectorStatus, ControlAction, ControlResult, DiscoveryAnnouncement,
    Identity, MessageEnvelope, PairingIdentity, StateSnapshot, WireMessage, ANNOUNCEMENT_TTL_SECS,
    ANNOUNCE_INTERVAL_SECS, CONTROL_REQUEST_KIND, CONTROL_RESULT_KIND, DISCOVERY_GROUP_V4,
    DISCOVERY_PORT, HANDSHAKE_TIMEOUT_SECS, HEARTBEAT_INTERVAL_SECS, HEARTBEAT_TIMEOUT_SECS,
    MAX_WIRE_FRAME_BYTES, MONITOR_PORT, P256_PEER_SYNC_CAPABILITY, PEER_SYNC_CAPABILITY,
    PROTOCOL_VERSION, SERVICE_NAME, STATE_SNAPSHOT_CAPABILITY, STATE_SNAPSHOT_KIND,
};
use discovery::DiscoveryCache;
use pairing::{default_pairing_path, PairingCodeManager, PairingStore};
use rand::RngCore;
use registry::Registry;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket as StdUdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, oneshot, watch, Mutex, RwLock};
use tokio::time::timeout;

type BoxError = Box<dyn Error + Send + Sync>;

const MAX_CONTROL_REPLAY_ENTRIES: usize = 256;

#[derive(Default)]
struct ControlReplayGuard {
    entries: Mutex<BTreeMap<(String, String), u64>>,
}

impl ControlReplayGuard {
    async fn claim(&self, sender_id: &str, request_id: &str, expires_at: u64, now: u64) -> bool {
        let mut entries = self.entries.lock().await;
        entries.retain(|_, expiry| *expiry >= now);
        let key = (sender_id.to_string(), request_id.to_string());
        if entries.contains_key(&key) {
            return false;
        }
        // ponytail: bounded process-local replay cache; durable idempotency is
        // unnecessary while control requests expire within two minutes.
        if entries.len() >= MAX_CONTROL_REPLAY_ENTRIES {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, expiry)| *expiry)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(key, expires_at);
        true
    }
}

#[derive(Default)]
struct PeerRouter {
    routes: Mutex<BTreeMap<String, BTreeMap<u64, PeerConnection>>>,
    next_generation: AtomicU64,
}

struct PeerConnection {
    control_sender: Option<mpsc::Sender<http::ControlCommand>>,
}

impl PeerRouter {
    async fn register(&self, node_id: String, sender: mpsc::Sender<http::ControlCommand>) -> u64 {
        self.register_connection(node_id, Some(sender)).await
    }

    async fn register_connection(
        &self,
        node_id: String,
        control_sender: Option<mpsc::Sender<http::ControlCommand>>,
    ) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.routes
            .lock()
            .await
            .entry(node_id)
            .or_default()
            .insert(generation, PeerConnection { control_sender });
        generation
    }

    async fn unregister(&self, node_id: &str, generation: u64) -> bool {
        let mut routes = self.routes.lock().await;
        let Some(peer_routes) = routes.get_mut(node_id) else {
            return true;
        };
        peer_routes.remove(&generation);
        if peer_routes.is_empty() {
            routes.remove(node_id);
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    async fn sender_for(&self, node_id: &str) -> Option<mpsc::Sender<http::ControlCommand>> {
        self.routes
            .lock()
            .await
            .get(node_id)
            .and_then(|peer_routes| {
                peer_routes
                    .values()
                    .rev()
                    .filter_map(|connection| connection.control_sender.as_ref())
                    .find(|sender| !sender.is_closed())
                    .cloned()
            })
    }

    async fn is_connected(&self, node_id: &str) -> bool {
        self.routes
            .lock()
            .await
            .get(node_id)
            .is_some_and(|connections| !connections.is_empty())
    }

    async fn send(
        &self,
        node_id: &str,
        mut command: http::ControlCommand,
    ) -> Result<(), http::ControlCommand> {
        let mut attempted = BTreeSet::new();
        loop {
            let candidate = {
                let routes = self.routes.lock().await;
                routes.get(node_id).and_then(|peer_routes| {
                    peer_routes
                        .iter()
                        .rev()
                        .filter_map(|(generation, connection)| {
                            connection
                                .control_sender
                                .as_ref()
                                .filter(|sender| !sender.is_closed())
                                .map(|sender| (*generation, sender.clone()))
                        })
                        .find(|(generation, _)| !attempted.contains(generation))
                })
            };
            let Some((generation, sender)) = candidate else {
                return Err(command);
            };
            attempted.insert(generation);
            match sender.send(command).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    command = error.0;
                    // A closed queue means its connection loop has already
                    // stopped; remove only this generation, then try older
                    // routes while preserving any concurrently registered one.
                    self.unregister(node_id, generation).await;
                }
            }
        }
    }
}

async fn run_control_router(
    mut control_rx: mpsc::Receiver<http::ControlCommand>,
    local_node_id: String,
    local_tx: mpsc::Sender<http::ControlCommand>,
    peers: Arc<PeerRouter>,
) {
    while let Some(command) = control_rx.recv().await {
        let target = command.request.target_node_id.clone();
        if target
            .as_deref()
            .is_none_or(|target| target == local_node_id)
        {
            if let Err(error) = local_tx.send(command).await {
                respond_control_failure(error.0, "local collector is unavailable");
            }
            continue;
        }
        if let Err(command) = peers
            .send(target.as_deref().unwrap_or_default(), command)
            .await
        {
            respond_control_failure(command, "target peer is not connected");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let identity_path = default_identity_path("monitor");
    let identity_existed = identity_path.exists();
    let identity = Arc::new(Identity::load_or_create(&identity_path)?);
    let pairing_identity = Arc::new(PairingIdentity::load_or_create(
        &default_pairing_identity_path("monitor"),
    )?);
    let monitor_id = identity.host_id();
    let alias_path = identity_path.with_file_name("monitor.alias");
    let alias_value = load_alias(
        &alias_path,
        env::var("CODEX_MONITOR_ALIAS").ok().as_deref(),
        identity_existed,
        &monitor_id,
    );
    let alias = Arc::new(RwLock::new(alias_value));
    let pairing_codes = PairingCodeManager::new();
    let network_id = env::var("CODEX_MONITOR_NETWORK_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "codex-monitor-lan".to_string());
    let pairings = Arc::new(Mutex::new(PairingStore::load(default_pairing_path())?));
    let registry = Arc::new(RwLock::new(Registry::load(
        default_state_cache_path(),
        Some(&network_id),
    )?));
    let (local_snapshot_tx, local_snapshot_rx) = watch::channel::<Option<StateSnapshot>>(None);
    let (control_tx, control_rx) = mpsc::channel(16);
    let (local_control_tx, local_control_rx) = mpsc::channel(16);
    let peer_router = Arc::new(PeerRouter::default());
    let replay_guard = Arc::new(ControlReplayGuard::default());
    let discoveries = Arc::new(Mutex::new(DiscoveryCache::default()));
    let (pair_tx, pair_rx) = mpsc::channel(8);

    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, MONITOR_PORT)).await?;
    let discovery_interface = discovery_interface_v4()?;
    let announcement_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    announcement_socket.bind(&SockAddr::from(SocketAddr::from((
        Ipv4Addr::UNSPECIFIED,
        0,
    ))))?;
    announcement_socket.set_multicast_if_v4(&discovery_interface)?;
    announcement_socket.set_multicast_ttl_v4(1)?;
    // Some WLANs suppress multicast while still allowing a local broadcast.
    // Keep the same authenticated announcement as a small discovery fallback.
    announcement_socket.set_broadcast(true)?;
    // Keep loopback enabled so a Monitor-local Agent (the Windows host's own
    // collector) can use the same discovery path as remote Agents.
    announcement_socket.set_multicast_loop_v4(true)?;
    let announcement_socket: StdUdpSocket = announcement_socket.into();
    announcement_socket.set_nonblocking(true)?;
    let announcement_socket = UdpSocket::from_std(announcement_socket)?;
    let announcement_socket_v6 = match StdUdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)) {
        Ok(socket) => match socket.set_nonblocking(true) {
            Ok(()) => match UdpSocket::from_std(socket) {
                Ok(socket) => Some(socket),
                Err(error) => {
                    eprintln!("IPv6 discovery target socket unavailable: {error}");
                    None
                }
            },
            Err(error) => {
                eprintln!("IPv6 discovery target socket setup failed: {error}");
                None
            }
        },
        Err(error) => {
            eprintln!("IPv6 discovery target socket unavailable: {error}");
            None
        }
    };
    let discovery_targets = discovery_target_specs();

    let monitor_public_key = identity.public_key_hex();
    let monitor_pairing_public_key = pairing_identity.public_key();
    let capabilities = vec![
        "sessions".to_string(),
        "control".to_string(),
        "usage".to_string(),
        STATE_SNAPSHOT_CAPABILITY.to_string(),
        PEER_SYNC_CAPABILITY.to_string(),
    ];
    let multicast_addr = SocketAddr::from((Ipv4Addr::from(DISCOVERY_GROUP_V4), DISCOVERY_PORT));

    eprintln!("codex-monitor-node id={monitor_id} listening on 0.0.0.0:{MONITOR_PORT}");
    eprintln!("monitor public key: {monitor_public_key}");
    eprintln!("network id: {network_id}");
    eprintln!("pairing code: fetch from authorized GET /api/v1/pairing-code");
    eprintln!("discovery group: {multicast_addr}");
    eprintln!("discovery interface: {discovery_interface}");
    if !discovery_targets.is_empty() {
        eprintln!("unicast discovery targets: {}", discovery_targets.join(","));
    }

    registry.write().await.register(
        monitor_id.clone(),
        alias.read().await.clone(),
        capabilities.clone(),
        unix_seconds(),
    );
    registry.write().await.set_identity(
        monitor_id.clone(),
        alias.read().await.clone(),
        Some(network_id.clone()),
        capabilities.clone(),
    );
    let http_bind = env::var("CODEX_MONITOR_HTTP_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8788".to_string())
        .parse::<SocketAddr>()?;
    let http_port = http_bind.port();
    let advertised_http_port = (!http_bind.ip().is_loopback()).then_some(http_port);
    let read_token = env::var("CODEX_MONITOR_READ_TOKEN")
        .ok()
        .filter(|value| !value.is_empty());
    let control_token = env::var("CODEX_MONITOR_CONTROL_TOKEN")
        .ok()
        .filter(|value| !value.is_empty());
    eprintln!(
        "read-only HTTP token: {}",
        if read_token.is_some() {
            "enabled"
        } else {
            "disabled"
        }
    );
    eprintln!(
        "local control token: {}",
        if control_token.is_some() {
            "enabled"
        } else {
            "disabled"
        }
    );
    tokio::spawn(run_control_router(
        control_rx,
        monitor_id.clone(),
        local_control_tx.clone(),
        Arc::clone(&peer_router),
    ));
    let http_registry = Arc::clone(&registry);
    let http_control_tx = control_tx.clone();
    let http_discoveries = Arc::clone(&discoveries);
    let http_network_id = network_id.clone();
    let http_alias = Arc::clone(&alias);
    let http_pairing_codes = pairing_codes.clone();
    tokio::spawn(async move {
        if let Err(error) = http::serve(
            http_bind,
            http_registry,
            read_token,
            control_token,
            http_control_tx,
            http_discoveries,
            http_network_id,
            pair_tx,
            http_alias,
            alias_path,
            http_pairing_codes,
        )
        .await
        {
            eprintln!("HTTP API stopped: {error}");
        }
    });
    tokio::spawn(run_local_collector(
        Arc::clone(&registry),
        monitor_id.clone(),
        Arc::clone(&alias),
        network_id.clone(),
        local_snapshot_tx.clone(),
        local_control_rx,
    ));

    tokio::spawn(run_peer_discovery(
        Arc::clone(&identity),
        Arc::clone(&pairing_identity),
        monitor_id.clone(),
        Arc::clone(&alias),
        network_id.clone(),
        Arc::clone(&pairings),
        Arc::clone(&registry),
        local_snapshot_rx.clone(),
        local_control_tx.clone(),
        Arc::clone(&peer_router),
        Arc::clone(&replay_guard),
        advertised_http_port,
        discovery_interface,
        Arc::clone(&discoveries),
        pair_rx,
    ));

    let announce_monitor_id = monitor_id.clone();
    let announce_alias = Arc::clone(&alias);
    let announce_public_key = monitor_public_key.clone();
    let announce_pairing_public_key = monitor_pairing_public_key.clone();
    let announce_capabilities = capabilities.clone();
    let announce_network_id = network_id.clone();
    let announce_http_port = advertised_http_port;
    let announce_target_specs = discovery_targets.clone();
    let broadcast_addr = SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT));
    let _announce_task = tokio::spawn(async move {
        loop {
            let now = unix_seconds();
            let announcement = DiscoveryAnnouncement {
                service: SERVICE_NAME.to_string(),
                version: PROTOCOL_VERSION,
                host_id: announce_monitor_id.clone(),
                alias: announce_alias.read().await.clone(),
                os: Some(std::env::consts::OS.to_string()),
                port: MONITOR_PORT,
                http_port: announce_http_port,
                public_key: announce_public_key.clone(),
                pairing_public_key: Some(announce_pairing_public_key.clone()),
                nonce: random_hex(16),
                issued_at: now,
                expires_at: now + ANNOUNCEMENT_TTL_SECS,
                capabilities: announce_capabilities.clone(),
                network_id: Some(announce_network_id.clone()),
            };
            let announcement_bytes = match serde_json::to_vec(&announcement) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("discovery announce serialization failed: {error}");
                    tokio::time::sleep(Duration::from_secs(ANNOUNCE_INTERVAL_SECS)).await;
                    continue;
                }
            };
            if let Err(error) = announcement_socket
                .send_to(&announcement_bytes, multicast_addr)
                .await
            {
                eprintln!("discovery announce failed: {error}");
            }
            if let Err(error) = announcement_socket
                .send_to(&announcement_bytes, broadcast_addr)
                .await
            {
                eprintln!("discovery broadcast fallback failed: {error}");
            }
            send_discovery_targets(
                &announcement_socket,
                announcement_socket_v6.as_ref(),
                &announcement_bytes,
                &announce_target_specs,
            )
            .await;
            tokio::time::sleep(Duration::from_secs(ANNOUNCE_INTERVAL_SECS)).await;
        }
    });

    loop {
        let (stream, peer) = listener.accept().await?;
        let pairings = Arc::clone(&pairings);
        let pairing_codes = pairing_codes.clone();
        let monitor_id = monitor_id.clone();
        let monitor_identity = Arc::clone(&identity);
        let monitor_pairing_identity = Arc::clone(&pairing_identity);
        let registry = Arc::clone(&registry);
        let network_id = network_id.clone();
        let local_snapshot_rx = local_snapshot_rx.clone();
        let local_control_tx = local_control_tx.clone();
        let peer_router = Arc::clone(&peer_router);
        let replay_guard = Arc::clone(&replay_guard);
        let agent_discoveries = Arc::clone(&discoveries);
        tokio::spawn(async move {
            if let Err(error) = handle_agent(
                stream,
                peer,
                &monitor_id,
                monitor_identity,
                monitor_pairing_identity,
                Arc::clone(&pairings),
                pairing_codes,
                registry,
                network_id,
                local_snapshot_rx,
                local_control_tx,
                peer_router,
                replay_guard,
                agent_discoveries,
            )
            .await
            {
                eprintln!("agent {peer} session ended: {error}");
            }
        });
    }
}

async fn handle_agent(
    stream: TcpStream,
    peer: SocketAddr,
    monitor_id: &str,
    monitor_identity: Arc<Identity>,
    monitor_pairing_identity: Arc<PairingIdentity>,
    pairings: Arc<Mutex<PairingStore>>,
    pairing_codes: PairingCodeManager,
    registry: Arc<RwLock<Registry>>,
    network_id: String,
    mut local_snapshot_rx: watch::Receiver<Option<StateSnapshot>>,
    local_control_tx: mpsc::Sender<http::ControlCommand>,
    peer_router: Arc<PeerRouter>,
    replay_guard: Arc<ControlReplayGuard>,
    discoveries: Arc<Mutex<DiscoveryCache>>,
) -> Result<(), BoxError> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let hello = timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        read_wire(&mut reader),
    )
    .await??
    .ok_or("agent closed before hello")?;
    let WireMessage::AgentHello {
        version,
        network_id: supplied_network_id,
        host_id,
        alias,
        os,
        http_port: supplied_http_port,
        public_key,
        pairing_public_key,
        capabilities,
    } = hello
    else {
        send_wire(
            &mut write_half,
            &WireMessage::PairRejected {
                reason: "first message must be AgentHello".to_string(),
            },
        )
        .await?;
        return Err("invalid first message".into());
    };

    if version != PROTOCOL_VERSION {
        send_wire(
            &mut write_half,
            &WireMessage::PairRejected {
                reason: "unsupported protocol version".to_string(),
            },
        )
        .await?;
        return Err("unsupported protocol version".into());
    }
    if let Some(supplied_network_id) = supplied_network_id.as_deref() {
        if !supplied_network_id.is_empty() && supplied_network_id != network_id {
            send_wire(
                &mut write_half,
                &WireMessage::PairRejected {
                    reason: "network id mismatch".to_string(),
                },
            )
            .await?;
            return Err("network id mismatch".into());
        }
    }
    let valid_identity =
        validate_node_identity(&host_id, &public_key, pairing_public_key.as_deref()).is_ok();
    if host_id.is_empty()
        || host_id.len() > 128
        || alias.is_empty()
        || alias.len() > 128
        || os
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 64)
        || !valid_identity
        || supplied_http_port == Some(0)
        || capabilities.len() > 32
        || capabilities.iter().any(|value| value.len() > 64)
    {
        return Err("invalid agent identity".into());
    }
    let known_key = pairings
        .lock()
        .await
        .public_key_for(&host_id)
        .map(str::to_string);
    let pairing_public_key = pairing_public_key.as_deref();
    let mut pairing_reservation = None;
    let handshake_public_key = if let Some(expected_key) = known_key.as_deref() {
        if expected_key == public_key {
            expected_key.to_string()
        } else if pairing_public_key == Some(expected_key) {
            expected_key.to_string()
        } else {
            send_wire(
                &mut write_half,
                &WireMessage::PairRejected {
                    reason: "host id is already paired with another key".to_string(),
                },
            )
            .await?;
            return Err("paired key mismatch".into());
        }
    } else {
        pairing_public_key.unwrap_or(&public_key).to_string()
    };

    let challenge = random_hex(32);
    let challenge_uses_p256 = known_key
        .as_deref()
        .map(|key| key.starts_with("p256:"))
        .unwrap_or_else(|| pairing_public_key.is_some() || public_key.starts_with("p256:"));
    let challenge_key = if challenge_uses_p256 {
        monitor_pairing_identity.public_key()
    } else {
        monitor_identity.public_key_hex()
    };
    let local_public_key = monitor_identity.public_key_hex();
    let local_pairing_key = monitor_pairing_identity.public_key();
    let remote_pairing_key = pairing_public_key.unwrap_or(&public_key);
    let transcript = pairing_transcript(
        &host_id,
        &public_key,
        remote_pairing_key,
        monitor_id,
        &local_public_key,
        &local_pairing_key,
        &network_id,
        &challenge,
    );
    let challenge_signature = if challenge_uses_p256 {
        monitor_pairing_identity.sign_hex(&transcript)
    } else {
        monitor_identity.sign_hex(&transcript)
    };
    send_wire(
        &mut write_half,
        &WireMessage::PairChallenge {
            challenge: challenge.clone(),
            monitor_id: monitor_id.to_string(),
            signature: challenge_signature,
            public_key: Some(local_public_key),
            pairing_public_key: challenge_uses_p256.then_some(challenge_key),
        },
    )
    .await?;
    let response = timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        read_wire(&mut reader),
    )
    .await??
    .ok_or("agent closed before pair response")?;
    let WireMessage::PairResponse {
        host_id: response_host_id,
        signature,
        code_proof,
    } = response
    else {
        return Err("expected pair response".into());
    };
    if response_host_id != host_id
        || !verify_pairing_signature(&handshake_public_key, &transcript, &signature)
    {
        send_wire(
            &mut write_half,
            &WireMessage::PairRejected {
                reason: "challenge signature rejected".to_string(),
            },
        )
        .await?;
        return Err("challenge signature rejected".into());
    }

    if known_key.is_none() {
        let Some(proof) = code_proof.as_deref() else {
            send_wire(
                &mut write_half,
                &WireMessage::PairRejected {
                    reason: "pairing code proof required".to_string(),
                },
            )
            .await?;
            return Err("pairing code proof required".into());
        };
        match pairing_codes.reserve_proof(proof, "initiator", &transcript) {
            Some(reservation) => pairing_reservation = Some(reservation),
            None => {
                send_wire(
                    &mut write_half,
                    &WireMessage::PairRejected {
                        reason: "pairing code proof rejected".to_string(),
                    },
                )
                .await?;
                return Err("pairing code proof rejected".into());
            }
        }
    } else if code_proof.is_some() {
        return Err("unexpected pairing code proof".into());
    }
    let acceptance_code_proof = pairing_reservation
        .as_ref()
        .map(|_| pairing_codes.reserved_proof("acceptor", &transcript));

    let persisted = {
        let mut pairings = pairings.lock().await;
        if let Some(reservation) = pairing_reservation.take() {
            match pairing_codes.commit(reservation, || {
                pairings.remember(&host_id, &handshake_public_key)
            }) {
                Ok(()) => Ok(()),
                Err(pairing::PairingCommitError::InvalidReservation) => {
                    Err("pairing code was refreshed before pairing completed".to_string())
                }
                Err(pairing::PairingCommitError::Store(error)) => Err(error.to_string()),
            }
        } else {
            pairings
                .remember(&host_id, &handshake_public_key)
                .map_err(|error| error.to_string())
        }
    };
    if let Err(reason) = persisted {
        let _ = send_wire(
            &mut write_half,
            &WireMessage::PairRejected {
                reason: reason.clone(),
            },
        )
        .await;
        return Err(reason.into());
    }
    send_wire(
        &mut write_half,
        &WireMessage::PairAccepted {
            host_id: host_id.clone(),
            network_id: Some(network_id.clone()),
            signature: Some(if challenge_uses_p256 {
                monitor_pairing_identity.sign_hex(&pairing_acceptance_message(&transcript))
            } else {
                monitor_identity.sign_hex(&pairing_acceptance_message(&transcript))
            }),
            code_proof: acceptance_code_proof,
        },
    )
    .await?;
    eprintln!("paired agent {alias} ({host_id}) from {peer}: {capabilities:?}");
    let state_supported = capabilities
        .iter()
        .any(|value| value == STATE_SNAPSHOT_CAPABILITY);
    let peer_sync_supported = capabilities
        .iter()
        .any(|value| value == PEER_SYNC_CAPABILITY);
    let p256_peer_sync_supported = capabilities
        .iter()
        .any(|value| value == P256_PEER_SYNC_CAPABILITY);
    let control_supported = capabilities.iter().any(|value| value == "control");
    registry.write().await.register(
        host_id.clone(),
        alias.clone(),
        capabilities.clone(),
        unix_seconds(),
    );
    registry.write().await.set_os(&host_id, os);
    discoveries.lock().await.mark_connected(&host_id);
    if let Some(http_port) = supplied_http_port {
        registry.write().await.set_http_endpoints(
            &host_id,
            http_endpoint_candidates(peer.ip(), http_port, &alias),
        );
    }

    if state_supported {
        let snapshot = local_snapshot_rx.borrow().clone();
        if let Some(snapshot) = snapshot {
            send_snapshot(
                &mut write_half,
                &monitor_identity,
                &monitor_pairing_identity,
                &network_id,
                snapshot,
                peer_sync_supported,
                p256_peer_sync_supported,
            )
            .await?;
        }
    }
    let (peer_control_tx, mut peer_control_rx) = mpsc::channel(8);
    let peer_route_generation = Some(if control_supported {
        peer_router.register(host_id.clone(), peer_control_tx).await
    } else {
        peer_router.register_connection(host_id.clone(), None).await
    });
    let mut pending_controls = BTreeMap::new();
    let mut control_sequence = 0u64;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    let result: Result<(), BoxError> = loop {
        let read = timeout(
            Duration::from_secs(HEARTBEAT_TIMEOUT_SECS),
            read_wire(&mut reader),
        );
        tokio::select! {
            biased;
            changed = local_snapshot_rx.changed(), if state_supported => {
                if changed.is_err() {
                    break Err("local snapshot channel closed".into());
                }
                let snapshot = local_snapshot_rx.borrow().clone();
                if let Some(snapshot) = snapshot {
                    send_snapshot(
                        &mut write_half,
                        &monitor_identity,
                        &monitor_pairing_identity,
                        &network_id,
                        snapshot,
                        peer_sync_supported,
                        p256_peer_sync_supported,
                    )
                    .await?;
                }
                continue;
            }
            command = peer_control_rx.recv(), if control_supported => {
                let Some(command) = command else {
                    break Err("peer control channel closed".into());
                };
                if let Err(reason) = command.request.validate(unix_seconds()) {
                    respond_control_failure(command, reason);
                    continue;
                }
                let request_id = command.request.request_id.clone();
                if pending_controls.contains_key(&request_id) {
                    respond_control_failure(command, "duplicate control request id");
                    continue;
                }
                let respond_to = command.respond_to;
                let envelope = MessageEnvelope::new(
                    CONTROL_REQUEST_KIND,
                    &network_id,
                    monitor_identity.host_id(),
                    control_sequence.saturating_add(1),
                    unix_seconds(),
                    Some(request_id.clone()),
                    command.request,
                );
                control_sequence = envelope.sequence;
                let envelope = match monitor_identity.signed_envelope(envelope) {
                    Ok(envelope) => envelope,
                    Err(error) => {
                        let _ = respond_to.send(ControlResult {
                            request_id,
                            ok: false,
                            message: format!("sign control request: {error}"),
                        });
                        continue;
                    }
                };
                pending_controls.insert(request_id.clone(), respond_to);
                if let Err(error) = send_wire(
                    &mut write_half,
                    &WireMessage::ControlRequest { envelope },
                )
                .await
                {
                    if let Some(respond_to) = pending_controls.remove(&request_id) {
                        let _ = respond_to.send(ControlResult {
                            request_id,
                            ok: false,
                            message: format!("send control request: {error}"),
                        });
                    }
                    break Err(error);
                }
            }
            result = read => {
                let message = match result {
                    Ok(Ok(message)) => message,
                    Ok(Err(error)) => break Err(error),
                    Err(error) => break Err(error.into()),
                };
                match message {
                    Some(WireMessage::Heartbeat { host_id: heartbeat_host_id }) if heartbeat_host_id == host_id => {
                        registry.write().await.touch(&host_id, unix_seconds());
                        if let Err(error) = send_wire(&mut write_half, &WireMessage::HeartbeatAck).await {
                            break Err(error);
                        }
                    }
                    Some(WireMessage::Heartbeat { .. }) => break Err("heartbeat host id mismatch".into()),
                    Some(WireMessage::HeartbeatAck) => {}
                    Some(WireMessage::StateSnapshot { snapshot }) if state_supported => {
                        if snapshot.host_id != host_id
                            || snapshot.source_id() != host_id
                            || snapshot
                                .network_id
                                .as_deref()
                                .is_some_and(|value| !value.is_empty() && value != network_id)
                        {
                            break Err("state snapshot host id mismatch".into());
                        }
                        if let Err(reason) = registry.write().await.update_snapshot(snapshot, unix_seconds()) {
                            break Err(reason.into());
                        }
                    }
                    Some(WireMessage::PeerSnapshot { envelope }) if peer_sync_supported => {
                        let snapshot = match accept_peer_snapshot(
                            envelope,
                            &public_key,
                            &host_id,
                            &network_id,
                        ) {
                            Ok(snapshot) => snapshot,
                            Err(reason) => break Err(reason.into()),
                        };
                        if let Err(reason) = registry
                            .write()
                            .await
                            .update_snapshot(snapshot, unix_seconds())
                        {
                            break Err(reason.into());
                        }
                    }
                    Some(WireMessage::PeerSnapshot { .. }) => {
                        eprintln!("agent {host_id} sent peer state without negotiated capability");
                    }
                    Some(WireMessage::ControlRequest { envelope }) if control_supported => {
                        handle_incoming_control(
                            &mut write_half,
                            envelope,
                            &public_key,
                            &host_id,
                            monitor_id,
                            &network_id,
                            &monitor_identity,
                            &local_control_tx,
                            &replay_guard,
                        )
                        .await?;
                    }
                    Some(WireMessage::ControlRequest { .. }) => {
                        eprintln!("agent {host_id} sent control without negotiated capability");
                    }
                    Some(WireMessage::ControlResponse { envelope }) if control_supported => {
                        let response = match accept_control_response(
                            envelope,
                            &public_key,
                            &host_id,
                            &network_id,
                        ) {
                            Ok(response) => response,
                            Err(reason) => break Err(reason.into()),
                        };
                        if let Some(respond_to) = pending_controls.remove(&response.request_id) {
                            let _ = respond_to.send(response);
                        } else {
                            eprintln!("agent {host_id} sent an unknown control response");
                        }
                    }
                    Some(WireMessage::ControlResponse { .. }) => {
                        eprintln!("agent {host_id} sent control without negotiated capability");
                    }
                    Some(WireMessage::StateSnapshot { .. }) => {
                        eprintln!("agent {host_id} sent state without negotiated capability");
                    }
                    Some(other) => eprintln!("agent {host_id} sent unsupported message: {other:?}"),
                    None => break Ok(()),
                }
            }
            _ = heartbeat.tick() => {
                if let Err(error) = send_wire(
                    &mut write_half,
                    &WireMessage::Heartbeat {
                        host_id: monitor_id.to_string(),
                    },
                )
                .await
                {
                    break Err(error);
                }
            }
        }
    };
    let should_disconnect = match peer_route_generation {
        Some(generation) => peer_router.unregister(&host_id, generation).await,
        None => true,
    };
    for (request_id, respond_to) in pending_controls {
        let _ = respond_to.send(ControlResult {
            request_id,
            ok: false,
            message: "peer connection closed before control completed".to_string(),
        });
    }
    if should_disconnect {
        registry.write().await.disconnect(&host_id, unix_seconds());
        if pairings.lock().await.public_key_for(&host_id).is_some() {
            discoveries.lock().await.mark_paired(&host_id);
        }
    }
    result
}

async fn handle_incoming_control<W>(
    writer: &mut W,
    envelope: MessageEnvelope<codex_monitor_protocol::ControlRequest>,
    public_key: &str,
    expected_sender: &str,
    local_node_id: &str,
    network_id: &str,
    identity: &Identity,
    local_control_tx: &mpsc::Sender<http::ControlCommand>,
    replay_guard: &ControlReplayGuard,
) -> Result<(), BoxError>
where
    W: AsyncWrite + Unpin,
{
    envelope.validate()?;
    if envelope.kind != CONTROL_REQUEST_KIND
        || envelope.sender_id != expected_sender
        || envelope.network_id != network_id
        || envelope.sequence == 0
        || !Identity::verify_envelope(public_key, &envelope)
    {
        return Err("control request signature or identity rejected".into());
    }
    let sequence = envelope.sequence;
    let envelope_request_id = envelope.request_id.clone();
    let request = envelope.payload;
    request.validate(unix_seconds())?;
    if envelope_request_id.as_deref() != Some(request.request_id.as_str())
        || request
            .target_node_id
            .as_deref()
            .is_some_and(|target| target != local_node_id)
    {
        return Err("control request signature, target, or identity rejected".into());
    }

    let request_id = request.request_id.clone();
    if !replay_guard
        .claim(
            expected_sender,
            &request_id,
            request.expires_at,
            unix_seconds(),
        )
        .await
    {
        return send_control_response(
            writer,
            identity,
            network_id,
            sequence,
            ControlResult {
                request_id,
                ok: false,
                message: "duplicate control request id".to_string(),
            },
        )
        .await;
    }
    let (respond_to, response) = tokio::sync::oneshot::channel();
    let result = match local_control_tx
        .send(http::ControlCommand {
            request,
            respond_to,
        })
        .await
    {
        Ok(()) => match timeout(Duration::from_secs(15), response).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => ControlResult {
                request_id: request_id.clone(),
                ok: false,
                message: "local collector closed before handling the request".to_string(),
            },
            Err(_) => ControlResult {
                request_id: request_id.clone(),
                ok: false,
                message: "local control request timed out".to_string(),
            },
        },
        Err(_) => ControlResult {
            request_id: request_id.clone(),
            ok: false,
            message: "local collector is unavailable".to_string(),
        },
    };
    send_control_response(writer, identity, network_id, sequence, result).await
}

async fn send_control_response<W>(
    writer: &mut W,
    identity: &Identity,
    network_id: &str,
    sequence: u64,
    result: ControlResult,
) -> Result<(), BoxError>
where
    W: AsyncWrite + Unpin,
{
    result.validate()?;
    let request_id = result.request_id.clone();
    let envelope = MessageEnvelope::new(
        CONTROL_RESULT_KIND,
        network_id,
        identity.host_id(),
        sequence,
        unix_seconds(),
        Some(request_id),
        result,
    );
    let envelope = identity.signed_envelope(envelope)?;
    send_wire(writer, &WireMessage::ControlResponse { envelope }).await
}

async fn run_local_collector(
    registry: Arc<RwLock<Registry>>,
    host_id: String,
    alias: Arc<RwLock<String>>,
    network_id: String,
    snapshot_tx: watch::Sender<Option<StateSnapshot>>,
    mut control_rx: mpsc::Receiver<http::ControlCommand>,
) {
    let mut sequence: u64 = 0;
    loop {
        let mut collector = match CodexCollector::spawn().await {
            Ok(collector) => collector,
            Err(error) => {
                sequence = sequence.saturating_add(1);
                let current_alias = alias.read().await.clone();
                update_local_snapshot(
                    &registry,
                    unavailable_snapshot(
                        &host_id,
                        &current_alias,
                        &network_id,
                        sequence,
                        &error,
                        None,
                    ),
                    &snapshot_tx,
                )
                .await;
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                    command = control_rx.recv() => {
                        let Some(command) = command else { return; };
                        respond_control_unavailable(command, &error);
                    }
                }
                continue;
            }
        };

        loop {
            sequence = sequence.saturating_add(1);
            let captured_at = unix_seconds();
            let current_alias = alias.read().await.clone();
            match collector
                .snapshot(&host_id, &current_alias, sequence, captured_at)
                .await
            {
                Ok(snapshot) => {
                    let mut snapshot = snapshot;
                    snapshot.network_id = Some(network_id.clone());
                    snapshot.source_node_id = Some(host_id.clone());
                    update_local_snapshot(&registry, snapshot, &snapshot_tx).await;
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                        command = control_rx.recv() => {
                            let Some(command) = command else { return; };
                            execute_control(&mut collector, command).await;
                        }
                    }
                }
                Err(error) => {
                    let previous = collector.last_snapshot();
                    update_local_snapshot(
                        &registry,
                        unavailable_snapshot(
                            &host_id,
                            &current_alias,
                            &network_id,
                            sequence,
                            &error,
                            previous,
                        ),
                        &snapshot_tx,
                    )
                    .await;
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

fn respond_control_unavailable(command: http::ControlCommand, message: &str) {
    respond_control_failure(command, &format!("local collector unavailable: {message}"));
}

fn respond_control_failure(command: http::ControlCommand, message: &str) {
    let request_id = command.request.request_id.clone();
    let _ = command.respond_to.send(ControlResult {
        request_id,
        ok: false,
        message: message.to_string(),
    });
}

async fn execute_control(collector: &mut CodexCollector, command: http::ControlCommand) {
    let request_id = command.request.request_id.clone();
    let result = if command.request.expires_at < unix_seconds() {
        Err("control request expired".to_string())
    } else {
        match command.request.action {
            ControlAction::Interrupt => {
                collector
                    .interrupt(&command.request.thread_id, &command.request.turn_id)
                    .await
            }
        }
    };
    let response = match result {
        Ok(()) => ControlResult {
            request_id,
            ok: true,
            message: "turn interrupt requested".to_string(),
        },
        Err(message) => ControlResult {
            request_id,
            ok: false,
            message,
        },
    };
    let _ = command.respond_to.send(response);
}

async fn update_local_snapshot(
    registry: &Arc<RwLock<Registry>>,
    snapshot: StateSnapshot,
    snapshot_tx: &watch::Sender<Option<StateSnapshot>>,
) {
    let snapshot_for_peers = snapshot.clone();
    if let Err(error) = registry
        .write()
        .await
        .update_snapshot(snapshot, unix_seconds())
    {
        eprintln!("local state snapshot rejected: {error}");
        return;
    }
    let _ = snapshot_tx.send(Some(snapshot_for_peers));
}

fn unavailable_snapshot(
    host_id: &str,
    alias: &str,
    network_id: &str,
    sequence: u64,
    error: &str,
    previous: Option<&StateSnapshot>,
) -> StateSnapshot {
    let mut snapshot = previous.cloned().unwrap_or_else(|| StateSnapshot {
        host_id: host_id.to_string(),
        alias: alias.to_string(),
        os: Some(std::env::consts::OS.to_string()),
        sequence,
        captured_at: unix_seconds(),
        collector_status: CollectorStatus::Unavailable,
        error: None,
        sessions: Vec::new(),
        account: None,
        network_id: None,
        source_node_id: None,
    });
    snapshot.host_id = host_id.to_string();
    snapshot.alias = alias.to_string();
    snapshot.sequence = sequence;
    snapshot.captured_at = unix_seconds();
    snapshot.collector_status = CollectorStatus::Unavailable;
    snapshot.error = Some(error.chars().take(256).collect());
    snapshot.network_id = Some(network_id.to_string());
    snapshot.source_node_id = Some(host_id.to_string());
    snapshot
}

async fn run_peer_discovery(
    identity: Arc<Identity>,
    pairing_identity: Arc<PairingIdentity>,
    node_id: String,
    alias: Arc<RwLock<String>>,
    network_id: String,
    pairings: Arc<Mutex<PairingStore>>,
    registry: Arc<RwLock<Registry>>,
    local_snapshot_rx: watch::Receiver<Option<StateSnapshot>>,
    local_control_tx: mpsc::Sender<http::ControlCommand>,
    peer_router: Arc<PeerRouter>,
    replay_guard: Arc<ControlReplayGuard>,
    http_port: Option<u16>,
    discovery_interface: Ipv4Addr,
    discoveries: Arc<Mutex<DiscoveryCache>>,
    mut pair_rx: mpsc::Receiver<http::PairCommand>,
) {
    let socket = match StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("peer discovery listener failed to bind: {error}");
            return;
        }
    };
    if let Err(error) =
        socket.join_multicast_v4(&Ipv4Addr::from(DISCOVERY_GROUP_V4), &discovery_interface)
    {
        eprintln!("peer discovery multicast join failed: {error}");
        return;
    }
    if let Err(error) = socket.set_nonblocking(true) {
        eprintln!("peer discovery listener setup failed: {error}");
        return;
    }
    let socket = match UdpSocket::from_std(socket) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("peer discovery async socket setup failed: {error}");
            return;
        }
    };
    discoveries.lock().await.set_available();
    let mut buffer = [0u8; 16 * 1024];
    let active_peers = Arc::new(Mutex::new(BTreeSet::new()));
    loop {
        let now = unix_seconds();
        let (announcement, source, manual_code, mut manual_response) = tokio::select! {
            received = socket.recv_from(&mut buffer) => {
                let (length, source) = match received { Ok(value) => value, Err(error) => { eprintln!("peer discovery receive failed: {error}"); continue; } };
                let announcement: DiscoveryAnnouncement = match serde_json::from_slice(&buffer[..length]) { Ok(value) => value, Err(error) => { eprintln!("ignoring malformed peer discovery packet from {source}: {error}"); continue; } };
                (announcement, source, None, None)
            }
            command = pair_rx.recv() => {
                let Some(command) = command else { continue; };
                let Some(peer) = discoveries.lock().await.get(&command.node_id, now) else { let _ = command.respond_to.send(Err("discovery not found".into())); continue; };
                (peer.announcement, peer.address, Some(command.pairing_code), Some(command.respond_to))
            }
        };
        if let Err(reason) = announcement.validate(now) {
            if let Some(response) = manual_response.take() {
                let _ = response.send(Err(reason.into()));
            }
            eprintln!("ignoring peer discovery packet from {source}: {reason}");
            continue;
        }
        if announcement.host_id == node_id {
            if let Some(response) = manual_response.take() {
                let _ = response.send(Err("cannot pair with this node".into()));
            }
            continue;
        }
        if manual_code.is_none() {
            {
                discoveries
                    .lock()
                    .await
                    .remember(announcement.clone(), source, now);
            }
            // An inbound trusted connection can win the race before this
            // peer's first UDP announcement reaches the discovery cache.
            if peer_router.is_connected(&announcement.host_id).await {
                discoveries
                    .lock()
                    .await
                    .mark_connected(&announcement.host_id);
            }
            if pairings
                .lock()
                .await
                .public_key_for(&announcement.host_id)
                .is_none()
            {
                // Unpaired peers remain discoverable; initial pairing is only
                // initiated by the explicit HTTP pair command.
                continue;
            }
        }
        // Use a deterministic initiator so simultaneous multicast announcements
        // do not create two TCP links and race the peer's liveness state.
        if (manual_code.is_none() && node_id.as_str() > announcement.host_id.as_str())
            || !announcement
                .capabilities
                .iter()
                .any(|value| value == PEER_SYNC_CAPABILITY)
        {
            if let Some(response) = manual_response.take() {
                let _ = response.send(Err("peer does not support mesh pairing".into()));
            }
            continue;
        }
        if announcement
            .network_id
            .as_deref()
            .is_some_and(|value| !value.is_empty() && value != network_id)
        {
            if let Some(response) = manual_response.take() {
                let _ = response.send(Err("network id mismatch".into()));
            }
            continue;
        }

        let peer_id = announcement.host_id.clone();
        if peer_router.is_connected(&peer_id).await {
            if let Some(response) = manual_response.take() {
                let _ = response.send(duplicate_pair_result(&peer_id, &peer_router).await);
            }
            continue;
        }
        let should_connect = active_peers.lock().await.insert(peer_id.clone());
        if !should_connect {
            if let Some(response) = manual_response.take() {
                let _ = response.send(duplicate_pair_result(&peer_id, &peer_router).await);
            }
            continue;
        }
        let peer_address = SocketAddr::new(source.ip(), announcement.port);
        let active_peers = Arc::clone(&active_peers);
        let identity = Arc::clone(&identity);
        let pairing_identity = Arc::clone(&pairing_identity);
        let pairings = Arc::clone(&pairings);
        let registry = Arc::clone(&registry);
        let local_snapshot_rx = local_snapshot_rx.clone();
        let local_control_tx = local_control_tx.clone();
        let peer_router = Arc::clone(&peer_router);
        let replay_guard = Arc::clone(&replay_guard);
        let discoveries = Arc::clone(&discoveries);
        let http_port = http_port;
        let node_id = node_id.clone();
        let alias = Arc::clone(&alias);
        let network_id = network_id.clone();
        tokio::spawn(async move {
            eprintln!("discovered peer {} at {peer_address}", announcement.alias);
            let mut paired_response = manual_response;
            let result = connect_peer_and_run(
                peer_address,
                &announcement,
                identity,
                pairing_identity,
                node_id,
                alias.read().await.clone(),
                network_id,
                manual_code,
                Arc::clone(&pairings),
                registry,
                local_snapshot_rx,
                local_control_tx,
                peer_router,
                replay_guard,
                http_port,
                &mut paired_response,
                Arc::clone(&discoveries),
            )
            .await;
            if let Err(error) = result {
                if let Some(response) = paired_response.take() {
                    let _ = response.send(Err(error.to_string()));
                }
                eprintln!("peer {peer_id} session ended: {error}");
            }
            if pairings.lock().await.public_key_for(&peer_id).is_some() {
                discoveries.lock().await.mark_paired(&peer_id);
            }
            active_peers.lock().await.remove(&peer_id);
        });
    }
}

async fn connect_peer_and_run(
    peer_address: SocketAddr,
    announcement: &DiscoveryAnnouncement,
    identity: Arc<Identity>,
    pairing_identity: Arc<PairingIdentity>,
    node_id: String,
    alias: String,
    network_id: String,
    pairing_code: Option<String>,
    pairings: Arc<Mutex<PairingStore>>,
    registry: Arc<RwLock<Registry>>,
    mut local_snapshot_rx: watch::Receiver<Option<StateSnapshot>>,
    local_control_tx: mpsc::Sender<http::ControlCommand>,
    peer_router: Arc<PeerRouter>,
    replay_guard: Arc<ControlReplayGuard>,
    http_port: Option<u16>,
    paired_response: &mut Option<oneshot::Sender<Result<(), String>>>,
    discoveries: Arc<Mutex<DiscoveryCache>>,
) -> Result<(), BoxError> {
    let trusted_key = pairings
        .lock()
        .await
        .public_key_for(&announcement.host_id)
        .map(str::to_string);
    let announced_pairing_key = announcement.pairing_public_key.as_deref();
    let remote_default_key = announced_pairing_key.unwrap_or(&announcement.public_key);
    let expected_remote_key = match trusted_key.as_deref() {
        Some(key) if key == announcement.public_key || announced_pairing_key == Some(key) => {
            key.to_string()
        }
        Some(_) => return Err("paired peer key does not match its discovery announcement".into()),
        None => remote_default_key.to_string(),
    };
    if trusted_key.is_none() && pairing_code.is_none() {
        return Err("manual pairing requires the peer's current pairing code".into());
    }
    let stream = timeout(Duration::from_secs(5), TcpStream::connect(peer_address)).await??;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let capabilities = vec![
        "sessions".to_string(),
        "control".to_string(),
        "usage".to_string(),
        STATE_SNAPSHOT_CAPABILITY.to_string(),
        PEER_SYNC_CAPABILITY.to_string(),
    ];
    let local_public_key = identity.public_key_hex();
    let local_pairing_key = pairing_identity.public_key();
    let code_for_proof = if trusted_key.is_none() {
        pairing_code.as_deref()
    } else {
        None
    };
    send_wire(
        &mut write_half,
        &WireMessage::AgentHello {
            version: PROTOCOL_VERSION,
            network_id: Some(network_id.clone()),
            host_id: node_id.clone(),
            alias: alias.clone(),
            os: Some(std::env::consts::OS.to_string()),
            http_port,
            public_key: local_public_key.clone(),
            pairing_public_key: Some(local_pairing_key.clone()),
            capabilities: capabilities.clone(),
        },
    )
    .await?;

    let server_handshake_key = match timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        read_wire(&mut reader),
    )
    .await??
    .ok_or("peer closed before pairing challenge")?
    {
        WireMessage::PairChallenge {
            challenge,
            monitor_id,
            signature,
            public_key: challenged_public_key,
            pairing_public_key,
        } => {
            let server_public_key = challenged_public_key
                .as_deref()
                .unwrap_or(&announcement.public_key);
            let challenge_key = pairing_public_key
                .as_deref()
                .unwrap_or(&announcement.public_key);
            let transcript = pairing_transcript(
                &node_id,
                &local_public_key,
                &local_pairing_key,
                &monitor_id,
                server_public_key,
                announcement
                    .pairing_public_key
                    .as_deref()
                    .unwrap_or(&announcement.public_key),
                &network_id,
                &challenge,
            );
            if monitor_id != announcement.host_id
                || server_public_key != announcement.public_key
                || challenge_key != expected_remote_key
                || !verify_pairing_signature(challenge_key, &transcript, &signature)
            {
                return Err("peer challenge signature rejected".into());
            }
            let signature = if expected_remote_key.starts_with("p256:") {
                pairing_identity.sign_hex(&transcript)
            } else {
                identity.sign_hex(&transcript)
            };
            let code_proof =
                code_for_proof.map(|code| pairing_code_proof(code, "initiator", &transcript));
            send_wire(
                &mut write_half,
                &WireMessage::PairResponse {
                    host_id: node_id.clone(),
                    signature,
                    code_proof,
                },
            )
            .await?;
            (challenge_key.to_string(), transcript)
        }
        WireMessage::PairRejected { reason } => return Err(reason.into()),
        other => return Err(format!("unexpected peer pairing message: {other:?}").into()),
    };

    match timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        read_wire(&mut reader),
    )
    .await??
    .ok_or("peer closed after pairing challenge")?
    {
        WireMessage::PairAccepted {
            host_id,
            network_id: accepted_network_id,
            signature,
            code_proof,
        } if host_id == node_id => {
            if accepted_network_id
                .as_deref()
                .is_some_and(|value| !value.is_empty() && value != network_id)
            {
                return Err("peer accepted a different network id".into());
            }
            let (server_key, transcript) = server_handshake_key;
            let signature = signature.as_deref().ok_or("unauthenticated PairAccepted")?;
            if !verify_pairing_signature(
                &server_key,
                &pairing_acceptance_message(&transcript),
                signature,
            ) {
                return Err("PairAccepted signature rejected".into());
            }
            if let Some(code) = code_for_proof {
                let expected = pairing_code_proof(code, "acceptor", &transcript);
                if !code_proof.as_deref().is_some_and(|proof| {
                    codex_monitor_protocol::constant_time_hex_eq(&expected, proof)
                }) {
                    return Err("peer did not prove possession of pairing code".into());
                }
            } else if code_proof.is_some() {
                return Err("unexpected PairAccepted code proof".into());
            }
            pairings
                .lock()
                .await
                .remember(&announcement.host_id, &server_key)?;
        }
        WireMessage::PairRejected { reason } => return Err(reason.into()),
        other => return Err(format!("unexpected peer pairing result: {other:?}").into()),
    }

    registry.write().await.register(
        announcement.host_id.clone(),
        announcement.alias.clone(),
        announcement.capabilities.clone(),
        unix_seconds(),
    );
    registry
        .write()
        .await
        .set_os(&announcement.host_id, announcement.os.clone());
    discoveries
        .lock()
        .await
        .mark_connected(&announcement.host_id);
    if let Some(http_port) = announcement.http_port {
        registry.write().await.set_http_endpoints(
            &announcement.host_id,
            http_endpoint_candidates(peer_address.ip(), http_port, &announcement.alias),
        );
    }
    let state_supported = announcement
        .capabilities
        .iter()
        .any(|value| value == STATE_SNAPSHOT_CAPABILITY);
    let peer_sync_supported = announcement
        .capabilities
        .iter()
        .any(|value| value == PEER_SYNC_CAPABILITY);
    let p256_peer_sync_supported = announcement
        .capabilities
        .iter()
        .any(|value| value == P256_PEER_SYNC_CAPABILITY);
    let control_supported = announcement
        .capabilities
        .iter()
        .any(|value| value == "control")
        && capabilities.iter().any(|value| value == "control");
    let snapshot = local_snapshot_rx.borrow().clone();
    if let Some(snapshot) = snapshot {
        send_snapshot(
            &mut write_half,
            &identity,
            &pairing_identity,
            &network_id,
            snapshot,
            peer_sync_supported,
            p256_peer_sync_supported,
        )
        .await?;
    }

    let peer_id = announcement.host_id.clone();
    let (peer_control_tx, mut peer_control_rx) = mpsc::channel(8);
    let peer_route_generation = Some(if control_supported {
        peer_router.register(peer_id.clone(), peer_control_tx).await
    } else {
        peer_router.register_connection(peer_id.clone(), None).await
    });
    if let Some(response) = paired_response.take() {
        let _ = response.send(Ok(()));
    }
    let mut pending_controls = BTreeMap::new();
    let mut control_sequence = 0u64;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    let result: Result<(), BoxError> = loop {
        let read = timeout(
            Duration::from_secs(HEARTBEAT_TIMEOUT_SECS),
            read_wire(&mut reader),
        );
        tokio::select! {
            biased;
            changed = local_snapshot_rx.changed(), if state_supported => {
                if changed.is_err() {
                    break Err("local snapshot channel closed".into());
                }
                let snapshot = local_snapshot_rx.borrow().clone();
                if let Some(snapshot) = snapshot {
                    send_snapshot(
                        &mut write_half,
                        &identity,
                        &pairing_identity,
                        &network_id,
                        snapshot,
                        peer_sync_supported,
                        p256_peer_sync_supported,
                    )
                    .await?;
                }
            }
            command = peer_control_rx.recv(), if control_supported => {
                let Some(command) = command else {
                    break Err("peer control channel closed".into());
                };
                if let Err(reason) = command.request.validate(unix_seconds()) {
                    respond_control_failure(command, reason);
                    continue;
                }
                let request_id = command.request.request_id.clone();
                if pending_controls.contains_key(&request_id) {
                    respond_control_failure(command, "duplicate control request id");
                    continue;
                }
                let respond_to = command.respond_to;
                let request = command.request;
                control_sequence = control_sequence.saturating_add(1);
                let envelope = MessageEnvelope::new(
                    CONTROL_REQUEST_KIND,
                    &network_id,
                    identity.host_id(),
                    control_sequence,
                    unix_seconds(),
                    Some(request_id.clone()),
                    request,
                );
                let envelope = match identity.signed_envelope(envelope) {
                    Ok(envelope) => envelope,
                    Err(error) => {
                        let _ = respond_to.send(ControlResult {
                            request_id,
                            ok: false,
                            message: format!("sign control request: {error}"),
                        });
                        continue;
                    }
                };
                pending_controls.insert(request_id.clone(), respond_to);
                if let Err(error) = send_wire(&mut write_half, &WireMessage::ControlRequest { envelope }).await {
                    if let Some(respond_to) = pending_controls.remove(&request_id) {
                        let _ = respond_to.send(ControlResult {
                            request_id,
                            ok: false,
                            message: format!("send control request: {error}"),
                        });
                    }
                    break Err(error);
                }
            }
            result = read => {
                let message = match result {
                    Ok(Ok(message)) => message,
                    Ok(Err(error)) => break Err(error),
                    Err(error) => break Err(error.into()),
                };
                match message {
                    Some(WireMessage::Heartbeat { host_id: heartbeat_host_id }) if heartbeat_host_id == peer_id => {
                        registry.write().await.touch(&peer_id, unix_seconds());
                        send_wire(&mut write_half, &WireMessage::HeartbeatAck).await?;
                    }
                    Some(WireMessage::Heartbeat { .. }) => break Err("peer heartbeat host id mismatch".into()),
                    Some(WireMessage::StateSnapshot { snapshot }) if state_supported => {
                        if snapshot.host_id != peer_id
                            || snapshot.source_id() != peer_id
                            || snapshot
                                .network_id
                                .as_deref()
                                .is_some_and(|value| !value.is_empty() && value != network_id)
                        {
                            break Err("peer state snapshot host id mismatch".into());
                        }
                        if let Err(reason) = registry.write().await.update_snapshot(snapshot, unix_seconds()) {
                            break Err(reason.into());
                        }
                    }
                    Some(WireMessage::StateSnapshot { .. }) => {
                        eprintln!("peer {peer_id} sent state without negotiated capability");
                    }
                    Some(WireMessage::PeerSnapshot { envelope }) if peer_sync_supported => {
                        let snapshot = match accept_peer_snapshot(
                            envelope,
                            &announcement.public_key,
                            &peer_id,
                            &network_id,
                        ) {
                            Ok(snapshot) => snapshot,
                            Err(reason) => break Err(reason.into()),
                        };
                        if let Err(reason) = registry
                            .write()
                            .await
                            .update_snapshot(snapshot, unix_seconds())
                        {
                            break Err(reason.into());
                        }
                    }
                    Some(WireMessage::PeerSnapshot { .. }) => {
                        eprintln!("peer {peer_id} sent peer state without negotiated capability");
                    }
                    Some(WireMessage::ControlRequest { envelope }) if control_supported => {
                        handle_incoming_control(
                            &mut write_half,
                            envelope,
                            &announcement.public_key,
                            &peer_id,
                            &node_id,
                            &network_id,
                            &identity,
                            &local_control_tx,
                            &replay_guard,
                        )
                        .await?;
                    }
                    Some(WireMessage::ControlRequest { .. }) => {
                        eprintln!("peer {peer_id} sent control without negotiated capability");
                    }
                    Some(WireMessage::ControlResponse { envelope }) if control_supported => {
                        let response = match accept_control_response(
                            envelope,
                            &announcement.public_key,
                            &peer_id,
                            &network_id,
                        ) {
                            Ok(response) => response,
                            Err(reason) => break Err(reason.into()),
                        };
                        if let Some(respond_to) = pending_controls.remove(&response.request_id) {
                            let _ = respond_to.send(response);
                        } else {
                            eprintln!("peer {peer_id} sent an unknown control response");
                        }
                    }
                    Some(WireMessage::ControlResponse { .. }) => {
                        eprintln!("peer {peer_id} sent control without negotiated capability");
                    }
                    Some(WireMessage::HeartbeatAck) => {}
                    Some(WireMessage::PairRejected { reason }) => break Err(reason.into()),
                    Some(other) => eprintln!("peer {peer_id} sent unsupported message: {other:?}"),
                    None => break Ok(()),
                }
            }
            _ = heartbeat.tick() => {
                send_wire(&mut write_half, &WireMessage::Heartbeat { host_id: node_id.clone() }).await?;
            }
        }
    };
    let should_disconnect = match peer_route_generation {
        Some(generation) => peer_router.unregister(&peer_id, generation).await,
        None => true,
    };
    for (request_id, respond_to) in pending_controls {
        let _ = respond_to.send(ControlResult {
            request_id,
            ok: false,
            message: "peer connection closed before control completed".to_string(),
        });
    }
    if should_disconnect {
        registry.write().await.disconnect(&peer_id, unix_seconds());
    }
    result
}

fn accept_peer_snapshot(
    envelope: MessageEnvelope<StateSnapshot>,
    public_key: &str,
    expected_sender: &str,
    network_id: &str,
) -> Result<StateSnapshot, &'static str> {
    envelope.validate()?;
    if envelope.kind != STATE_SNAPSHOT_KIND
        || Identity::host_id_from_public_key(public_key).as_deref() != Some(expected_sender)
        || envelope.sender_id != expected_sender
        || envelope.network_id != network_id
        || envelope.sequence == 0
        || !Identity::verify_envelope(public_key, &envelope)
    {
        return Err("peer snapshot signature or identity rejected");
    }
    let snapshot = envelope.payload;
    if snapshot.host_id != expected_sender
        || snapshot.source_id() != expected_sender
        || snapshot.sequence != envelope.sequence
        || snapshot.captured_at != envelope.sent_at
        || snapshot
            .network_id
            .as_deref()
            .is_some_and(|value| !value.is_empty() && value != network_id)
    {
        return Err("peer snapshot source mismatch");
    }
    snapshot.validate()?;
    Ok(snapshot)
}

fn accept_control_response(
    envelope: MessageEnvelope<ControlResult>,
    public_key: &str,
    expected_sender: &str,
    network_id: &str,
) -> Result<ControlResult, &'static str> {
    envelope.validate()?;
    if envelope.kind != CONTROL_RESULT_KIND
        || envelope.sender_id != expected_sender
        || envelope.network_id != network_id
        || envelope.sequence == 0
        || !Identity::verify_envelope(public_key, &envelope)
    {
        return Err("control response signature or identity rejected");
    }
    let result = envelope.payload;
    result.validate()?;
    if envelope.request_id.as_deref() != Some(result.request_id.as_str()) {
        return Err("control response request id mismatch");
    }
    Ok(result)
}

fn http_endpoint_candidates(ip: IpAddr, port: u16, alias: &str) -> Vec<String> {
    let address = match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    let mut endpoints = vec![format!("http://{address}:{port}")];
    if valid_hostname(alias) {
        endpoints.push(format!("http://{alias}:{port}"));
        if !alias.ends_with(".local") {
            endpoints.push(format!("http://{alias}.local:{port}"));
        }
    }
    endpoints
}

fn discovery_target_specs() -> Vec<String> {
    env::var("CODEX_MONITOR_DISCOVERY_TARGETS")
        .ok()
        .map_or_else(Vec::new, |value| parse_discovery_target_specs(&value))
}

fn parse_discovery_target_specs(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .map(str::to_string)
        .collect()
}

fn with_default_discovery_port(spec: &str) -> String {
    if spec.parse::<SocketAddr>().is_ok() {
        return spec.to_string();
    }
    if let Ok(IpAddr::V6(_)) = spec.parse::<IpAddr>() {
        return format!("[{spec}]:{DISCOVERY_PORT}");
    }
    if spec.starts_with('[') && spec.ends_with(']') {
        return format!("{spec}:{DISCOVERY_PORT}");
    }
    format!("{spec}:{DISCOVERY_PORT}")
}

fn resolve_discovery_target(spec: &str) -> Result<Vec<SocketAddr>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("target is empty".to_string());
    }

    let defaulted = with_default_discovery_port(spec);
    let candidates = if defaulted == spec {
        vec![spec.to_string()]
    } else {
        // Trying the original first lets standard-library resolution handle
        // hostname:port without a custom parser.
        vec![spec.to_string(), defaulted]
    };
    let mut last_error = None;
    for candidate in candidates {
        match candidate.to_socket_addrs() {
            Ok(addresses) => {
                let mut addresses: Vec<_> = addresses.collect();
                addresses.sort();
                addresses.dedup();
                if !addresses.is_empty() {
                    return Ok(addresses);
                }
                last_error = Some(format!("no address resolved for {candidate}"));
            }
            Err(error) => {
                last_error = Some(format!("{candidate}: {error}"));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "target did not resolve".to_string()))
}

async fn send_discovery_targets(
    socket_v4: &UdpSocket,
    socket_v6: Option<&UdpSocket>,
    announcement: &[u8],
    specs: &[String],
) {
    for spec in specs {
        let addresses = match resolve_discovery_target(spec) {
            Ok(addresses) => addresses,
            Err(error) => {
                eprintln!("warning: discovery target '{spec}' resolution failed: {error}");
                continue;
            }
        };
        for address in addresses {
            let socket = match address {
                SocketAddr::V4(_) => socket_v4,
                SocketAddr::V6(_) => match socket_v6 {
                    Some(socket) => socket,
                    None => {
                        eprintln!(
                            "warning: discovery target '{spec}' resolved to {address}, but the IPv6 socket is unavailable"
                        );
                        continue;
                    }
                },
            };
            if let Err(error) = send_discovery_announcement(socket, announcement, address).await {
                eprintln!(
                    "warning: discovery target send to {address} (from '{spec}') failed: {error}"
                );
            }
        }
    }
}

async fn send_discovery_announcement(
    socket: &UdpSocket,
    announcement: &[u8],
    address: SocketAddr,
) -> std::io::Result<()> {
    socket.send_to(announcement, address).await.map(|_| ())
}

fn valid_hostname(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 {
        return false;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
    })
}

async fn send_snapshot<W>(
    writer: &mut W,
    identity: &Identity,
    pairing_identity: &PairingIdentity,
    network_id: &str,
    snapshot: StateSnapshot,
    peer_sync: bool,
    p256_peer_sync: bool,
) -> Result<(), BoxError>
where
    W: AsyncWrite + Unpin,
{
    if peer_sync || p256_peer_sync {
        if p256_peer_sync {
            let payload_json = serde_json::to_string(&snapshot)?;
            let mut signing_bytes = format!(
                "codex-monitor/android-snapshot-v1:{network_id}:{}:",
                identity.host_id()
            )
            .into_bytes();
            signing_bytes.extend_from_slice(payload_json.as_bytes());
            return send_wire(
                writer,
                &WireMessage::AndroidSnapshot {
                    network_id: network_id.to_string(),
                    sender_id: identity.host_id(),
                    payload_json,
                    signature: pairing_identity.sign_hex(&signing_bytes),
                },
            )
            .await;
        }
        let mut envelope = MessageEnvelope::new(
            STATE_SNAPSHOT_KIND,
            network_id,
            identity.host_id(),
            snapshot.sequence,
            snapshot.captured_at,
            None,
            snapshot,
        );
        identity.sign_envelope(&mut envelope)?;
        send_wire(writer, &WireMessage::PeerSnapshot { envelope }).await
    } else {
        send_wire(writer, &WireMessage::StateSnapshot { snapshot }).await
    }
}

async fn send_wire<W>(writer: &mut W, message: &WireMessage) -> Result<(), BoxError>
where
    W: AsyncWrite + Unpin,
{
    let mut encoded = serde_json::to_vec(message)?;
    encoded.push(b'\n');
    writer.write_all(&encoded).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_wire<R>(reader: &mut R) -> Result<Option<WireMessage>, BoxError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let read = reader
        .take((MAX_WIRE_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)
        .await?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_WIRE_FRAME_BYTES {
        return Err("wire frame exceeds maximum size".into());
    }
    Ok(Some(serde_json::from_slice(&line)?))
}

async fn duplicate_pair_result(peer_id: &str, peer_router: &PeerRouter) -> Result<(), String> {
    if peer_router.is_connected(peer_id).await {
        Ok(())
    } else {
        Err("peer connection is already in progress".to_string())
    }
}

fn default_state_cache_path() -> PathBuf {
    if let Ok(path) = env::var("CODEX_MONITOR_STATE_PATH") {
        return PathBuf::from(path);
    }
    default_identity_path("monitor").with_file_name("state-cache.json")
}

fn local_hostname() -> String {
    env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| "codex-monitor-node".to_string())
}

fn load_alias(
    alias_path: &std::path::Path,
    configured_alias: Option<&str>,
    identity_existed: bool,
    node_id: &str,
) -> String {
    std::fs::read_to_string(alias_path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            configured_alias
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            if identity_existed {
                local_hostname()
            } else {
                cute_default_alias(node_id)
            }
        })
}

fn cute_default_alias(node_id: &str) -> String {
    const ADJECTIVES: [&str; 8] = [
        "Brisk", "Cozy", "Gentle", "Jolly", "Lucky", "Merry", "Quiet", "Sunny",
    ];
    const ANIMALS: [&str; 8] = [
        "Badger", "Fox", "Koala", "Otter", "Panda", "Rabbit", "Robin", "Seal",
    ];
    let suffix = node_id.rsplit(':').next().unwrap_or(node_id);
    let bytes = suffix.as_bytes();
    format!(
        "{} {} {}",
        ADJECTIVES[bytes.first().copied().unwrap_or(0) as usize % 8],
        ANIMALS[bytes.get(1).copied().unwrap_or(0) as usize % 8],
        &suffix[..suffix.len().min(4)]
    )
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut value);
    hex::encode(value)
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{
        duplicate_pair_result, http_endpoint_candidates, load_alias, parse_discovery_target_specs,
        send_discovery_announcement, unavailable_snapshot, with_default_discovery_port,
        CollectorStatus, ControlAction, ControlReplayGuard, DiscoveryAnnouncement, PeerRouter,
        StateSnapshot, UdpSocket, ANNOUNCEMENT_TTL_SECS, DISCOVERY_PORT, MONITOR_PORT,
        PEER_SYNC_CAPABILITY, PROTOCOL_VERSION, SERVICE_NAME,
    };
    use codex_monitor_protocol::{ControlRequest, SessionState};
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::sync::{mpsc, oneshot};

    #[tokio::test]
    async fn duplicate_pair_reports_pending_until_router_confirms_connection() {
        let router = PeerRouter::default();
        assert_eq!(
            duplicate_pair_result("node-a", &router).await,
            Err("peer connection is already in progress".to_string())
        );

        let generation = router.register_connection("node-a".to_string(), None).await;
        assert_eq!(duplicate_pair_result("node-a", &router).await, Ok(()));
        assert!(router.unregister("node-a", generation).await);
        assert_eq!(
            duplicate_pair_result("node-a", &router).await,
            Err("peer connection is already in progress".to_string())
        );

        let (sender, _receiver) = mpsc::channel(1);
        let generation = router.register("node-a".to_string(), sender).await;
        assert_eq!(duplicate_pair_result("node-a", &router).await, Ok(()));
        router.unregister("node-a", generation).await;
    }

    fn control_command(request_id: &str) -> super::http::ControlCommand {
        let (respond_to, _response) = oneshot::channel();
        super::http::ControlCommand {
            request: ControlRequest {
                action: ControlAction::Interrupt,
                target_node_id: Some("node-a".to_string()),
                thread_id: "thread-a".to_string(),
                turn_id: "turn-a".to_string(),
                request_id: request_id.to_string(),
                expires_at: u64::MAX,
            },
            respond_to,
        }
    }

    #[test]
    fn persisted_alias_overrides_env_after_restart() {
        let root = std::env::temp_dir().join(format!(
            "codex-monitor-alias-load-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("monitor.alias");

        assert_eq!(
            load_alias(&path, Some("Configured Name"), true, "node-a"),
            "Configured Name"
        );
        std::fs::write(&path, "UI Name\n").unwrap();
        assert_eq!(
            load_alias(&path, Some("Configured Name"), true, "node-a"),
            "UI Name"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn peer_router_tracks_connected_routes() {
        let router = PeerRouter::default();
        let (sender, _receiver) = mpsc::channel(1);
        let generation = router.register("node-a".to_string(), sender).await;
        assert!(router.sender_for("node-a").await.is_some());
        router.unregister("node-a", generation).await;
        assert!(router.sender_for("node-a").await.is_none());
    }

    #[test]
    fn endpoint_candidates_include_stable_hostname_fallbacks() {
        assert_eq!(
            http_endpoint_candidates(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 8788, "desktop-a"),
            vec![
                "http://10.0.0.2:8788",
                "http://desktop-a:8788",
                "http://desktop-a.local:8788"
            ]
        );
        assert_eq!(
            http_endpoint_candidates(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 8788, "开发机 A"),
            vec!["http://10.0.0.2:8788"]
        );
    }

    #[test]
    fn discovery_target_defaults_use_discovery_port_and_preserve_ipv6_syntax() {
        assert_eq!(
            with_default_discovery_port("desktop-a"),
            format!("desktop-a:{DISCOVERY_PORT}")
        );
        assert_eq!(
            with_default_discovery_port("10.0.0.2"),
            format!("10.0.0.2:{DISCOVERY_PORT}")
        );
        assert_eq!(
            with_default_discovery_port("2001:db8::2"),
            format!("[2001:db8::2]:{DISCOVERY_PORT}")
        );
        assert_eq!(
            with_default_discovery_port("[2001:db8::2]"),
            format!("[2001:db8::2]:{DISCOVERY_PORT}")
        );
        assert_eq!(
            with_default_discovery_port("[2001:db8::2]:9876"),
            "[2001:db8::2]:9876"
        );
    }

    #[test]
    fn discovery_target_list_trims_and_ignores_empty_entries() {
        assert_eq!(
            parse_discovery_target_specs(" desktop-a, ,10.0.0.2:37991,[2001:db8::2] "),
            vec!["desktop-a", "10.0.0.2:37991", "[2001:db8::2]"]
        );
    }

    #[tokio::test]
    async fn discovery_announcement_sender_delivers_valid_payload() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind discovery receiver");
        let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind discovery sender");
        let issued_at = super::unix_seconds();
        let announcement = DiscoveryAnnouncement {
            service: SERVICE_NAME.to_string(),
            version: PROTOCOL_VERSION,
            host_id: "ed25519:1111111111111111".to_string(),
            alias: "test-node".to_string(),
            os: None,
            port: MONITOR_PORT,
            http_port: Some(8788),
            public_key: "11".repeat(32),
            pairing_public_key: None,
            nonce: "test-nonce".to_string(),
            issued_at,
            expires_at: issued_at + ANNOUNCEMENT_TTL_SECS,
            capabilities: vec![PEER_SYNC_CAPABILITY.to_string()],
            network_id: Some("test-network".to_string()),
        };
        let payload = serde_json::to_vec(&announcement).expect("serialize announcement");
        send_discovery_announcement(&sender, &payload, receiver.local_addr().unwrap())
            .await
            .expect("send discovery announcement");

        let mut buffer = [0u8; 4096];
        let (length, source) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            receiver.recv_from(&mut buffer),
        )
        .await
        .expect("receive discovery announcement")
        .expect("receive discovery datagram");
        assert_eq!(source.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));

        let decoded: DiscoveryAnnouncement =
            serde_json::from_slice(&buffer[..length]).expect("decode discovery announcement");
        assert_eq!(decoded.host_id, announcement.host_id);
        assert_eq!(decoded.network_id, announcement.network_id);
        decoded
            .validate(super::unix_seconds())
            .expect("validate discovery announcement");
    }

    #[test]
    fn unavailable_snapshot_keeps_last_payload() {
        let previous = StateSnapshot {
            host_id: "node-a".to_string(),
            alias: "A".to_string(),
            os: None,
            sequence: 1,
            captured_at: 10,
            collector_status: CollectorStatus::Ready,
            error: None,
            sessions: vec![SessionState {
                thread_id: "thread-a".to_string(),
                turn_id: Some("turn-a".to_string()),
                title: Some("running".to_string()),
                status: "active".to_string(),
                last_output_summary: None,
                created_at: None,
                updated_at: None,
            }],
            account: None,
            network_id: Some("network-a".to_string()),
            source_node_id: Some("node-a".to_string()),
        };
        let snapshot = unavailable_snapshot(
            "node-a",
            "A",
            "network-a",
            2,
            "app-server exited",
            Some(&previous),
        );
        assert_eq!(snapshot.collector_status, CollectorStatus::Unavailable);
        assert_eq!(snapshot.sequence, 2);
        assert_eq!(snapshot.sessions, previous.sessions);
        assert_eq!(snapshot.error.as_deref(), Some("app-server exited"));
    }

    #[tokio::test]
    async fn stale_connection_cannot_remove_new_route() {
        let router = PeerRouter::default();
        let (old_sender, _old_receiver) = mpsc::channel(1);
        let old_generation = router.register("node-a".to_string(), old_sender).await;
        let (new_sender, _new_receiver) = mpsc::channel(1);
        let new_generation = router.register("node-a".to_string(), new_sender).await;

        assert!(!router.unregister("node-a", new_generation).await);
        assert!(router.sender_for("node-a").await.is_some());
        assert!(router.unregister("node-a", old_generation).await);
        assert!(router.sender_for("node-a").await.is_none());
    }

    #[tokio::test]
    async fn closed_latest_route_falls_back_to_previous() {
        let router = PeerRouter::default();
        let (old_sender, mut old_receiver) = mpsc::channel(1);
        router.register("node-a".to_string(), old_sender).await;
        let (new_sender, new_receiver) = mpsc::channel(1);
        router.register("node-a".to_string(), new_sender).await;
        drop(new_receiver);

        assert!(router
            .send("node-a", control_command("request-1"))
            .await
            .is_ok());
        assert_eq!(
            old_receiver
                .recv()
                .await
                .expect("fallback command")
                .request
                .request_id,
            "request-1"
        );
    }

    #[tokio::test]
    async fn replay_guard_accepts_once_until_expiry() {
        let guard = ControlReplayGuard::default();
        assert!(guard.claim("node-a", "req-1", 110, 100).await);
        assert!(!guard.claim("node-a", "req-1", 110, 100).await);
        assert!(guard.claim("node-a", "req-1", 110, 111).await);
    }

    #[tokio::test]
    async fn outbound_pairing_completes_signed_challenge_and_persists_key() {
        use std::sync::Arc;
        use tokio::io::BufReader;
        use tokio::net::TcpListener;
        use tokio::sync::{watch, Mutex, RwLock};

        let root =
            std::env::temp_dir().join(format!("codex-monitor-pair-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let client_identity =
            Arc::new(super::Identity::load_or_create(&root.join("client.identity")).unwrap());
        let server_identity =
            Arc::new(super::Identity::load_or_create(&root.join("server.identity")).unwrap());
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = Arc::clone(&server_identity);
        let client_public_key = client_identity.public_key_hex();
        let client_pairing_identity =
            super::PairingIdentity::load_or_create(&root.join("client.pairing.identity")).unwrap();
        let server_public_key = server_identity.public_key_hex();
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);
            let hello = super::read_wire(&mut reader).await.unwrap().unwrap();
            let super::WireMessage::AgentHello {
                host_id,
                public_key,
                pairing_public_key,
                ..
            } = hello
            else {
                panic!("hello")
            };
            let challenge = "fixture-challenge";
            let monitor_id = server.host_id();
            let transcript = super::pairing_transcript(
                &host_id,
                &public_key,
                pairing_public_key.as_deref().unwrap_or(&public_key),
                &monitor_id,
                &server_public_key,
                &server_public_key,
                "test-network",
                challenge,
            );
            let signature = server.sign_hex(&transcript);
            super::send_wire(
                &mut write,
                &super::WireMessage::PairChallenge {
                    challenge: challenge.into(),
                    monitor_id: monitor_id.clone(),
                    signature,
                    public_key: Some(server_public_key.clone()),
                    pairing_public_key: None,
                },
            )
            .await
            .unwrap();
            let response = super::read_wire(&mut reader).await.unwrap().unwrap();
            let super::WireMessage::PairResponse {
                host_id: response_id,
                signature,
                code_proof,
            } = response
            else {
                panic!("response")
            };
            assert_eq!(response_id, host_id);
            assert!(super::Identity::verify_hex(
                &client_public_key,
                &transcript,
                &signature
            ));
            assert!(code_proof.as_deref().is_some_and(|proof| {
                codex_monitor_protocol::constant_time_hex_eq(
                    proof,
                    &codex_monitor_protocol::pairing_code_proof(
                        "Ab3dE7qK",
                        "initiator",
                        &transcript,
                    ),
                )
            }));
            super::send_wire(
                &mut write,
                &super::WireMessage::PairAccepted {
                    host_id,
                    network_id: Some("test-network".into()),
                    signature: Some(server.sign_hex(
                        &codex_monitor_protocol::pairing_acceptance_message(&transcript),
                    )),
                    code_proof: Some(codex_monitor_protocol::pairing_code_proof(
                        "Ab3dE7qK",
                        "acceptor",
                        &transcript,
                    )),
                },
            )
            .await
            .unwrap();
        });
        let announcement = DiscoveryAnnouncement {
            service: SERVICE_NAME.into(),
            version: PROTOCOL_VERSION,
            host_id: server_identity.host_id(),
            alias: "server".into(),
            os: Some("linux".into()),
            port: address.port(),
            http_port: None,
            public_key: server_identity.public_key_hex(),
            pairing_public_key: None,
            nonce: "n".into(),
            issued_at: super::unix_seconds(),
            expires_at: super::unix_seconds() + ANNOUNCEMENT_TTL_SECS,
            capabilities: vec![PEER_SYNC_CAPABILITY.into()],
            network_id: Some("test-network".into()),
        };
        let pairings = Arc::new(Mutex::new(
            super::PairingStore::load(root.join("pairings.json")).unwrap(),
        ));
        let registry = Arc::new(RwLock::new(
            super::Registry::load(root.join("state.json"), Some("test-network")).unwrap(),
        ));
        let (_snapshot_tx, snapshot_rx) = watch::channel(None);
        let (control_tx, _control_rx) = mpsc::channel(1);
        let (paired_tx, paired_rx) = oneshot::channel();
        let mut paired = Some(paired_tx);
        let _result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            super::connect_peer_and_run(
                address,
                &announcement,
                Arc::clone(&client_identity),
                Arc::new(client_pairing_identity),
                client_identity.host_id(),
                "client".into(),
                "test-network".into(),
                Some("Ab3dE7qK".into()),
                Arc::clone(&pairings),
                registry,
                snapshot_rx,
                control_tx,
                Arc::new(PeerRouter::default()),
                Arc::new(ControlReplayGuard::default()),
                None,
                &mut paired,
                Arc::new(Mutex::new(super::DiscoveryCache::default())),
            ),
        )
        .await;
        assert_eq!(paired_rx.await.unwrap(), Ok(()));
        assert_eq!(
            pairings
                .lock()
                .await
                .public_key_for(&server_identity.host_id()),
            Some(server_identity.public_key_hex().as_str())
        );
        server_task.await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn inbound_p256_pairing_consumes_code_and_persists_handshake_key() {
        use std::sync::Arc;
        use tokio::io::BufReader;
        use tokio::net::{TcpListener, TcpStream};
        use tokio::sync::{watch, Mutex, RwLock};

        let root = std::env::temp_dir().join(format!(
            "codex-monitor-inbound-p256-pair-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let monitor_identity =
            Arc::new(super::Identity::load_or_create(&root.join("monitor.identity")).unwrap());
        let monitor_pairing_identity = Arc::new(
            super::PairingIdentity::load_or_create(&root.join("monitor.pairing.identity")).unwrap(),
        );
        let client_identity =
            super::Identity::load_or_create(&root.join("client.identity")).unwrap();
        let client_pairing_identity =
            super::PairingIdentity::load_or_create(&root.join("client.pairing.identity")).unwrap();
        let pairings = Arc::new(Mutex::new(
            super::PairingStore::load(root.join("pairings.json")).unwrap(),
        ));
        let pairing_codes = super::PairingCodeManager::new();
        let code = pairing_codes.rotate().code;
        let registry = Arc::new(RwLock::new(
            super::Registry::load(root.join("state.json"), Some("test-network")).unwrap(),
        ));
        let (_snapshot_tx, snapshot_rx) = watch::channel::<Option<StateSnapshot>>(None);
        let (control_tx, _control_rx) = mpsc::channel(1);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let now = super::unix_seconds();
        let discoveries = Arc::new(Mutex::new(super::DiscoveryCache::default()));
        discoveries.lock().await.remember(
            DiscoveryAnnouncement {
                service: SERVICE_NAME.into(),
                version: PROTOCOL_VERSION,
                host_id: client_identity.host_id(),
                alias: "android peer".into(),
                os: Some("android".into()),
                port: address.port(),
                http_port: None,
                public_key: client_identity.public_key_hex(),
                pairing_public_key: Some(client_pairing_identity.public_key()),
                nonce: "test-nonce".into(),
                issued_at: now,
                expires_at: now + ANNOUNCEMENT_TTL_SECS,
                capabilities: vec![PEER_SYNC_CAPABILITY.into()],
                network_id: Some("test-network".into()),
            },
            address,
            now,
        );
        let server_pairings = Arc::clone(&pairings);
        let server_pairing_codes = pairing_codes.clone();
        let server_registry = Arc::clone(&registry);
        let server_identity = Arc::clone(&monitor_identity);
        let server_pairing_identity = Arc::clone(&monitor_pairing_identity);
        let server_discoveries = Arc::clone(&discoveries);
        let server_task = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            super::handle_agent(
                stream,
                peer,
                &server_identity.host_id(),
                server_identity,
                server_pairing_identity,
                server_pairings,
                server_pairing_codes,
                server_registry,
                "test-network".into(),
                snapshot_rx,
                control_tx,
                Arc::new(super::PeerRouter::default()),
                Arc::new(super::ControlReplayGuard::default()),
                server_discoveries,
            )
            .await
        });

        let stream = TcpStream::connect(address).await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        super::send_wire(
            &mut write,
            &super::WireMessage::AgentHello {
                version: PROTOCOL_VERSION,
                network_id: Some("test-network".into()),
                host_id: client_identity.host_id(),
                alias: "android peer".into(),
                os: Some("android".into()),
                http_port: None,
                public_key: client_identity.public_key_hex(),
                pairing_public_key: Some(client_pairing_identity.public_key()),
                capabilities: vec![],
            },
        )
        .await
        .unwrap();
        let challenge = super::read_wire(&mut reader).await.unwrap().unwrap();
        let super::WireMessage::PairChallenge {
            challenge,
            monitor_id,
            signature,
            public_key: Some(monitor_primary_key),
            pairing_public_key: Some(monitor_public_key),
        } = challenge
        else {
            panic!("expected P-256 pair challenge")
        };
        assert_eq!(monitor_id, monitor_identity.host_id());
        let transcript = super::pairing_transcript(
            &client_identity.host_id(),
            &client_identity.public_key_hex(),
            &client_pairing_identity.public_key(),
            &monitor_id,
            &monitor_primary_key,
            &monitor_public_key,
            "test-network",
            &challenge,
        );
        assert!(codex_monitor_protocol::verify_pairing_signature(
            &monitor_public_key,
            &transcript,
            &signature,
        ));
        assert!(!codex_monitor_protocol::verify_pairing_signature(
            &monitor_public_key,
            &codex_monitor_protocol::pairing_acceptance_message(&transcript),
            &signature,
        ));
        super::send_wire(
            &mut write,
            &super::WireMessage::PairResponse {
                host_id: client_identity.host_id(),
                signature: client_pairing_identity.sign_hex(&transcript),
                code_proof: Some(codex_monitor_protocol::pairing_code_proof(
                    &code,
                    "initiator",
                    &transcript,
                )),
            },
        )
        .await
        .unwrap();
        let accepted = super::read_wire(&mut reader).await.unwrap().unwrap();
        let super::WireMessage::PairAccepted {
            signature: Some(accepted_signature),
            code_proof: Some(accepted_proof),
            ..
        } = accepted
        else {
            panic!("expected authenticated acceptance")
        };
        assert!(codex_monitor_protocol::verify_pairing_signature(
            &monitor_public_key,
            &codex_monitor_protocol::pairing_acceptance_message(&transcript),
            &accepted_signature
        ));
        assert!(codex_monitor_protocol::constant_time_hex_eq(
            &accepted_proof,
            &codex_monitor_protocol::pairing_code_proof(&code, "acceptor", &transcript)
        ));
        assert_eq!(
            discoveries.lock().await.views(now, "test-network")[0].pairing_state,
            "connected"
        );
        drop(write);
        let _ = server_task.await.unwrap();
        assert_eq!(
            discoveries.lock().await.views(now, "test-network")[0].pairing_state,
            "paired"
        );

        assert_eq!(
            pairings
                .lock()
                .await
                .public_key_for(&client_identity.host_id()),
            Some(client_pairing_identity.public_key().as_str())
        );
        assert!(pairing_codes.reserve(&code).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
