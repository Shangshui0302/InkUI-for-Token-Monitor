mod trust;

use codex_monitor_protocol::{
    default_identity_path, discovery_interface_v4, pairing_message, DiscoveryAnnouncement,
    Identity, WireMessage, DISCOVERY_GROUP_V4, DISCOVERY_PORT, HANDSHAKE_TIMEOUT_SECS,
    HEARTBEAT_INTERVAL_SECS, PROTOCOL_VERSION, SERVICE_NAME,
};
use std::env;
use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{sleep, timeout};
use trust::{default_monitor_trust_path, MonitorTrustStore};

type BoxError = Box<dyn Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let identity = Identity::load_or_create(&default_identity_path("agent"))?;
    let alias = env::var("CODEX_MONITOR_ALIAS").unwrap_or_else(|_| local_hostname());
    let pairing_code = env::var("CODEX_MONITOR_PAIR_CODE").ok();
    let mut monitor_trust = MonitorTrustStore::load(default_monitor_trust_path())?;

    eprintln!(
        "codex-monitor-agent host_id={} alias={alias}",
        identity.host_id()
    );
    if pairing_code.is_none() {
        eprintln!("CODEX_MONITOR_PAIR_CODE is unset; only a previously paired host can connect");
    }

    let discovery_interface = discovery_interface_v4()?;
    let socket = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT))?;
    socket.set_nonblocking(true)?;
    socket.join_multicast_v4(&Ipv4Addr::from(DISCOVERY_GROUP_V4), &discovery_interface)?;
    let socket = UdpSocket::from_std(socket)?;
    eprintln!(
        "listening for {SERVICE_NAME} announcements on 239.255.77.77:{} via {discovery_interface}",
        DISCOVERY_PORT,
    );

    let mut buffer = [0u8; 16 * 1024];
    let mut backoff = Duration::from_secs(1);
    loop {
        let (length, source) = socket.recv_from(&mut buffer).await?;
        let now = unix_seconds();
        let announcement: DiscoveryAnnouncement = match serde_json::from_slice(&buffer[..length]) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("ignoring malformed discovery packet from {source}: {error}");
                continue;
            }
        };

        if let Err(reason) = announcement.validate(now) {
            eprintln!("ignoring discovery packet from {source}: {reason}");
            continue;
        }
        if !monitor_trust.accepts(&announcement) {
            eprintln!(
                "ignoring monitor {} with an untrusted public key; remove {} to re-pair",
                announcement.alias,
                monitor_trust.path().display()
            );
            continue;
        }

        let monitor_addr = SocketAddr::new(source.ip(), announcement.port);
        eprintln!(
            "discovered monitor {} at {monitor_addr}",
            announcement.alias
        );
        match connect_and_run(
            monitor_addr,
            &announcement,
            &identity,
            &alias,
            pairing_code.as_deref(),
            &mut monitor_trust,
        )
        .await
        {
            Ok(()) => {
                backoff = Duration::from_secs(1);
                eprintln!("monitor session ended; waiting for the next announcement");
            }
            Err(error) => {
                eprintln!("monitor connection failed: {error}");
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}

async fn connect_and_run(
    monitor_addr: SocketAddr,
    announcement: &DiscoveryAnnouncement,
    identity: &Identity,
    alias: &str,
    pairing_code: Option<&str>,
    monitor_trust: &mut MonitorTrustStore,
) -> Result<(), BoxError> {
    let stream = timeout(Duration::from_secs(5), TcpStream::connect(monitor_addr)).await??;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    send_wire(
        &mut write_half,
        &WireMessage::AgentHello {
            version: PROTOCOL_VERSION,
            host_id: identity.host_id(),
            alias: alias.to_string(),
            public_key: identity.public_key_hex(),
            pairing_code: pairing_code.map(str::to_string),
            capabilities: vec!["sessions".to_string(), "usage".to_string()],
        },
    )
    .await?;

    match timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        read_wire(&mut reader),
    )
    .await??
    .ok_or("monitor closed before pairing")?
    {
        WireMessage::PairChallenge {
            challenge,
            monitor_id,
            signature,
        } => {
            if monitor_id != announcement.host_id
                || !Identity::verify_hex(
                    &announcement.public_key,
                    &pairing_message(&challenge, &monitor_id, &identity.host_id()),
                    &signature,
                )
            {
                return Err("monitor challenge signature rejected".into());
            }
            let signature = identity.sign_hex(&pairing_message(
                &challenge,
                &monitor_id,
                &identity.host_id(),
            ));
            send_wire(
                &mut write_half,
                &WireMessage::PairResponse {
                    host_id: identity.host_id(),
                    signature,
                },
            )
            .await?;
        }
        WireMessage::PairRejected { reason } => return Err(reason.into()),
        other => return Err(format!("unexpected pairing message: {other:?}").into()),
    }

    match timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        read_wire(&mut reader),
    )
    .await??
    .ok_or("monitor closed after challenge")?
    {
        WireMessage::PairAccepted { host_id } if host_id == identity.host_id() => {
            monitor_trust.remember(announcement)?;
            eprintln!("paired with monitor {monitor_addr}");
        }
        WireMessage::PairRejected { reason } => return Err(reason.into()),
        other => return Err(format!("unexpected pairing result: {other:?}").into()),
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    loop {
        tokio::select! {
            message = read_wire(&mut reader) => {
                match message? {
                    Some(WireMessage::HeartbeatAck) => {}
                    Some(WireMessage::PairRejected { reason }) => return Err(reason.into()),
                    Some(other) => eprintln!("monitor sent an unsupported message: {other:?}"),
                    None => return Ok(()),
                }
            }
            _ = heartbeat.tick() => {
                send_wire(
                    &mut write_half,
                    &WireMessage::Heartbeat { host_id: identity.host_id() },
                ).await?;
            }
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
    if line.len() > codex_monitor_protocol::MAX_WIRE_FRAME_BYTES {
        return Err("wire frame exceeds maximum size".into());
    }
    Ok(Some(serde_json::from_slice(&line)?))
}

fn local_hostname() -> String {
    env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| "codex-agent".to_string())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
