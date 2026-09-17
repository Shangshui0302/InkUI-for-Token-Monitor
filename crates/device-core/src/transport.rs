//! Noise XX authenticated WebSocket sessions. Providers must configure the same
//! WebSocket limits before parsing peer input (see `websocket_config`).
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use nekodex_domain::signing::member_id_for_key;
use nekodex_protocol::BusinessCapabilities;
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, io, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async_with_config, client_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use url::Url;

#[cfg(test)]
use crate::types::TRANSPORT_PROTOCOL_VERSION;

pub const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
pub const MAX_FRAME: usize = 65_535;
pub const MAX_PLAINTEXT: usize = MAX_FRAME - 16;
pub const MAX_QUEUED: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

pub trait TransportIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> TransportIo for T {}
pub type Socket = WebSocketStream<Box<dyn TransportIo>>;

pub fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME))
        .max_frame_size(Some(MAX_FRAME))
}

/// Direct and relay providers expose the same framed duplex connection. The
/// provider must bound its HTTP upgrade, enforce `websocket_config`, and use WSS
/// when the deployment requires hiding Noise metadata from intermediaries.
#[async_trait]
pub trait ConnectionProvider: Send + Sync {
    async fn connect(&self, address: &str) -> Result<Socket, TransportError>;
}

/// LAN-direct provider. Payload confidentiality and peer authentication come
/// from the mandatory Noise session established after this WebSocket upgrade.
#[derive(Clone, Copy, Debug, Default)]
pub struct DirectWebSocketProvider;

#[async_trait]
impl ConnectionProvider for DirectWebSocketProvider {
    async fn connect(&self, address: &str) -> Result<Socket, TransportError> {
        let url = Url::parse(address).map_err(|_| TransportError::InvalidAddress)?;
        if url.scheme() != "ws" || url.path() != "/device" {
            return Err(TransportError::InvalidAddress);
        }
        let host = url.host_str().ok_or(TransportError::InvalidAddress)?;
        let port = url
            .port_or_known_default()
            .ok_or(TransportError::InvalidAddress)?;
        let stream = timeout(IO_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| TransportError::Timeout)??;
        let transport: Box<dyn TransportIo> = Box::new(stream);
        let (socket, _) = timeout(
            IO_TIMEOUT,
            client_async_with_config(address, transport, Some(websocket_config())),
        )
        .await
        .map_err(|_| TransportError::Timeout)??;
        Ok(socket)
    }
}

pub async fn accept_direct(stream: TcpStream) -> Result<Socket, TransportError> {
    let (socket, route) = accept_routed(stream).await?;
    if route != WebSocketRoute::Device {
        return Err(TransportError::Protocol);
    }
    Ok(socket)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebSocketRoute {
    Device,
    Join,
    Unknown,
}

pub async fn accept_routed(stream: TcpStream) -> Result<(Socket, WebSocketRoute), TransportError> {
    let selected = std::sync::Arc::new(std::sync::Mutex::new(WebSocketRoute::Unknown));
    let selected_by_callback = std::sync::Arc::clone(&selected);
    let transport: Box<dyn TransportIo> = Box::new(stream);
    let socket = timeout(
        IO_TIMEOUT,
        accept_hdr_async_with_config(
            transport,
            move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                  response| {
                let route = match request.uri().path() {
                    "/device" => WebSocketRoute::Device,
                    "/join" => WebSocketRoute::Join,
                    _ => WebSocketRoute::Unknown,
                };
                *selected_by_callback.lock().expect("route mutex poisoned") = route;
                Ok(response)
            },
            Some(websocket_config()),
        ),
    )
    .await
    .map_err(|_| TransportError::Timeout)??;
    let route = *selected.lock().map_err(|_| TransportError::Protocol)?;
    Ok((socket, route))
}

pub struct PeerCredentials {
    pub device_id: String,
    pub group_id: String,
    pub transport_version: u16,
    pub signing_key: SigningKey,
    pub noise_private_key: [u8; 32],
    pub business_capabilities: BusinessCapabilities,
}

/// Implementations must check the current authoritative membership state on
/// every call, including revocation. Sync must verify signed log entries before
/// persisting them; receiving arbitrary bytes never constitutes synchronization.
#[async_trait]
pub trait MembershipAuthorizer: Send + Sync {
    async fn authorize(
        &self,
        group: &str,
        device: &str,
        key: &[u8; 32],
    ) -> Result<(), TransportError>;
    async fn export_sync(&self, group: &str) -> Result<Vec<u8>, TransportError>;
    async fn apply_sync(&self, group: &str, device: &str, log: &[u8])
    -> Result<(), TransportError>;
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("transport timeout")]
    Timeout,
    #[error("transport closed")]
    Closed,
    #[error("invalid or out-of-order transport frame")]
    Protocol,
    #[error("transport frame exceeds limit")]
    Oversized,
    #[error("outbound queue is full")]
    QueueFull,
    #[error("peer authentication or membership rejected")]
    Unauthorized,
    #[error("incompatible transport protocol version (expected {expected}, got {actual})")]
    IncompatibleVersion { expected: u16, actual: u16 },
    #[error("incompatible business protocol or missing required capability")]
    IncompatibleBusinessProtocol,
    #[error("invalid direct WebSocket address")]
    InvalidAddress,
    #[error("I/O: {0}")]
    Io(#[from] io::Error),
    #[error("Noise: {0}")]
    Noise(#[from] snow::Error),
    #[error("WebSocket: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Frame {
    Proof {
        device: String,
        group: String,
        version: u16,
        key: [u8; 32],
        signature: Vec<u8>,
    },
    Membership {
        log: Vec<u8>,
    },
    Negotiation {
        capabilities: BusinessCapabilities,
    },
    Synced,
    Business {
        payload: Vec<u8>,
    },
    BusinessBase64 {
        payload: String,
    },
}

fn transcript(hash: &[u8], group: &str, version: u16, device: &str, noise_key: &[u8]) -> Vec<u8> {
    let mut result = b"nekodex/noise-membership-proof/v1\0".to_vec();
    for field in [
        hash,
        group.as_bytes(),
        &version.to_be_bytes(),
        device.as_bytes(),
        noise_key,
    ] {
        result.extend_from_slice(&(field.len() as u64).to_be_bytes());
        result.extend_from_slice(field);
    }
    result
}

fn verify_device_id(device: &str, key: &VerifyingKey) -> Result<(), TransportError> {
    if device != member_id_for_key(key) {
        return Err(TransportError::Unauthorized);
    }
    Ok(())
}

async fn write<S: TransportIo>(
    socket: &mut WebSocketStream<S>,
    bytes: Vec<u8>,
) -> Result<(), TransportError> {
    if bytes.len() > MAX_FRAME {
        return Err(TransportError::Oversized);
    }
    timeout(IO_TIMEOUT, socket.send(Message::Binary(bytes.into())))
        .await
        .map_err(|_| TransportError::Timeout)??;
    Ok(())
}
async fn read<S: TransportIo>(socket: &mut WebSocketStream<S>) -> Result<Vec<u8>, TransportError> {
    let msg = timeout(IO_TIMEOUT, socket.next())
        .await
        .map_err(|_| TransportError::Timeout)?
        .ok_or(TransportError::Closed)??;
    match msg {
        Message::Binary(bytes) if bytes.len() <= MAX_FRAME => Ok(bytes.to_vec()),
        Message::Binary(_) => Err(TransportError::Oversized),
        Message::Close(_) => Err(TransportError::Closed),
        _ => Err(TransportError::Protocol),
    }
}

/// Only constructible after both encrypted membership synchronization and its
/// acknowledgement. Any I/O error poisons the session. Cancelling a receive
/// while waiting for a complete WebSocket message is safe because no Noise
/// nonce has been consumed yet.
pub struct OnlineSession<S: TransportIo> {
    socket: WebSocketStream<S>,
    noise: snow::TransportState,
    outbound: VecDeque<Vec<u8>>,
    pub peer_device_id: String,
    group_id: String,
    pub negotiated_capabilities: BusinessCapabilities,
    poisoned: bool,
}

impl<S: TransportIo> OnlineSession<S> {
    fn uses_compact_business_frames(&self) -> bool {
        self.negotiated_capabilities
            .features
            .contains(BusinessCapabilities::COMPACT_PAYLOAD_FEATURE)
    }

    fn business_frame(&self, payload: Vec<u8>) -> Frame {
        if self.uses_compact_business_frames() {
            Frame::BusinessBase64 {
                payload: STANDARD_NO_PAD.encode(payload),
            }
        } else {
            Frame::Business { payload }
        }
    }

    pub fn group_id(&self) -> &str {
        &self.group_id
    }
    async fn receive_membership(
        &mut self,
        group: &str,
        authorizer: &dyn MembershipAuthorizer,
    ) -> Result<(), TransportError> {
        match self.receive_frame().await? {
            Frame::Membership { log } => {
                authorizer
                    .apply_sync(group, &self.peer_device_id, &log)
                    .await
            }
            _ => {
                self.poisoned = true;
                Err(TransportError::Protocol)
            }
        }
    }

    pub fn negotiated_capabilities(&self) -> &BusinessCapabilities {
        &self.negotiated_capabilities
    }
    async fn send_frame(&mut self, frame: &Frame) -> Result<(), TransportError> {
        if self.poisoned {
            return Err(TransportError::Closed);
        }
        let plaintext = serde_json::to_vec(frame).map_err(|_| TransportError::Protocol)?;
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(TransportError::Oversized);
        }
        self.poisoned = true;
        let mut bytes = vec![0; MAX_FRAME];
        let n = self.noise.write_message(&plaintext, &mut bytes)?;
        bytes.truncate(n);
        write(&mut self.socket, bytes).await?;
        self.poisoned = false;
        Ok(())
    }
    async fn receive_frame(&mut self) -> Result<Frame, TransportError> {
        if self.poisoned {
            return Err(TransportError::Closed);
        }
        let bytes = match read(&mut self.socket).await {
            Ok(bytes) => bytes,
            Err(error) => {
                self.poisoned = true;
                return Err(error);
            }
        };
        self.poisoned = true;
        let mut plaintext = vec![0; MAX_FRAME];
        let n = self.noise.read_message(&bytes, &mut plaintext)?;
        let frame =
            serde_json::from_slice(&plaintext[..n]).map_err(|_| TransportError::Protocol)?;
        self.poisoned = false;
        Ok(frame)
    }
    /// Queue admission is synchronous and bounded by both count and encoded size.
    pub fn enqueue(&mut self, payload: Vec<u8>) -> Result<(), TransportError> {
        if self.poisoned {
            return Err(TransportError::Closed);
        }
        if payload.len() > MAX_PLAINTEXT {
            return Err(TransportError::Oversized);
        }
        if serde_json::to_vec(&self.business_frame(payload.clone()))
        .map_err(|_| TransportError::Protocol)?
        .len()
            > MAX_PLAINTEXT
        {
            return Err(TransportError::Oversized);
        }
        if self.outbound.len() >= MAX_QUEUED {
            return Err(TransportError::QueueFull);
        }
        self.outbound.push_back(payload);
        Ok(())
    }
    pub async fn flush(&mut self) -> Result<(), TransportError> {
        while let Some(payload) = self.outbound.pop_front() {
            let frame = self.business_frame(payload);
            self.send_frame(&frame).await?;
        }
        Ok(())
    }
    pub async fn receive(&mut self) -> Result<Vec<u8>, TransportError> {
        match self.receive_frame().await? {
            Frame::Business { payload } if !self.uses_compact_business_frames() => Ok(payload),
            Frame::BusinessBase64 { payload } if self.uses_compact_business_frames() => {
                STANDARD_NO_PAD
                    .decode(payload)
                    .map_err(|_| TransportError::Protocol)
            }
            _ => {
                self.poisoned = true;
                Err(TransportError::Protocol)
            }
        }
    }
}

pub async fn establish<S: TransportIo>(
    socket: WebSocketStream<S>,
    initiator: bool,
    credentials: &PeerCredentials,
    authorizer: &dyn MembershipAuthorizer,
) -> Result<OnlineSession<S>, TransportError> {
    timeout(
        HANDSHAKE_TIMEOUT,
        establish_inner(socket, initiator, credentials, authorizer),
    )
    .await
    .map_err(|_| TransportError::Timeout)?
}

async fn establish_inner<S: TransportIo>(
    mut socket: WebSocketStream<S>,
    initiator: bool,
    c: &PeerCredentials,
    authorizer: &dyn MembershipAuthorizer,
) -> Result<OnlineSession<S>, TransportError> {
    verify_device_id(&c.device_id, &c.signing_key.verifying_key())?;
    let config = socket.get_config();
    if config.max_message_size.is_none_or(|n| n > MAX_FRAME)
        || config.max_frame_size.is_none_or(|n| n > MAX_FRAME)
    {
        return Err(TransportError::Protocol);
    }
    let local_noise_key =
        x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(c.noise_private_key))
            .to_bytes();
    let builder =
        snow::Builder::new(NOISE_PATTERN.parse()?).local_private_key(&c.noise_private_key)?;
    let mut noise = if initiator {
        builder.build_initiator()?
    } else {
        builder.build_responder()?
    };
    let mut buffer = vec![0; MAX_FRAME];
    for turn in 0..3 {
        if (turn % 2 == 0) == initiator {
            let n = noise.write_message(&[], &mut buffer)?;
            write(&mut socket, buffer[..n].to_vec()).await?;
        } else {
            let bytes = read(&mut socket).await?;
            if noise.read_message(&bytes, &mut buffer)? != 0 {
                return Err(TransportError::Protocol);
            }
        }
    }
    let hash = noise.get_handshake_hash().to_vec();
    let remote_noise_key = noise
        .get_remote_static()
        .ok_or(TransportError::Unauthorized)?
        .to_vec();
    let signature = c.signing_key.sign(&transcript(
        &hash,
        &c.group_id,
        c.transport_version,
        &c.device_id,
        &local_noise_key,
    ));
    let mut session = OnlineSession {
        socket,
        noise: noise.into_transport_mode()?,
        outbound: VecDeque::new(),
        peer_device_id: String::new(),
        group_id: c.group_id.clone(),
        negotiated_capabilities: c.business_capabilities.clone(),
        poisoned: false,
    };
    session
        .send_frame(&Frame::Proof {
            device: c.device_id.clone(),
            group: c.group_id.clone(),
            version: c.transport_version,
            key: c.signing_key.verifying_key().to_bytes(),
            signature: signature.to_bytes().to_vec(),
        })
        .await?;
    let (device, key) = match session.receive_frame().await? {
        Frame::Proof {
            device,
            group,
            version,
            key,
            signature,
        } => {
            if group != c.group_id {
                return Err(TransportError::Unauthorized);
            }
            if version != c.transport_version {
                return Err(TransportError::IncompatibleVersion {
                    expected: c.transport_version,
                    actual: version,
                });
            }
            let verifying =
                VerifyingKey::from_bytes(&key).map_err(|_| TransportError::Unauthorized)?;
            verify_device_id(&device, &verifying)?;
            let signature =
                Signature::from_slice(&signature).map_err(|_| TransportError::Unauthorized)?;
            verifying
                .verify_strict(
                    &transcript(&hash, &group, version, &device, &remote_noise_key),
                    &signature,
                )
                .map_err(|_| TransportError::Unauthorized)?;
            (device, key)
        }
        _ => return Err(TransportError::Protocol),
    };
    session.peer_device_id = device;
    // Capability exchange is authenticated by the completed Noise handshake
    // and must finish before either side is allowed to synchronize membership.
    session
        .send_frame(&Frame::Negotiation {
            capabilities: c.business_capabilities.clone(),
        })
        .await?;
    let remote_capabilities = match session.receive_frame().await? {
        Frame::Negotiation { capabilities } => capabilities,
        _ => return Err(TransportError::Protocol),
    };
    session.negotiated_capabilities = c
        .business_capabilities
        .negotiate(&remote_capabilities)
        .map_err(|_| TransportError::IncompatibleBusinessProtocol)?;
    session
        .send_frame(&Frame::Membership {
            log: authorizer.export_sync(&c.group_id).await?,
        })
        .await?;
    session.receive_membership(&c.group_id, authorizer).await?;
    authorizer
        .authorize(&c.group_id, &session.peer_device_id, &key)
        .await?;
    session.send_frame(&Frame::Synced).await?;
    if !matches!(session.receive_frame().await?, Frame::Synced) {
        return Err(TransportError::Protocol);
    }
    authorizer
        .authorize(&c.group_id, &session.peer_device_id, &key)
        .await?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio_tungstenite::tungstenite::protocol::Role;

    struct Members {
        accepted: std::collections::HashMap<String, [u8; 32]>,
        revoked: AtomicBool,
        synced: AtomicBool,
    }
    #[async_trait]
    impl MembershipAuthorizer for Members {
        async fn authorize(
            &self,
            _: &str,
            device: &str,
            key: &[u8; 32],
        ) -> Result<(), TransportError> {
            if self.revoked.load(Ordering::SeqCst) || self.accepted.get(device) != Some(key) {
                return Err(TransportError::Unauthorized);
            }
            Ok(())
        }
        async fn export_sync(&self, _: &str) -> Result<Vec<u8>, TransportError> {
            Ok(b"verified-log".to_vec())
        }
        async fn apply_sync(&self, _: &str, _: &str, log: &[u8]) -> Result<(), TransportError> {
            if log != b"verified-log" {
                return Err(TransportError::Unauthorized);
            }
            self.synced.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    fn credentials(seed: u8) -> PeerCredentials {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        PeerCredentials {
            device_id: member_id_for_key(&signing_key.verifying_key()),
            group_id: "group".into(),
            transport_version: crate::TRANSPORT_PROTOCOL_VERSION,
            business_capabilities: BusinessCapabilities::current(),
            signing_key,
            noise_private_key: [seed.wrapping_add(100); 32],
        }
    }
    fn members(a: &PeerCredentials, b: &PeerCredentials) -> Members {
        Members {
            accepted: [
                (
                    a.device_id.clone(),
                    a.signing_key.verifying_key().to_bytes(),
                ),
                (
                    b.device_id.clone(),
                    b.signing_key.verifying_key().to_bytes(),
                ),
            ]
            .into_iter()
            .collect(),
            revoked: AtomicBool::new(false),
            synced: AtomicBool::new(false),
        }
    }
    async fn sockets() -> (
        WebSocketStream<tokio::net::TcpStream>,
        WebSocketStream<tokio::net::TcpStream>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, server) = tokio::join!(
            tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept()
        );
        let (a, b) = tokio::join!(
            WebSocketStream::from_raw_socket(
                client.unwrap(),
                Role::Client,
                Some(websocket_config())
            ),
            WebSocketStream::from_raw_socket(
                server.unwrap().0,
                Role::Server,
                Some(websocket_config())
            )
        );
        (a, b)
    }
    #[tokio::test]
    async fn forged_device_id_rejected() {
        let (a, _b) = sockets().await;
        let (mut ca, cb) = (credentials(1), credentials(2));
        let auth = members(&ca, &cb);
        assert_ne!(ca.device_id, cb.device_id);
        ca.device_id = cb.device_id.clone();
        assert!(matches!(
            establish(a, true, &ca, &auth).await,
            Err(TransportError::Unauthorized)
        ));
        // This is also the receive-side identity gate, before membership authorization.
        assert!(matches!(
            verify_device_id(&ca.device_id, &ca.signing_key.verifying_key()),
            Err(TransportError::Unauthorized)
        ));
        assert!(
            auth.authorize(
                "group",
                &ca.device_id,
                &ca.signing_key.verifying_key().to_bytes()
            )
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn encrypted_roundtrip_and_bounded_queue() {
        let (a, b) = sockets().await;
        let (ca, cb) = (credentials(1), credentials(2));
        let auth = Arc::new(members(&ca, &cb));
        let (a, b) = tokio::join!(
            establish(a, true, &ca, auth.as_ref()),
            establish(b, false, &cb, auth.as_ref())
        );
        let (mut a, mut b) = (a.unwrap(), b.unwrap());
        assert!(auth.synced.load(Ordering::SeqCst));
        a.enqueue(b"secret".to_vec()).unwrap();
        let (sent, received) = tokio::join!(a.flush(), b.receive());
        sent.unwrap();
        assert_eq!(received.unwrap(), b"secret");
        b.enqueue(b"reply".to_vec()).unwrap();
        let (sent, received) = tokio::join!(b.flush(), a.receive());
        sent.unwrap();
        assert_eq!(received.unwrap(), b"reply");
        let large = vec![b'x'; 40 * 1024];
        a.enqueue(large.clone()).unwrap();
        let (sent, received) = tokio::join!(a.flush(), b.receive());
        sent.unwrap();
        assert_eq!(received.unwrap(), large);
        assert!(matches!(
            a.enqueue(vec![0; MAX_FRAME]),
            Err(TransportError::Oversized)
        ));
        for _ in 0..MAX_QUEUED {
            a.enqueue(vec![1]).unwrap();
        }
        assert!(matches!(a.enqueue(vec![1]), Err(TransportError::QueueFull)));
    }

    #[tokio::test]
    async fn legacy_business_peer_keeps_bounded_byte_array_frames() {
        let (a, b) = sockets().await;
        let (mut ca, cb) = (credentials(31), credentials(32));
        ca.business_capabilities = BusinessCapabilities::baseline();
        let auth = Arc::new(members(&ca, &cb));
        let (a, b) = tokio::join!(
            establish(a, true, &ca, auth.as_ref()),
            establish(b, false, &cb, auth.as_ref())
        );
        let (mut a, b) = (a.unwrap(), b.unwrap());
        assert!(!a.uses_compact_business_frames());
        assert!(!b.uses_compact_business_frames());
        assert!(matches!(
            a.enqueue(vec![b'x'; 20 * 1024]),
            Err(TransportError::Oversized)
        ));
    }

    #[tokio::test]
    async fn cancelled_receive_does_not_poison_session() {
        let (a, b) = sockets().await;
        let (ca, cb) = (credentials(21), credentials(22));
        let auth = members(&ca, &cb);
        let (a, b) = tokio::join!(
            establish(a, true, &ca, &auth),
            establish(b, false, &cb, &auth)
        );
        let (mut a, mut b) = (a.unwrap(), b.unwrap());

        assert!(
            tokio::time::timeout(Duration::from_millis(10), a.receive())
                .await
                .is_err()
        );
        a.enqueue(b"after-cancel".to_vec()).unwrap();
        let (sent, received) = tokio::join!(a.flush(), b.receive());
        sent.unwrap();
        assert_eq!(received.unwrap(), b"after-cancel");
    }

    #[tokio::test]
    async fn business_actor_routes_simultaneous_requests_both_ways() {
        use crate::business::{BusinessConnection, BusinessLimits};
        use nekodex_protocol::{Operation, ProtocolVersion, RequestEnvelope, ResponseEnvelope};
        use serde_json::json;

        let (a, b) = sockets().await;
        let (ca, cb) = (credentials(31), credentials(32));
        let auth = Arc::new(members(&ca, &cb));
        let (a, b) = tokio::join!(
            establish(a, true, &ca, auth.as_ref()),
            establish(b, false, &cb, auth.as_ref())
        );
        let (client_a, mut server_a, _events_a, _membership_a) = BusinessConnection::spawn(
            a.unwrap(),
            BusinessLimits::default(),
            auth.clone(),
        );
        let (client_b, mut server_b, _events_b, _membership_b) = BusinessConnection::spawn(
            b.unwrap(),
            BusinessLimits::default(),
            auth,
        );
        // Ensure both actors are already blocked in their read branch before
        // local requests arrive; each command must safely cancel that read.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let request_a = RequestEnvelope {
            version: ProtocolVersion::V1,
            request_id: "request-a".into(),
            source_device_id: ca.device_id.clone(),
            target_device_id: cb.device_id.clone(),
            operation: Operation::ProjectsList,
            params: json!({}),
        };
        let request_b = RequestEnvelope {
            version: ProtocolVersion::V1,
            request_id: "request-b".into(),
            source_device_id: cb.device_id,
            target_device_id: ca.device_id,
            operation: Operation::ProjectsList,
            params: json!({}),
        };

        let exchange = async {
            let side_a = async {
                let request = client_a.request(request_a, Duration::from_secs(1));
                let respond = async {
                    let incoming = server_a.receive_request().await.unwrap();
                    server_a
                        .send_response(ResponseEnvelope {
                            version: ProtocolVersion::V1,
                            request_id: incoming.request_id,
                            result: Some(json!("from-a")),
                            error: None,
                        })
                        .await
                        .unwrap();
                };
                let (response, ()) = tokio::join!(request, respond);
                response.unwrap()
            };
            let side_b = async {
                let request = client_b.request(request_b, Duration::from_secs(1));
                let respond = async {
                    let incoming = server_b.receive_request().await.unwrap();
                    server_b
                        .send_response(ResponseEnvelope {
                            version: ProtocolVersion::V1,
                            request_id: incoming.request_id,
                            result: Some(json!("from-b")),
                            error: None,
                        })
                        .await
                        .unwrap();
                };
                let (response, ()) = tokio::join!(request, respond);
                response.unwrap()
            };
            tokio::join!(side_a, side_b)
        };
        let (response_a, response_b) = tokio::time::timeout(Duration::from_secs(2), exchange)
            .await
            .unwrap();
        assert_eq!(response_a.result, Some(json!("from-b")));
        assert_eq!(response_b.result, Some(json!("from-a")));
    }
    #[tokio::test]
    async fn unknown_wrong_group_wrong_version_and_revoked_rejected() {
        for case in 0..4 {
            let (a, b) = sockets().await;
            let (ca, mut cb) = (credentials(1), credentials(2));
            let mut auth = members(&ca, &cb);
            match case {
                0 => auth.accepted.clear(),
                1 => cb.group_id = "other".into(),
                2 => cb.transport_version = cb.transport_version.saturating_add(1),
                _ => auth.revoked.store(true, Ordering::SeqCst),
            }
            let (a, b) = tokio::join!(
                establish(a, true, &ca, &auth),
                establish(b, false, &cb, &auth)
            );
            if case == 2 {
                assert!(matches!(
                    a,
                    Err(TransportError::IncompatibleVersion {
                        expected: TRANSPORT_PROTOCOL_VERSION,
                        actual
                    }) if actual == TRANSPORT_PROTOCOL_VERSION + 1
                ));
                assert!(matches!(
                    b,
                    Err(TransportError::IncompatibleVersion {
                        expected,
                        actual: TRANSPORT_PROTOCOL_VERSION
                    }) if expected == TRANSPORT_PROTOCOL_VERSION + 1
                ));
            } else {
                assert!(matches!(a, Err(TransportError::Unauthorized)));
                assert!(matches!(b, Err(TransportError::Unauthorized)));
            }
        }
    }
    #[test]
    fn signature_binds_all_transcript_fields() {
        let c = credentials(1);
        let original = transcript(&[1; 32], "group", 1, "device", &[2; 32]);
        let signature = c.signing_key.sign(&original);
        for changed in [
            transcript(&[3; 32], "group", 1, "device", &[2; 32]),
            transcript(&[1; 32], "other", 1, "device", &[2; 32]),
            transcript(&[1; 32], "group", 2, "device", &[2; 32]),
            transcript(&[1; 32], "group", 1, "other", &[2; 32]),
            transcript(&[1; 32], "group", 1, "device", &[3; 32]),
        ] {
            assert!(
                c.signing_key
                    .verifying_key()
                    .verify_strict(&changed, &signature)
                    .is_err()
            );
        }
    }
    #[tokio::test]
    async fn oversized_websocket_rejected_before_noise() {
        let (mut a, mut b) = sockets().await;
        let (sent, received) = tokio::join!(
            a.send(Message::Binary(vec![0; MAX_FRAME + 1].into())),
            read(&mut b)
        );
        sent.unwrap();
        assert!(matches!(
            received,
            Err(TransportError::WebSocket(_)) | Err(TransportError::Oversized)
        ));
    }
    #[tokio::test]
    async fn business_frame_rejected_by_membership_sync_gate() {
        let (a, b) = sockets().await;
        let (ca, cb) = (credentials(1), credentials(2));
        let auth = members(&ca, &cb);
        let (a, b) = tokio::join!(
            establish(a, true, &ca, &auth),
            establish(b, false, &cb, &auth)
        );
        let (mut a, mut b) = (a.unwrap(), b.unwrap());
        auth.synced.store(false, Ordering::SeqCst);
        let frame = Frame::Business { payload: vec![1] };
        let (sent, received) =
            tokio::join!(a.send_frame(&frame), b.receive_membership("group", &auth));
        sent.unwrap();
        assert!(matches!(received, Err(TransportError::Protocol)));
        assert!(!auth.synced.load(Ordering::SeqCst));
        assert!(matches!(b.receive().await, Err(TransportError::Closed)));
    }

    #[tokio::test]
    async fn direct_provider_upgrades_then_runs_authenticated_noise() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("ws://{}/device", listener.local_addr().unwrap());
        let (ca, cb) = (credentials(11), credentials(12));
        let auth = members(&ca, &cb);
        let client = async {
            let socket = DirectWebSocketProvider.connect(&address).await.unwrap();
            establish(socket, true, &ca, &auth).await
        };
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let socket = accept_direct(stream).await.unwrap();
            establish(socket, false, &cb, &auth).await
        };
        let (client, server) = tokio::join!(client, server);
        let (mut client, mut server) = (client.unwrap(), server.unwrap());
        client.enqueue(b"direct".to_vec()).unwrap();
        let (sent, received) = tokio::join!(client.flush(), server.receive());
        sent.unwrap();
        assert_eq!(received.unwrap(), b"direct");
    }
}
