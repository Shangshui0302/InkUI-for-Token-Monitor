mod pairing;

use codex_monitor_protocol::{
    default_identity_path, discovery_interface_v4, pairing_message, DiscoveryAnnouncement,
    Identity, WireMessage, ANNOUNCEMENT_TTL_SECS, ANNOUNCE_INTERVAL_SECS, DISCOVERY_GROUP_V4,
    DISCOVERY_PORT, HANDSHAKE_TIMEOUT_SECS, HEARTBEAT_TIMEOUT_SECS, MAX_WIRE_FRAME_BYTES,
    MONITOR_PORT, PROTOCOL_VERSION, SERVICE_NAME,
};
use pairing::{default_pairing_path, PairingStore};
use rand::RngCore;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::env;
use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::time::timeout;

type BoxError = Box<dyn Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let identity = Arc::new(Identity::load_or_create(&default_identity_path("monitor"))?);
    let monitor_id = identity.host_id();
    let alias = env::var("CODEX_MONITOR_ALIAS").unwrap_or_else(|_| "Windows Monitor".to_string());
    let pairing_code = env::var("CODEX_MONITOR_PAIR_CODE").unwrap_or_else(|_| generate_code());
    let pairings = Arc::new(Mutex::new(PairingStore::load(default_pairing_path())?));

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
    ];
    let multicast_addr = SocketAddr::from((Ipv4Addr::from(DISCOVERY_GROUP_V4), DISCOVERY_PORT));

    eprintln!("codex-monitor-server id={monitor_id} listening on 0.0.0.0:{MONITOR_PORT}");
    eprintln!("monitor public key: {monitor_public_key}");
    eprintln!("pairing code: {pairing_code}");
    eprintln!("discovery group: {multicast_addr}");
    eprintln!("discovery interface: {discovery_interface}");

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
        tokio::spawn(async move {
            if let Err(error) = handle_agent(
                stream,
                peer,
                &monitor_id,
                &pairing_code,
                monitor_identity,
                pairings,
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

    loop {
        let message = timeout(
            Duration::from_secs(HEARTBEAT_TIMEOUT_SECS),
            read_wire(&mut reader),
        )
        .await??;
        match message {
            Some(WireMessage::Heartbeat {
                host_id: heartbeat_host_id,
            }) if heartbeat_host_id == host_id => {
                send_wire(&mut write_half, &WireMessage::HeartbeatAck).await?;
            }
            Some(WireMessage::Heartbeat { .. }) => return Err("heartbeat host id mismatch".into()),
            Some(other) => eprintln!("agent {host_id} sent unsupported message: {other:?}"),
            None => return Ok(()),
        }
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
