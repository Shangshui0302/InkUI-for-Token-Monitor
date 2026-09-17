//! WebSocket bootstrap for joining an existing device group.
//!
//! The short code and PAKE session key are deliberately absent from every wire
//! frame.  A successful server commit must atomically add the paired member and
//! seal the resulting current membership bundle before it returns.

use std::{error::Error as StdError, future::Future, io, time::Duration};

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::{Instant, timeout, timeout_at};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use url::Url;

use crate::identity::DeviceIdentity;
use crate::membership::{
    ActiveInvitation, InvitationOffer, InviterConfirmation, JoinerConfirmation, JoinerHello,
    JoinerPairing, PairingError, PairingTrust, SealedMembershipBundle, validate_envelope,
    validate_inviter_confirmation, validate_joiner_confirmation, validate_joiner_hello,
    validate_offer, validate_profile_input,
};
use crate::transport::{Socket, TransportIo};
use nekodex_domain::MemberProfile;

pub const JOIN_PROTOCOL_VERSION: u16 = 2;
pub const JOIN_MAX_FRAME: usize = 65 * 1024;
pub const JOIN_IO_TIMEOUT: Duration = Duration::from_secs(10);
pub const JOIN_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
pub const JOIN_APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Use this configuration while creating or upgrading the WebSocket. The
/// receive path also checks the assembled message size as defense in depth.
pub fn join_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(JOIN_MAX_FRAME))
        .max_frame_size(Some(JOIN_MAX_FRAME))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinErrorCode {
    MalformedFrame,
    UnsupportedVersion,
    OutOfOrder,
    InvitationRejected,
    PairingRejected,
    ApprovalRejected,
    ApprovalTimedOut,
    CommitFailed,
}

/// Authenticated joiner details that may be shown to the inviter before the
/// membership transaction is allowed to run. Any included profile has already
/// passed its identity-bound signature proof.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JoinCandidate {
    pub joiner_id: String,
    pub profile: Option<MemberProfile>,
}

/// Every bootstrap message carries an explicit protocol version. Nested
/// membership types also reject unknown fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JoinFrame {
    Offer {
        version: u16,
        offer: InvitationOffer,
    },
    JoinerHello {
        version: u16,
        hello: JoinerHello,
    },
    InviterConfirmation {
        version: u16,
        confirmation: InviterConfirmation,
    },
    JoinerConfirmation {
        version: u16,
        confirmation: JoinerConfirmation,
    },
    MembershipBundle {
        version: u16,
        bundle: SealedMembershipBundle,
    },
    Error {
        version: u16,
        code: JoinErrorCode,
    },
}

impl JoinFrame {
    fn version(&self) -> u16 {
        match self {
            Self::Offer { version, .. }
            | Self::JoinerHello { version, .. }
            | Self::InviterConfirmation { version, .. }
            | Self::JoinerConfirmation { version, .. }
            | Self::MembershipBundle { version, .. }
            | Self::Error { version, .. } => *version,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JoinTransportError {
    #[error("join handshake timed out")]
    HandshakeTimeout,
    #[error("join transport I/O timed out")]
    IoTimeout,
    #[error("join transport closed")]
    Closed,
    #[error("join frame exceeds 65 KiB")]
    Oversized,
    #[error("WebSocket was not configured with the 65 KiB join limits")]
    UnsafeWebSocketConfig,
    #[error("malformed join frame")]
    MalformedFrame,
    #[error("unsupported join protocol version")]
    UnsupportedVersion,
    #[error("out-of-order join frame")]
    OutOfOrder,
    #[error("remote rejected join: {0:?}")]
    Remote(JoinErrorCode),
    #[error("pairing failed: {0}")]
    Pairing(#[from] PairingError),
    #[error("inviter rejected the authenticated join candidate")]
    ApprovalRejected,
    #[error("inviter approval timed out")]
    ApprovalTimeout,
    #[error("membership commit failed: {0}")]
    Commit(String),
    #[error("WebSocket: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("invalid join WebSocket address")]
    InvalidAddress,
    #[error("join transport I/O: {0}")]
    Io(#[from] io::Error),
}

pub async fn connect_join(address: &str) -> Result<Socket, JoinTransportError> {
    let url = Url::parse(address).map_err(|_| JoinTransportError::InvalidAddress)?;
    if url.scheme() != "ws" || url.path() != "/join" {
        return Err(JoinTransportError::InvalidAddress);
    }
    let host = url.host_str().ok_or(JoinTransportError::InvalidAddress)?;
    let port = url
        .port_or_known_default()
        .ok_or(JoinTransportError::InvalidAddress)?;
    let stream = timeout(JOIN_IO_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| JoinTransportError::IoTimeout)??;
    let transport: Box<dyn TransportIo> = Box::new(stream);
    let (socket, _) = timeout(
        JOIN_IO_TIMEOUT,
        client_async_with_config(address, transport, Some(join_websocket_config())),
    )
    .await
    .map_err(|_| JoinTransportError::IoTimeout)??;
    Ok(socket)
}

/// The implementation owns the durable transaction boundary. It must add the
/// paired member and seal the resulting *current* full bundle atomically: an
/// error or cancellation must leave durable membership unchanged.
#[async_trait]
pub trait InvitationCommitter: Send {
    type Error: StdError + Send + Sync + 'static;

    async fn add_paired_member_and_seal(
        &mut self,
        trust: &PairingTrust,
    ) -> Result<SealedMembershipBundle, Self::Error>;
}

/// Serve one already-created, one-time invitation.
pub async fn serve_invitation<S, C>(
    socket: &mut WebSocketStream<S>,
    invitation: &mut ActiveInvitation,
    now: u64,
    committer: &mut C,
) -> Result<(), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: InvitationCommitter,
{
    serve_invitation_with_approval(socket, invitation, now, committer, |_| async { true }).await
}

/// Serve an invitation and pause for inviter approval after mutual PAKE and
/// any joiner profile proof have been verified, but before durable membership
/// is changed. The approval wait has its own deadline and therefore does not
/// consume the cryptographic handshake's 30-second budget.
pub async fn serve_invitation_with_approval<S, C, A, F>(
    socket: &mut WebSocketStream<S>,
    invitation: &mut ActiveInvitation,
    now: u64,
    committer: &mut C,
    approve: A,
) -> Result<(), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: InvitationCommitter,
    A: FnOnce(JoinCandidate) -> F,
    F: Future<Output = bool>,
{
    require_frame_limit(socket)?;
    let deadline = Instant::now() + JOIN_HANDSHAKE_TIMEOUT;
    if let Err(error) = send_before(
        socket,
        deadline,
        &JoinFrame::Offer {
            version: JOIN_PROTOCOL_VERSION,
            offer: invitation.offer().clone(),
        },
    )
    .await
    {
        record_failure(invitation, now);
        return Err(error);
    }

    let hello = match receive_before(socket, deadline).await {
        Ok(JoinFrame::JoinerHello { hello, .. }) => hello,
        Ok(JoinFrame::Error { code, .. }) => {
            record_failure(invitation, now);
            return Err(JoinTransportError::Remote(code));
        }
        Ok(_) => return reject(socket, deadline, JoinErrorCode::OutOfOrder, invitation, now).await,
        Err(error) => {
            record_failure(invitation, now);
            let _ = send_error(socket, deadline, code_for_error(&error)).await;
            return Err(error);
        }
    };

    let attempt = match invitation.begin_attempt(now, &hello) {
        Ok(attempt) => attempt,
        Err(error) => {
            // Malformed PAKE input is recorded inside `begin_attempt`; context
            // failures are recorded here. Recording can itself reveal that the
            // invitation is already exhausted, which is still a rejection.
            if !matches!(error, PairingError::Pake) {
                record_failure(invitation, now);
            }
            let _ = send_error(socket, deadline, JoinErrorCode::InvitationRejected).await;
            return Err(error.into());
        }
    };
    if let Err(error) = send_before(
        socket,
        deadline,
        &JoinFrame::InviterConfirmation {
            version: JOIN_PROTOCOL_VERSION,
            confirmation: attempt.response().clone(),
        },
    )
    .await
    {
        record_failure(invitation, now);
        return Err(error);
    }

    let confirmation = match receive_before(socket, deadline).await {
        Ok(JoinFrame::JoinerConfirmation { confirmation, .. }) => confirmation,
        Ok(JoinFrame::Error { code, .. }) => {
            record_failure(invitation, now);
            return Err(JoinTransportError::Remote(code));
        }
        Ok(_) => return reject(socket, deadline, JoinErrorCode::OutOfOrder, invitation, now).await,
        Err(error) => {
            record_failure(invitation, now);
            let _ = send_error(socket, deadline, code_for_error(&error)).await;
            return Err(error);
        }
    };

    let trust = match invitation.confirm_attempt(now, attempt, &confirmation) {
        Ok(trust) => trust,
        Err(error) => {
            if matches!(error, PairingError::ContextMismatch) {
                record_failure(invitation, now);
            }
            let _ = send_error(socket, deadline, JoinErrorCode::PairingRejected).await;
            return Err(error.into());
        }
    };

    let candidate = JoinCandidate {
        joiner_id: trust.joiner_id().to_owned(),
        profile: trust.joiner_profile().cloned(),
    };
    let approved = match timeout(JOIN_APPROVAL_TIMEOUT, approve(candidate)).await {
        Ok(approved) => approved,
        Err(_) => {
            let _ = send_error(
                socket,
                Instant::now() + JOIN_IO_TIMEOUT,
                JoinErrorCode::ApprovalTimedOut,
            )
            .await;
            return Err(JoinTransportError::ApprovalTimeout);
        }
    };
    if !approved {
        let _ = send_error(
            socket,
            Instant::now() + JOIN_IO_TIMEOUT,
            JoinErrorCode::ApprovalRejected,
        )
        .await;
        return Err(JoinTransportError::ApprovalRejected);
    }

    // `confirm_attempt` has now consumed the invitation. If this transaction
    // fails it remains consumed, so the same invitation cannot be replayed.
    let bundle = timeout(
        JOIN_IO_TIMEOUT,
        committer.add_paired_member_and_seal(&trust),
    )
    .await
    .map_err(|_| JoinTransportError::IoTimeout)
    .and_then(|result| result.map_err(|error| JoinTransportError::Commit(error.to_string())));
    let bundle = match bundle {
        Ok(bundle) => bundle,
        Err(error) => {
            let _ = send_error(
                socket,
                Instant::now() + JOIN_IO_TIMEOUT,
                JoinErrorCode::CommitFailed,
            )
            .await;
            return Err(error);
        }
    };
    send_before(
        socket,
        Instant::now() + JOIN_IO_TIMEOUT,
        &JoinFrame::MembershipBundle {
            version: JOIN_PROTOCOL_VERSION,
            bundle,
        },
    )
    .await
}

/// Join using a signing key and caller-held short code. The short code is
/// consumed only by `JoinerPairing::start` and is never serialized.
pub async fn join_invitation<S>(
    socket: &mut WebSocketStream<S>,
    offer: &InvitationOffer,
    joiner_key: &SigningKey,
    short_code: &[u8],
) -> Result<(PairingTrust, SealedMembershipBundle), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    validate_offer(offer).map_err(JoinTransportError::Pairing)?;
    let (pairing, hello) = JoinerPairing::start(offer, joiner_key, short_code);
    join_invitation_prepared(socket, offer, pairing, hello).await
}

/// Fetch an active offer from a discovered device, then run PAKE using the
/// caller-held short code. This matches the camera-free nearby-device flow.
pub async fn join_discovered_invitation<S>(
    socket: &mut WebSocketStream<S>,
    joiner_key: &SigningKey,
    short_code: &[u8],
) -> Result<(InvitationOffer, PairingTrust, SealedMembershipBundle), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    require_frame_limit(socket)?;
    let deadline = Instant::now() + JOIN_HANDSHAKE_TIMEOUT;
    let offer = match receive_before(socket, deadline).await? {
        JoinFrame::Offer { offer, .. } => offer,
        JoinFrame::Error { code, .. } => return Err(JoinTransportError::Remote(code)),
        _ => return Err(JoinTransportError::OutOfOrder),
    };
    let (pairing, hello) = JoinerPairing::start(&offer, joiner_key, short_code);
    let (trust, bundle) = join_after_offer(socket, deadline, pairing, hello).await?;
    Ok((offer, trust, bundle))
}

pub async fn join_discovered_invitation_for_profile<S>(
    socket: &mut WebSocketStream<S>,
    profile: MemberProfile,
    identity: &DeviceIdentity,
    short_code: &[u8],
) -> Result<(InvitationOffer, PairingTrust, SealedMembershipBundle), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    validate_profile_input(&profile).map_err(JoinTransportError::Pairing)?;
    require_frame_limit(socket)?;
    let deadline = Instant::now() + JOIN_HANDSHAKE_TIMEOUT;
    let offer = match receive_before(socket, deadline).await? {
        JoinFrame::Offer { offer, .. } => offer,
        JoinFrame::Error { code, .. } => return Err(JoinTransportError::Remote(code)),
        _ => return Err(JoinTransportError::OutOfOrder),
    };
    let (pairing, hello) = JoinerPairing::start_with_profile(
        &offer,
        identity.signing_key(),
        short_code,
        Some(profile),
    );
    let (trust, bundle) = join_after_offer(socket, deadline, pairing, hello).await?;
    Ok((offer, trust, bundle))
}

pub async fn join_discovered_invitation_for_identity<S>(
    socket: &mut WebSocketStream<S>,
    identity: &DeviceIdentity,
    short_code: &[u8],
) -> Result<(InvitationOffer, PairingTrust, SealedMembershipBundle), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    join_discovered_invitation(socket, identity.signing_key(), short_code).await
}

/// Join using PAKE state prepared by the caller. The matching hello is required
/// because `JoinerPairing` intentionally does not expose its secret state.
pub async fn join_invitation_prepared<S>(
    socket: &mut WebSocketStream<S>,
    expected_offer: &InvitationOffer,
    pairing: JoinerPairing,
    hello: JoinerHello,
) -> Result<(PairingTrust, SealedMembershipBundle), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    require_frame_limit(socket)?;
    let deadline = Instant::now() + JOIN_HANDSHAKE_TIMEOUT;
    match receive_before(socket, deadline).await? {
        JoinFrame::Offer { offer, .. } if &offer == expected_offer => {}
        JoinFrame::Offer { .. } => return Err(JoinTransportError::OutOfOrder),
        JoinFrame::Error { code, .. } => return Err(JoinTransportError::Remote(code)),
        _ => return Err(JoinTransportError::OutOfOrder),
    }
    join_after_offer(socket, deadline, pairing, hello).await
}

async fn join_after_offer<S>(
    socket: &mut WebSocketStream<S>,
    deadline: Instant,
    pairing: JoinerPairing,
    hello: JoinerHello,
) -> Result<(PairingTrust, SealedMembershipBundle), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_before(
        socket,
        deadline,
        &JoinFrame::JoinerHello {
            version: JOIN_PROTOCOL_VERSION,
            hello,
        },
    )
    .await?;

    let response = match receive_before(socket, deadline).await? {
        JoinFrame::InviterConfirmation { confirmation, .. } => confirmation,
        JoinFrame::Error { code, .. } => return Err(JoinTransportError::Remote(code)),
        _ => return Err(JoinTransportError::OutOfOrder),
    };
    let (confirmation, trust) = match pairing.finish(&response) {
        Ok(confirmed) => confirmed,
        Err(error) => {
            let _ = send_error(socket, deadline, JoinErrorCode::PairingRejected).await;
            return Err(error.into());
        }
    };
    send_before(
        socket,
        deadline,
        &JoinFrame::JoinerConfirmation {
            version: JOIN_PROTOCOL_VERSION,
            confirmation,
        },
    )
    .await?;

    // The cryptographic exchange is complete here. Waiting for a person to
    // compare and approve the authenticated device must not inherit the
    // ordinary 10-second socket-idle timeout used by machine-only frames.
    let approval_deadline = Instant::now() + JOIN_APPROVAL_TIMEOUT + JOIN_IO_TIMEOUT;
    let bundle = match receive_before_with_io_timeout(
        socket,
        approval_deadline,
        JOIN_APPROVAL_TIMEOUT + JOIN_IO_TIMEOUT,
    )
    .await?
    {
        JoinFrame::MembershipBundle { bundle, .. } => bundle,
        JoinFrame::Error { code, .. } => return Err(JoinTransportError::Remote(code)),
        _ => return Err(JoinTransportError::OutOfOrder),
    };
    // Authenticate the ciphertext and its complete public context before
    // handing it to persistence code. The caller still decides whether/where
    // to persist the returned trust and sealed bundle.
    trust.open_bundle(&bundle)?;
    Ok((trust, bundle))
}

fn record_failure(invitation: &mut ActiveInvitation, now: u64) {
    let _ = invitation.record_failed(now);
}

fn require_frame_limit<S>(socket: &WebSocketStream<S>) -> Result<(), JoinTransportError> {
    let config = socket.get_config();
    if config
        .max_message_size
        .is_some_and(|limit| limit <= JOIN_MAX_FRAME)
        && config
            .max_frame_size
            .is_some_and(|limit| limit <= JOIN_MAX_FRAME)
    {
        Ok(())
    } else {
        Err(JoinTransportError::UnsafeWebSocketConfig)
    }
}

async fn reject<S>(
    socket: &mut WebSocketStream<S>,
    deadline: Instant,
    code: JoinErrorCode,
    invitation: &mut ActiveInvitation,
    now: u64,
) -> Result<(), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    record_failure(invitation, now);
    let _ = send_error(socket, deadline, code).await;
    Err(JoinTransportError::OutOfOrder)
}

fn code_for_error(error: &JoinTransportError) -> JoinErrorCode {
    match error {
        JoinTransportError::UnsupportedVersion => JoinErrorCode::UnsupportedVersion,
        JoinTransportError::OutOfOrder => JoinErrorCode::OutOfOrder,
        _ => JoinErrorCode::MalformedFrame,
    }
}

async fn send_error<S>(
    socket: &mut WebSocketStream<S>,
    deadline: Instant,
    code: JoinErrorCode,
) -> Result<(), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send_before(
        socket,
        deadline,
        &JoinFrame::Error {
            version: JOIN_PROTOCOL_VERSION,
            code,
        },
    )
    .await
}

async fn send_before<S>(
    socket: &mut WebSocketStream<S>,
    deadline: Instant,
    frame: &JoinFrame,
) -> Result<(), JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let encoded = serde_json::to_vec(frame).map_err(|_| JoinTransportError::MalformedFrame)?;
    if encoded.len() > JOIN_MAX_FRAME {
        return Err(JoinTransportError::Oversized);
    }
    let send = timeout(
        JOIN_IO_TIMEOUT,
        socket.send(Message::Binary(encoded.into())),
    );
    timeout_at(deadline, send)
        .await
        .map_err(|_| JoinTransportError::HandshakeTimeout)?
        .map_err(|_| JoinTransportError::IoTimeout)??;
    Ok(())
}

async fn receive_before<S>(
    socket: &mut WebSocketStream<S>,
    deadline: Instant,
) -> Result<JoinFrame, JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    receive_before_with_io_timeout(socket, deadline, JOIN_IO_TIMEOUT).await
}

async fn receive_before_with_io_timeout<S>(
    socket: &mut WebSocketStream<S>,
    deadline: Instant,
    io_timeout: Duration,
) -> Result<JoinFrame, JoinTransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let receive = timeout(io_timeout, socket.next());
    let message = timeout_at(deadline, receive)
        .await
        .map_err(|_| JoinTransportError::HandshakeTimeout)?
        .map_err(|_| JoinTransportError::IoTimeout)?
        .ok_or(JoinTransportError::Closed)??;
    let bytes = match message {
        Message::Binary(bytes) if bytes.len() <= JOIN_MAX_FRAME => bytes,
        Message::Binary(_) => return Err(JoinTransportError::Oversized),
        Message::Close(_) => return Err(JoinTransportError::Closed),
        _ => return Err(JoinTransportError::MalformedFrame),
    };
    let frame: JoinFrame =
        serde_json::from_slice(&bytes).map_err(|_| JoinTransportError::MalformedFrame)?;
    let valid = match &frame {
        JoinFrame::Offer { offer, .. } => validate_offer(offer),
        JoinFrame::JoinerHello { hello, .. } => validate_joiner_hello(hello),
        JoinFrame::InviterConfirmation { confirmation, .. } => {
            validate_inviter_confirmation(confirmation)
        }
        JoinFrame::JoinerConfirmation { confirmation, .. } => {
            validate_joiner_confirmation(confirmation)
        }
        JoinFrame::MembershipBundle { bundle, .. } => validate_envelope(bundle),
        JoinFrame::Error { .. } => Ok(()),
    };
    if valid.is_err() {
        return Err(JoinTransportError::MalformedFrame);
    }
    if frame.version() != JOIN_PROTOCOL_VERSION {
        return Err(JoinTransportError::UnsupportedVersion);
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::{GroupMembership, MembershipBundle};
    use nekodex_domain::invitation::InvitationPolicy;
    use nekodex_domain::signing::member_id_for_key;
    use tokio::io::DuplexStream;
    use tokio_tungstenite::tungstenite::protocol::Role;

    fn key(byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[byte; 32])
    }

    fn policy() -> InvitationPolicy {
        InvitationPolicy {
            max_failures: 3,
            window_limit: 2,
            window: 10,
        }
    }

    async fn sockets() -> (WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
        let (client, server) = tokio::io::duplex(128 * 1024);
        tokio::join!(
            WebSocketStream::from_raw_socket(client, Role::Client, Some(join_websocket_config()),),
            WebSocketStream::from_raw_socket(server, Role::Server, Some(join_websocket_config()),),
        )
    }

    #[derive(Debug, thiserror::Error)]
    #[error("commit failed")]
    struct CommitError;

    struct TestCommitter {
        membership: GroupMembership,
        key: SigningKey,
        commits: usize,
    }

    #[async_trait]
    impl InvitationCommitter for TestCommitter {
        type Error = CommitError;

        async fn add_paired_member_and_seal(
            &mut self,
            trust: &PairingTrust,
        ) -> Result<SealedMembershipBundle, Self::Error> {
            self.commits += 1;
            let mut candidate = self.membership.clone();
            candidate
                .add_paired_member(trust, "add-joiner", &self.key)
                .map_err(|_| CommitError)?;
            let sealed = trust
                .seal_bundle(&candidate.export_bundle())
                .map_err(|_| CommitError)?;
            self.membership = candidate;
            Ok(sealed)
        }
    }

    #[tokio::test]
    async fn successful_join_returns_authenticated_sealed_current_bundle() {
        let inviter_key = key(1);
        let joiner_key = key(2);
        let membership = GroupMembership::create("group", "genesis", &inviter_key).unwrap();
        let (mut invitation, short_code) = membership
            .create_invitation("invite", 100, 200, policy())
            .unwrap();
        let offer = invitation.offer().clone();
        let mut committer = TestCommitter {
            membership,
            key: inviter_key,
            commits: 0,
        };
        let (mut client, mut server) = sockets().await;
        let expected_joiner_id = member_id_for_key(&joiner_key.verifying_key());

        let (server_result, client_result) = tokio::join!(
            serve_invitation_with_approval(
                &mut server,
                &mut invitation,
                100,
                &mut committer,
                |candidate| async move {
                    assert_eq!(candidate.joiner_id, expected_joiner_id);
                    assert_eq!(candidate.profile, None);
                    true
                },
            ),
            join_invitation(&mut client, &offer, &joiner_key, short_code.as_bytes()),
        );
        server_result.unwrap();
        let (trust, sealed) = client_result.unwrap();
        let bundle: MembershipBundle = trust.open_bundle(&sealed).unwrap();
        assert_eq!(bundle, committer.membership.export_bundle());
        assert!(committer.membership.is_member(trust.joiner_id()));
        assert_eq!(committer.commits, 1);

        let wire = serde_json::to_string(&JoinFrame::MembershipBundle {
            version: JOIN_PROTOCOL_VERSION,
            bundle: sealed,
        })
        .unwrap();
        assert!(!wire.contains(short_code.as_str()));
        assert!(!wire.contains("session_key"));
        assert!(!wire.contains("genesis"));
    }

    #[tokio::test]
    async fn rejected_authenticated_candidate_never_runs_committer() {
        let inviter_key = key(1);
        let joiner_key = key(2);
        let membership = GroupMembership::create("group", "genesis", &inviter_key).unwrap();
        let (mut invitation, short_code) = membership
            .create_invitation("invite", 100, 200, policy())
            .unwrap();
        let offer = invitation.offer().clone();
        let joiner_id = member_id_for_key(&joiner_key.verifying_key());
        let rejected_joiner_id = joiner_id.clone();
        let profile = MemberProfile {
            device_id: joiner_id.clone(),
            name: "phone".into(),
            signing_public_key: joiner_key.verifying_key().to_bytes(),
            noise_public_key: [9; 32],
            codex_capable: false,
            protocol_version: 1,
        };
        let (pairing, hello) = JoinerPairing::start_with_profile(
            &offer,
            &joiner_key,
            short_code.as_bytes(),
            Some(profile.clone()),
        );
        let mut committer = TestCommitter {
            membership,
            key: inviter_key,
            commits: 0,
        };
        let (mut client, mut server) = sockets().await;

        let (server_result, client_result) = tokio::join!(
            serve_invitation_with_approval(
                &mut server,
                &mut invitation,
                100,
                &mut committer,
                |candidate| async move {
                    assert_eq!(candidate.joiner_id, joiner_id);
                    assert_eq!(candidate.profile, Some(profile));
                    false
                },
            ),
            join_invitation_prepared(&mut client, &offer, pairing, hello),
        );

        assert!(matches!(
            server_result,
            Err(JoinTransportError::ApprovalRejected)
        ));
        assert!(matches!(
            client_result,
            Err(JoinTransportError::Remote(JoinErrorCode::ApprovalRejected))
        ));
        assert_eq!(committer.commits, 0);
        assert!(!committer.membership.is_member(&rejected_joiner_id));
    }

    #[tokio::test]
    async fn server_rejects_out_of_order_frame_with_stable_error_code() {
        let inviter_key = key(1);
        let membership = GroupMembership::create("group", "genesis", &inviter_key).unwrap();
        let (mut invitation, _) = membership
            .create_invitation("invite", 100, 200, policy())
            .unwrap();
        let mut committer = TestCommitter {
            membership,
            key: inviter_key,
            commits: 0,
        };
        let (mut client, mut server) = sockets().await;

        let server_task =
            async { serve_invitation(&mut server, &mut invitation, 100, &mut committer).await };
        let client_task = async {
            assert!(matches!(
                receive_before(&mut client, Instant::now() + JOIN_HANDSHAKE_TIMEOUT).await,
                Ok(JoinFrame::Offer { .. })
            ));
            send_before(
                &mut client,
                Instant::now() + JOIN_HANDSHAKE_TIMEOUT,
                &JoinFrame::JoinerConfirmation {
                    version: JOIN_PROTOCOL_VERSION,
                    confirmation: JoinerConfirmation {
                        invitation_id: "invite".into(),
                        confirmation: [0; 32],
                        profile: None,
                        profile_proof: None,
                    },
                },
            )
            .await
            .unwrap();
            receive_before(&mut client, Instant::now() + JOIN_HANDSHAKE_TIMEOUT).await
        };
        let (server_result, client_result) = tokio::join!(server_task, client_task);
        assert!(matches!(server_result, Err(JoinTransportError::OutOfOrder)));
        assert!(matches!(
            client_result,
            Ok(JoinFrame::Error {
                code: JoinErrorCode::OutOfOrder,
                ..
            })
        ));
    }
}
