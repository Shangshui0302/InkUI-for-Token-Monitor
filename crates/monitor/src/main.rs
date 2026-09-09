mod http;
mod pairing;
mod registry;

use codex_monitor_codex_client::CodexCollector;
use codex_monitor_protocol::{
    default_identity_path, discovery_interface_v4, pairing_message, CollectorStatus,
    DiscoveryAnnouncement, Identity, StateSnapshot, WireMessage, ANNOUNCEMENT_TTL_SECS,
    ANNOUNCE_INTERVAL_SECS, DISCOVERY_GROUP_V4, DISCOVERY_PORT, HANDSHAKE_TIMEOUT_SECS,
    HEARTBEAT_TIMEOUT_SECS, MAX_WIRE_FRAME_BYTES, MONITOR_PORT, PROTOCOL_VERSION, SERVICE_NAME,
    STATE_SNAPSHOT_CAPABILITY,
};
use pairing::{default_pairing_path, PairingStore};
use rand::RngCore;
use registry::Registry;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::env;
use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, RwLock};
use tokio::time::timeout;

type BoxError = Box<dyn Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let identity = Arc::new(Identity::load_or_create(&default_identity_path("monitor"))?);
    let monitor_id = identity.host_id();
    let alias = env::var("CODEX_MONITOR_ALIAS").unwrap_or_else(|_| "Windows Monitor".to_string());
    let pairing_code = env::var("CODEX_MONITOR_PAIR_CODE").unwrap_or_else(|_| generate_code());
    let pairings = Arc::new(Mutex::new(PairingStore::load(default_pairing_path())?));
    let registry = Arc::new(RwLock::new(Registry::default()));

    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, MONITOR_PORT)).await?;
    let discovery_interface = discovery_interface_v4()?;
    let announcement_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    announcement_socket.bind(&SockAddr::from(SocketAddr::from((
        Ipv4Addr::UNSPECIFIED,
        0,
    ))))?;
    announcement_socket.set_multicast_if_v4(&discovery_interface)?;
    announcement_socket.set_multicast_ttl_v4(1)?;
    // Keep loopback enabled so a Monitor-local Agent (the Windows host's own
    // collector) can use the same discovery path as remote Agents.
    announcement_socket.set_multicast_loop_v4(true)?;
    let announcement_socket: StdUdpSocket = announcement_socket.into();
    announcement_socket.set_nonblocking(true)?;
    let announcement_socket = UdpSocket::from_std(announcement_socket)?;

    let monitor_public_key = identity.public_key_hex();
    let capabilities = vec![
        "sessions".to_string(),
        "control".to_string(),
        "usage".to_string(),
        STATE_SNAPSHOT_CAPABILITY.to_string(),
    ];
    let multicast_addr = SocketAddr::from((Ipv4Addr::from(DISCOVERY_GROUP_V4), DISCOVERY_PORT));

    eprintln!("codex-monitor-server id={monitor_id} listening on 0.0.0.0:{MONITOR_PORT}");
    eprintln!("monitor public key: {monitor_public_key}");
    eprintln!("pairing code: {pairing_code}");
    eprintln!("discovery group: {multicast_addr}");
    eprintln!("discovery interface: {discovery_interface}");

    registry.write().await.register(
        monitor_id.clone(),
        alias.clone(),
        capabilities.clone(),
        unix_seconds(),
    );
    let http_bind = env::var("CODEX_MONITOR_HTTP_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8788".to_string())
        .parse::<SocketAddr>()?;
    let http_registry = Arc::clone(&registry);
    tokio::spawn(async move {
        if let Err(error) = http::serve(http_bind, http_registry).await {
            eprintln!("HTTP API stopped: {error}");
        }
    });
    tokio::spawn(run_local_collector(
        Arc::clone(&registry),
        monitor_id.clone(),
        alias.clone(),
    ));

    let announce_monitor_id = monitor_id.clone();
    let announce_alias = alias.clone();
    let announce_public_key = monitor_public_key.clone();
    let announce_capabilities = capabilities.clone();
    let _announce_task = tokio::spawn(async move {
        loop {
            let now = unix_seconds();
            let announcement = DiscoveryAnnouncement {
                service: SERVICE_NAME.to_string(),
                version: PROTOCOL_VERSION,
                host_id: announce_monitor_id.clone(),
                alias: announce_alias.clone(),
                port: MONITOR_PORT,
                public_key: announce_public_key.clone(),
                nonce: random_hex(16),
                issued_at: now,
                expires_at: now + ANNOUNCEMENT_TTL_SECS,
                capabilities: announce_capabilities.clone(),
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
            tokio::time::sleep(Duration::from_secs(ANNOUNCE_INTERVAL_SECS)).await;
        }
    });

    loop {
        let (stream, peer) = listener.accept().await?;
        let pairings = Arc::clone(&pairings);
        let monitor_id = monitor_id.clone();
        let pairing_code = pairing_code.clone();
        let monitor_identity = Arc::clone(&identity);
        let registry = Arc::clone(&registry);
        tokio::spawn(async move {
            if let Err(error) = handle_agent(
                stream,
                peer,
                &monitor_id,
                &pairing_code,
                monitor_identity,
                pairings,
                registry,
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
    pairing_code: &str,
    monitor_identity: Arc<Identity>,
    pairings: Arc<Mutex<PairingStore>>,
    registry: Arc<RwLock<Registry>>,
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
        host_id,
        alias,
        public_key,
        pairing_code: supplied_code,
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
    let expected_host_id = if public_key.len() == 64 {
        format!("ed25519:{}", &public_key[..16])
    } else {
        String::new()
    };
    if host_id.is_empty()
        || host_id.len() > 128
        || alias.is_empty()
        || alias.len() > 128
        || public_key.len() != 64
        || host_id != expected_host_id
        || hex::decode(&public_key).is_err()
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
    if let Some(expected_key) = known_key.as_deref() {
        if expected_key != public_key {
            send_wire(
                &mut write_half,
                &WireMessage::PairRejected {
                    reason: "host id is already paired with another key".to_string(),
                },
            )
            .await?;
            return Err("paired key mismatch".into());
        }
    } else if supplied_code.as_deref() != Some(pairing_code) {
        send_wire(
            &mut write_half,
            &WireMessage::PairRejected {
                reason: "pairing code required".to_string(),
            },
        )
        .await?;
        return Err("pairing code rejected".into());
    }

    let challenge = random_hex(32);
    let challenge_signature =
        monitor_identity.sign_hex(&pairing_message(&challenge, monitor_id, &host_id));
    send_wire(
        &mut write_half,
        &WireMessage::PairChallenge {
            challenge: challenge.clone(),
            monitor_id: monitor_id.to_string(),
            signature: challenge_signature,
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
    } = response
    else {
        return Err("expected pair response".into());
    };
    if response_host_id != host_id
        || !Identity::verify_hex(
            &public_key,
            &pairing_message(&challenge, monitor_id, &host_id),
            &signature,
        )
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

    pairings.lock().await.remember(&host_id, &public_key)?;
    send_wire(
        &mut write_half,
        &WireMessage::PairAccepted {
            host_id: host_id.clone(),
        },
    )
    .await?;
    eprintln!("paired agent {alias} ({host_id}) from {peer}: {capabilities:?}");
    let state_supported = capabilities
        .iter()
        .any(|value| value == STATE_SNAPSHOT_CAPABILITY);
    registry.write().await.register(
        host_id.clone(),
        alias.clone(),
        capabilities.clone(),
        unix_seconds(),
    );

    let result: Result<(), BoxError> = loop {
        let message = timeout(
            Duration::from_secs(HEARTBEAT_TIMEOUT_SECS),
            read_wire(&mut reader),
        )
        .await;
        let message = match message {
            Ok(Ok(message)) => message,
            Ok(Err(error)) => break Err(error),
            Err(error) => break Err(error.into()),
        };
        match message {
            Some(WireMessage::Heartbeat {
                host_id: heartbeat_host_id,
            }) if heartbeat_host_id == host_id => {
                registry.write().await.touch(&host_id, unix_seconds());
                if let Err(error) = send_wire(&mut write_half, &WireMessage::HeartbeatAck).await {
                    break Err(error);
                }
            }
            Some(WireMessage::Heartbeat { .. }) => break Err("heartbeat host id mismatch".into()),
            Some(WireMessage::StateSnapshot { snapshot }) if state_supported => {
                if snapshot.host_id != host_id {
                    break Err("state snapshot host id mismatch".into());
                }
                if let Err(reason) = registry
                    .write()
                    .await
                    .update_snapshot(snapshot, unix_seconds())
                {
                    break Err(reason.into());
                }
            }
            Some(WireMessage::StateSnapshot { .. }) => {
                eprintln!("agent {host_id} sent state without negotiated capability");
            }
            Some(other) => eprintln!("agent {host_id} sent unsupported message: {other:?}"),
            None => break Ok(()),
        }
    };
    registry.write().await.disconnect(&host_id, unix_seconds());
    result
}

async fn run_local_collector(registry: Arc<RwLock<Registry>>, host_id: String, alias: String) {
    let mut sequence: u64 = 0;
    loop {
        let mut collector = match CodexCollector::spawn().await {
            Ok(collector) => collector,
            Err(error) => {
                sequence = sequence.saturating_add(1);
                update_local_snapshot(
                    &registry,
                    unavailable_snapshot(&host_id, &alias, sequence, &error),
                )
                .await;
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };

        loop {
            sequence = sequence.saturating_add(1);
            let captured_at = unix_seconds();
            match collector
                .snapshot(&host_id, &alias, sequence, captured_at)
                .await
            {
                Ok(snapshot) => {
                    update_local_snapshot(&registry, snapshot).await;
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                Err(error) => {
                    update_local_snapshot(
                        &registry,
                        unavailable_snapshot(&host_id, &alias, sequence, &error),
                    )
                    .await;
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn update_local_snapshot(registry: &Arc<RwLock<Registry>>, snapshot: StateSnapshot) {
    if let Err(error) = registry
        .write()
        .await
        .update_snapshot(snapshot, unix_seconds())
    {
        eprintln!("local state snapshot rejected: {error}");
    }
}

fn unavailable_snapshot(host_id: &str, alias: &str, sequence: u64, error: &str) -> StateSnapshot {
    StateSnapshot {
        host_id: host_id.to_string(),
        alias: alias.to_string(),
        sequence,
        captured_at: unix_seconds(),
        collector_status: CollectorStatus::Unavailable,
        error: Some(error.chars().take(256).collect()),
        sessions: Vec::new(),
        account: None,
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
    let read = reader.read_until(b'\n', &mut line).await?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_WIRE_FRAME_BYTES {
        return Err("wire frame exceeds maximum size".into());
    }
    Ok(Some(serde_json::from_slice(&line)?))
}

fn generate_code() -> String {
    format!("{:06}", rand::random::<u32>() % 1_000_000)
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
