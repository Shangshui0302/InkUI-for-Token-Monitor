//! Device-group creation, PAKE invitation orchestration, log synchronization,
//! and the membership gate used before opening an application connection.
//!
//! Short codes are held only by non-serializable local state. Every wire type
//! contains PAKE output or public membership data, never the password supplied
//! to `PairingContext`.

use std::collections::{BTreeMap, BTreeSet};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use nekodex_domain::{
    EventState, GroupCreated, MemberAdded, MemberProfile, MemberRenamed, MemberRevoked,
    MembershipEvent, MembershipLog,
    checkpoint::{CheckpointError, MembershipCheckpoint},
    invitation::{
        Invitation, InvitationConfigError, InvitationError, InvitationId, InvitationPolicy,
        PakeAttempt,
    },
    pairing::{PairingContext, PairingHandshake, PairingResult},
    signing::{SignedMembershipEvent, member_id_for_key},
};
use rand::{
    RngCore,
    distributions::{Distribution, Uniform},
    rngs::OsRng,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const SHORT_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
const SHORT_CODE_LENGTH: usize = 8;
const BOOTSTRAP_AAD_DOMAIN: &[u8] = b"nekodex/membership-bootstrap/xchacha20poly1305/v1";
const PROFILE_PROOF_DOMAIN: &[u8] = b"nekodex/joiner-profile-proof/ed25519/v1";

/// The membership payload is encoded as a JSON array of bytes inside the
/// transport envelope.  Keeping its JSON representation at 12 KiB leaves
/// ample room below the existing 64 KiB WebSocket frame after that wrapper,
/// Noise overhead, and future envelope fields are accounted for.
pub const MAX_MEMBERSHIP_BUNDLE_BYTES: usize = 12 * 1024;
pub const MAX_MEMBERSHIP_EVENTS: usize = 256;
pub const MAX_MEMBERSHIP_IDENTIFIER_BYTES: usize = 128;
pub const MAX_MEMBERSHIP_PARENTS: usize = 64;
pub const MAX_MEMBER_DISPLAY_NAME_CHARS: usize = 128;
pub const MAX_MEMBER_DISPLAY_NAME_BYTES: usize = 512;
/// Bounds for values carried by the pairing handshake and authenticated
/// envelope. These are deliberately much smaller than the WebSocket frame
/// limit so hostile values cannot reach PAKE, signature, or persistence code.
pub const MAX_PAIRING_MESSAGE_BYTES: usize = 4 * 1024;
const AEAD_TAG_BYTES: usize = 16;

/// A display-only secret. It cannot be serialized or put in a wire message.
pub struct ShortCode([u8; SHORT_CODE_LENGTH]);

impl ShortCode {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("short-code alphabet is ASCII")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvitationOffer {
    pub invitation_id: String,
    pub group_id: String,
    pub group_name: String,
    pub inviter_id: String,
    pub inviter_nonce: [u8; 32],
    pub expires_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinerHello {
    pub invitation_id: String,
    pub joiner_id: String,
    pub joiner_nonce: [u8; 32],
    pub pake_message: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviterConfirmation {
    pub invitation_id: String,
    pub pake_message: Vec<u8>,
    pub confirmation: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinerConfirmation {
    pub invitation_id: String,
    pub confirmation: [u8; 32],
    #[serde(default)]
    pub profile: Option<MemberProfile>,
    #[serde(default)]
    pub profile_proof: Option<Vec<u8>>,
}

/// A complete authenticated membership state. Deltas are intentionally not
/// accepted because they cannot prove that checkpointed history is present.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipBundle {
    pub group_id: String,
    pub events: Vec<SignedMembershipEvent>,
    pub checkpoint: MembershipCheckpoint,
}

/// Encrypted bootstrap transfer. Its public context is authenticated as AAD;
/// neither the PAKE session key nor the short code is present on the wire.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedMembershipBundle {
    pub invitation_id: String,
    pub group_id: String,
    pub inviter_id: String,
    pub joiner_id: String,
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

/// Opaque, non-serializable evidence of successful mutual PAKE confirmation.
pub struct PairingTrust {
    invitation_id: String,
    group_id: String,
    inviter_id: String,
    joiner_id: String,
    session_key: [u8; 32],
    joiner_profile: Option<MemberProfile>,
}

impl PairingTrust {
    pub fn invitation_id(&self) -> &str {
        &self.invitation_id
    }

    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    pub fn inviter_id(&self) -> &str {
        &self.inviter_id
    }

    pub fn joiner_id(&self) -> &str {
        &self.joiner_id
    }

    pub fn joiner_profile(&self) -> Option<&MemberProfile> {
        self.joiner_profile.as_ref()
    }

    /// Serialize and encrypt the initial full log with the PAKE-derived key.
    pub fn seal_bundle(
        &self,
        bundle: &MembershipBundle,
    ) -> Result<SealedMembershipBundle, PairingError> {
        validate_identifier(&self.invitation_id).map_err(|_| PairingError::InputTooLong)?;
        validate_identifier(&self.group_id).map_err(|_| PairingError::InputTooLong)?;
        validate_identifier(&self.inviter_id).map_err(|_| PairingError::InputTooLong)?;
        validate_identifier(&self.joiner_id).map_err(|_| PairingError::InputTooLong)?;
        if bundle.group_id != self.group_id {
            return Err(PairingError::ContextMismatch);
        }
        validate_bundle(bundle).map_err(|_| PairingError::BootstrapSeal)?;
        let plaintext = serde_json::to_vec(bundle).map_err(|_| PairingError::BootstrapSeal)?;
        let mut nonce = [0; 24];
        OsRng.fill_bytes(&mut nonce);
        let mut envelope = SealedMembershipBundle {
            invitation_id: self.invitation_id.clone(),
            group_id: self.group_id.clone(),
            inviter_id: self.inviter_id.clone(),
            joiner_id: self.joiner_id.clone(),
            nonce,
            ciphertext: Vec::new(),
        };
        let aad = bootstrap_aad(&envelope);
        let cipher = XChaCha20Poly1305::new_from_slice(&self.session_key)
            .expect("SPAKE2 session key has the required AEAD key length");
        envelope.ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&envelope.nonce),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| PairingError::BootstrapSeal)?;
        Ok(envelope)
    }

    /// Authenticate and decrypt a bootstrap transfer. No API accepts a raw key
    /// for this operation, keeping the pairing context attached to decryption.
    pub fn open_bundle(
        &self,
        envelope: &SealedMembershipBundle,
    ) -> Result<MembershipBundle, PairingError> {
        validate_envelope(envelope)?;
        if envelope.invitation_id != self.invitation_id
            || envelope.group_id != self.group_id
            || envelope.inviter_id != self.inviter_id
            || envelope.joiner_id != self.joiner_id
        {
            return Err(PairingError::BootstrapRejected);
        }
        if envelope.ciphertext.len() > MAX_MEMBERSHIP_BUNDLE_BYTES + AEAD_TAG_BYTES {
            return Err(PairingError::BootstrapRejected);
        }
        let aad = bootstrap_aad(envelope);
        let cipher = XChaCha20Poly1305::new_from_slice(&self.session_key)
            .expect("SPAKE2 session key has the required AEAD key length");
        let plaintext = cipher
            .decrypt(
                XNonce::from_slice(&envelope.nonce),
                Payload {
                    msg: &envelope.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| PairingError::BootstrapRejected)?;
        let bundle: MembershipBundle =
            serde_json::from_slice(&plaintext).map_err(|_| PairingError::BootstrapRejected)?;
        if bundle.group_id != self.group_id || validate_bundle(&bundle).is_err() {
            return Err(PairingError::BootstrapRejected);
        }
        Ok(bundle)
    }
}

/// Capability produced only after all membership gates pass.
#[derive(Debug, Eq, PartialEq)]
pub struct MembershipAuthorization {
    group_id: String,
    peer_id: String,
}

impl MembershipAuthorization {
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum MembershipError {
    #[error("group or event identifier is empty")]
    EmptyIdentifier,
    #[error("local signing key does not match the local member identity")]
    WrongSigningKey,
    #[error("membership bundle belongs to a different group")]
    GroupMismatch,
    #[error("membership bundle contains duplicate event ids")]
    DuplicateEvent,
    #[error("membership event id has conflicting signed contents")]
    ConflictingEvent,
    #[error("invalid signed membership event: {0}")]
    InvalidSignedEvent(&'static str),
    #[error("membership event is not causally valid")]
    InvalidEvent,
    #[error("membership checkpoint failed validation: {0:?}")]
    Checkpoint(CheckpointError),
    #[error("bundle does not exactly match its checkpoint")]
    IncompleteBundle,
    #[error("pairing trust does not match this operation")]
    PairingTrustMismatch,
    #[error("peer is not an active member")]
    UnauthorizedPeer,
    #[error("member has been revoked and cannot be restored")]
    PermanentlyRevoked,
    #[error("membership bundle exceeds the byte limit")]
    BundleTooLarge,
    #[error("membership bundle contains too many events")]
    TooManyEvents,
    #[error("membership identifier exceeds the byte limit")]
    IdentifierTooLong,
    #[error("membership event contains too many parents")]
    TooManyParents,
    #[error("membership parent identifier exceeds the byte limit")]
    ParentIdentifierTooLong,
    #[error("membership display name exceeds the limit")]
    DisplayNameTooLong,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PairingError {
    #[error("invitation configuration is invalid: {0:?}")]
    InvalidInvitation(InvitationConfigError),
    #[error("invitation attempt rejected: {0:?}")]
    Invitation(InvitationError),
    #[error("pairing message does not match the invitation")]
    ContextMismatch,
    #[error("SPAKE2 exchange failed")]
    Pake,
    #[error("peer key confirmation failed")]
    ConfirmationFailed,
    #[error("joiner profile proof failed")]
    ProfileProof,
    #[error("membership bootstrap bundle could not be encrypted")]
    BootstrapSeal,
    #[error("membership bootstrap envelope was rejected")]
    BootstrapRejected,
    #[error("pairing input exceeds its length limit")]
    InputTooLong,
}

/// Local invitation state. It holds the short code and cannot be serialized.
pub struct ActiveInvitation {
    invitation: Invitation,
    offer: InvitationOffer,
    short_code: [u8; SHORT_CODE_LENGTH],
}

impl ActiveInvitation {
    pub fn offer(&self) -> &InvitationOffer {
        &self.offer
    }

    pub fn state(
        &self,
        now: u64,
    ) -> Result<nekodex_domain::invitation::InvitationState, InvitationError> {
        self.invitation.state(now)
    }

    /// Checks rate limits before doing the intentionally expensive PAKE work.
    pub fn begin_attempt(
        &mut self,
        now: u64,
        hello: &JoinerHello,
    ) -> Result<InviterPairingAttempt, PairingError> {
        validate_offer(&self.offer)?;
        validate_joiner_hello(hello)?;
        self.invitation
            .can_attempt(now)
            .map_err(PairingError::Invitation)?;
        if hello.invitation_id != self.offer.invitation_id || hello.joiner_id.is_empty() {
            return Err(PairingError::ContextMismatch);
        }
        let context = pairing_context(&self.offer, &hello.joiner_id, hello.joiner_nonce);
        let (handshake, message) = context.start_inviter(&self.short_code);
        let result = match handshake.finish(&hello.pake_message) {
            Ok(result) => result,
            Err(_) => {
                self.record_failed(now)?;
                return Err(PairingError::Pake);
            }
        };
        Ok(InviterPairingAttempt {
            invitation_id: self.offer.invitation_id.clone(),
            group_id: self.offer.group_id.clone(),
            inviter_id: self.offer.inviter_id.clone(),
            inviter_nonce: self.offer.inviter_nonce,
            joiner_id: hello.joiner_id.clone(),
            response: InviterConfirmation {
                invitation_id: self.offer.invitation_id.clone(),
                pake_message: message,
                confirmation: result.confirmation,
            },
            result,
        })
    }

    pub fn record_failed(&mut self, now: u64) -> Result<(), PairingError> {
        self.invitation
            .record(now, PakeAttempt::Failed)
            .map(|_| ())
            .map_err(PairingError::Invitation)
    }

    /// Consumes the one-time invitation only after validating the joiner.
    pub fn confirm_attempt(
        &mut self,
        now: u64,
        attempt: InviterPairingAttempt,
        peer: &JoinerConfirmation,
    ) -> Result<PairingTrust, PairingError> {
        validate_offer(&self.offer)?;
        validate_joiner_confirmation(peer)?;
        if attempt.invitation_id != self.offer.invitation_id
            || attempt.inviter_nonce != self.offer.inviter_nonce
            || peer.invitation_id != self.offer.invitation_id
        {
            return Err(PairingError::ContextMismatch);
        }
        if !PairingHandshake::verify_peer(&attempt.result, &peer.confirmation, b"joiner") {
            self.record_failed(now)?;
            return Err(PairingError::ConfirmationFailed);
        }
        if let Some(profile) = &peer.profile {
            let proof = peer
                .profile_proof
                .as_deref()
                .ok_or(PairingError::ProfileProof)?;
            verify_profile(
                profile,
                &attempt.joiner_id,
                &profile_proof_bytes(&peer.invitation_id, &peer.confirmation, profile),
                proof,
            )
            .map_err(|_| PairingError::ProfileProof)?;
        } else if peer.profile_proof.is_some() {
            return Err(PairingError::ProfileProof);
        }
        self.invitation
            .record(now, PakeAttempt::Succeeded)
            .map_err(PairingError::Invitation)?;
        Ok(attempt.into_trust(peer.profile.clone()))
    }
}

pub struct InviterPairingAttempt {
    invitation_id: String,
    group_id: String,
    inviter_id: String,
    inviter_nonce: [u8; 32],
    joiner_id: String,
    response: InviterConfirmation,
    result: PairingResult,
}

impl InviterPairingAttempt {
    pub fn response(&self) -> &InviterConfirmation {
        &self.response
    }

    fn into_trust(self, joiner_profile: Option<MemberProfile>) -> PairingTrust {
        PairingTrust {
            invitation_id: self.invitation_id,
            group_id: self.group_id,
            inviter_id: self.inviter_id,
            joiner_id: self.joiner_id,
            session_key: self.result.session_key,
            joiner_profile,
        }
    }
}

pub struct JoinerPairing {
    invitation_id: String,
    group_id: String,
    inviter_id: String,
    joiner_id: String,
    handshake: PairingHandshake,
    profile: Option<MemberProfile>,
    joiner_key: SigningKey,
}

impl JoinerPairing {
    pub fn start(
        offer: &InvitationOffer,
        joiner_key: &SigningKey,
        short_code: &[u8],
    ) -> (Self, JoinerHello) {
        Self::start_with_profile(offer, joiner_key, short_code, None)
    }

    pub fn start_with_profile(
        offer: &InvitationOffer,
        joiner_key: &SigningKey,
        short_code: &[u8],
        profile: Option<MemberProfile>,
    ) -> (Self, JoinerHello) {
        let joiner_id = member_id_for_key(&joiner_key.verifying_key());
        let mut joiner_nonce = [0; 32];
        OsRng.fill_bytes(&mut joiner_nonce);
        let context = pairing_context(offer, &joiner_id, joiner_nonce);
        let (handshake, pake_message) = context.start_joiner(short_code);
        (
            Self {
                invitation_id: offer.invitation_id.clone(),
                group_id: offer.group_id.clone(),
                inviter_id: offer.inviter_id.clone(),
                joiner_id: joiner_id.clone(),
                handshake,
                profile,
                joiner_key: joiner_key.clone(),
            },
            JoinerHello {
                invitation_id: offer.invitation_id.clone(),
                joiner_id,
                joiner_nonce,
                pake_message,
            },
        )
    }

    /// A wrong code returns an error and produces no message to confirm.
    pub fn finish(
        self,
        response: &InviterConfirmation,
    ) -> Result<(JoinerConfirmation, PairingTrust), PairingError> {
        validate_identifier(&self.invitation_id).map_err(|_| PairingError::InputTooLong)?;
        if let Some(profile) = self.profile.as_ref() {
            validate_profile_shape(Some(profile)).map_err(|_| PairingError::InputTooLong)?;
        }
        validate_inviter_confirmation(response)?;
        if response.invitation_id != self.invitation_id {
            return Err(PairingError::ContextMismatch);
        }
        let result = self
            .handshake
            .finish(&response.pake_message)
            .map_err(|_| PairingError::Pake)?;
        if !PairingHandshake::verify_peer(&result, &response.confirmation, b"inviter") {
            return Err(PairingError::ConfirmationFailed);
        }
        Ok((
            JoinerConfirmation {
                invitation_id: self.invitation_id.clone(),
                confirmation: result.confirmation,
                profile: self.profile.clone(),
                profile_proof: self.profile.as_ref().map(|profile| {
                    self.joiner_key
                        .sign(&profile_proof_bytes(
                            &self.invitation_id,
                            &result.confirmation,
                            profile,
                        ))
                        .to_bytes()
                        .to_vec()
                }),
            },
            PairingTrust {
                invitation_id: self.invitation_id,
                group_id: self.group_id,
                inviter_id: self.inviter_id,
                joiner_id: self.joiner_id,
                session_key: result.session_key,
                joiner_profile: self.profile,
            },
        ))
    }
}

/// Locally confirmed group state. It originates only from locally signed
/// genesis or from successful pairing plus a verified complete bundle.
#[derive(Clone)]
pub struct GroupMembership {
    group_id: String,
    local_member_id: String,
    log: MembershipLog,
    signed_events: BTreeMap<String, SignedMembershipEvent>,
    checkpoint: MembershipCheckpoint,
    revoked_ever: BTreeSet<String>,
}

impl GroupMembership {
    pub fn create(
        group_id: impl Into<String>,
        genesis_event_id: impl Into<String>,
        local_key: &SigningKey,
    ) -> Result<Self, MembershipError> {
        Self::create_with_profile(group_id, genesis_event_id, None, local_key)
    }

    pub fn create_with_profile(
        group_id: impl Into<String>,
        genesis_event_id: impl Into<String>,
        profile: Option<MemberProfile>,
        local_key: &SigningKey,
    ) -> Result<Self, MembershipError> {
        let group_id = group_id.into();
        let event_id = genesis_event_id.into();
        if group_id.is_empty() || event_id.is_empty() {
            return Err(MembershipError::EmptyIdentifier);
        }
        let local_member_id = member_id_for_key(&local_key.verifying_key());
        let genesis = SignedMembershipEvent::sign(
            &group_id,
            MembershipEvent::GroupCreated(GroupCreated {
                event_id,
                group_id: group_id.clone(),
                member_id: local_member_id.clone(),
                profile,
            }),
            local_key,
        )
        .map_err(MembershipError::InvalidSignedEvent)?;
        Self::from_validated(&group_id, &local_member_id, vec![genesis])
    }

    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    pub fn local_member_id(&self) -> &str {
        &self.local_member_id
    }

    pub fn checkpoint(&self) -> &MembershipCheckpoint {
        &self.checkpoint
    }

    pub fn is_member(&self, member_id: &str) -> bool {
        !self.revoked_ever.contains(member_id) && self.log.is_member(member_id)
    }

    pub fn active_member_ids(&self) -> Vec<String> {
        let candidates: BTreeSet<_> = self
            .signed_events
            .values()
            .filter_map(|signed| match &signed.event {
                MembershipEvent::GroupCreated(event) => Some(event.member_id.clone()),
                MembershipEvent::Added(event) => Some(event.member_id.clone()),
                MembershipEvent::Revoked(_) => None,
                MembershipEvent::Renamed(_) => None,
            })
            .collect();
        candidates
            .into_iter()
            .filter(|member_id| self.is_member(member_id))
            .collect()
    }

    /// Profiles are sourced exclusively from authenticated membership events.
    /// Missing legacy profiles are intentionally omitted, never filled from discovery.
    pub fn active_member_profiles(&self) -> Vec<MemberProfile> {
        let mut profiles: BTreeMap<String, MemberProfile> = self
            .signed_events
            .values()
            .filter_map(|signed| match &signed.event {
                MembershipEvent::GroupCreated(e) if self.is_member(&e.member_id) => {
                    e.profile.clone()
                }
                MembershipEvent::Added(e) if self.is_member(&e.member_id) => e.profile.clone(),
                _ => None,
            })
            .map(|profile| (profile.device_id.clone(), profile))
            .collect();
        let mut renames: Vec<_> = self
            .signed_events
            .values()
            .filter_map(|signed| match &signed.event {
                MembershipEvent::Renamed(event) if self.is_member(&event.member_id) => Some((
                    event_depth(&self.signed_events, &event.event_id),
                    &event.event_id,
                    event,
                )),
                _ => None,
            })
            .collect();
        renames.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        for (_, _, rename) in renames {
            if let Some(profile) = profiles.get_mut(&rename.member_id) {
                profile.name.clone_from(&rename.name);
            }
        }
        profiles.into_values().collect()
    }

    pub fn create_invitation(
        &self,
        invitation_id: impl Into<String>,
        now: u64,
        expires_at: u64,
        policy: InvitationPolicy,
    ) -> Result<(ActiveInvitation, ShortCode), PairingError> {
        self.create_named_invitation(invitation_id, self.group_id.clone(), now, expires_at, policy)
    }

    pub fn create_named_invitation(
        &self,
        invitation_id: impl Into<String>,
        group_name: impl Into<String>,
        now: u64,
        expires_at: u64,
        policy: InvitationPolicy,
    ) -> Result<(ActiveInvitation, ShortCode), PairingError> {
        let invitation_id =
            InvitationId::new(invitation_id).map_err(PairingError::InvalidInvitation)?;
        let mut inviter_nonce = [0; 32];
        OsRng.fill_bytes(&mut inviter_nonce);
        let mut code = [0; SHORT_CODE_LENGTH];
        let alphabet_index = Uniform::from(0..SHORT_CODE_ALPHABET.len());
        let mut rng = OsRng;
        for output in &mut code {
            *output = SHORT_CODE_ALPHABET[alphabet_index.sample(&mut rng)];
        }
        let invitation = Invitation::new(invitation_id.clone(), now, expires_at, policy)
            .map_err(PairingError::InvalidInvitation)?;
        let offer = InvitationOffer {
            invitation_id: invitation_id.as_str().to_owned(),
            group_id: self.group_id.clone(),
            group_name: group_name.into(),
            inviter_id: self.local_member_id.clone(),
            inviter_nonce,
            expires_at,
        };
        validate_offer(&offer)?;
        Ok((
            ActiveInvitation {
                invitation,
                offer,
                short_code: code,
            },
            ShortCode(code),
        ))
    }

    /// The inviter signs `MemberAdded` only after mutual confirmation.
    pub fn add_paired_member(
        &mut self,
        trust: &PairingTrust,
        event_id: impl Into<String>,
        local_key: &SigningKey,
    ) -> Result<SignedMembershipEvent, MembershipError> {
        self.add_paired_member_with_profile(trust, event_id, None, local_key)
    }

    pub fn add_paired_member_with_profile(
        &mut self,
        trust: &PairingTrust,
        event_id: impl Into<String>,
        profile: Option<MemberProfile>,
        local_key: &SigningKey,
    ) -> Result<SignedMembershipEvent, MembershipError> {
        self.check_local_key(local_key)?;
        if !self.is_member(&self.local_member_id) {
            return Err(MembershipError::UnauthorizedPeer);
        }
        if trust.group_id != self.group_id || trust.inviter_id != self.local_member_id {
            return Err(MembershipError::PairingTrustMismatch);
        }
        if self.revoked_ever.contains(&trust.joiner_id) {
            return Err(MembershipError::PermanentlyRevoked);
        }
        if let Some(profile) = &profile {
            if profile.device_id != trust.joiner_id {
                return Err(MembershipError::PairingTrustMismatch);
            }
            let key = ed25519_dalek::VerifyingKey::from_bytes(&profile.signing_public_key)
                .map_err(|_| MembershipError::PairingTrustMismatch)?;
            if member_id_for_key(&key) != profile.device_id {
                return Err(MembershipError::PairingTrustMismatch);
            }
        }
        let event_id = event_id.into();
        if event_id.is_empty() {
            return Err(MembershipError::EmptyIdentifier);
        }
        let signed = SignedMembershipEvent::sign(
            &self.group_id,
            MembershipEvent::Added(MemberAdded {
                event_id,
                member_id: trust.joiner_id.clone(),
                issuer_id: self.local_member_id.clone(),
                parents: self.log.authenticated_frontier(),
                profile,
            }),
            local_key,
        )
        .map_err(MembershipError::InvalidSignedEvent)?;
        self.insert_local(signed.clone())?;
        Ok(signed)
    }

    pub fn revoke_member(
        &mut self,
        member_id: impl Into<String>,
        event_id: impl Into<String>,
        local_key: &SigningKey,
    ) -> Result<SignedMembershipEvent, MembershipError> {
        self.check_local_key(local_key)?;
        if !self.is_member(&self.local_member_id) {
            return Err(MembershipError::UnauthorizedPeer);
        }
        let member_id = member_id.into();
        let event_id = event_id.into();
        if member_id.is_empty() || event_id.is_empty() {
            return Err(MembershipError::EmptyIdentifier);
        }
        let signed = SignedMembershipEvent::sign(
            &self.group_id,
            MembershipEvent::Revoked(MemberRevoked {
                event_id,
                member_id,
                issuer_id: self.local_member_id.clone(),
                parents: self.log.authenticated_frontier(),
            }),
            local_key,
        )
        .map_err(MembershipError::InvalidSignedEvent)?;
        self.insert_local(signed.clone())?;
        Ok(signed)
    }

    pub fn rename_local(
        &mut self,
        name: impl Into<String>,
        event_id: impl Into<String>,
        local_key: &SigningKey,
    ) -> Result<SignedMembershipEvent, MembershipError> {
        self.check_local_key(local_key)?;
        if !self.is_member(&self.local_member_id) {
            return Err(MembershipError::UnauthorizedPeer);
        }
        let name = name.into().trim().to_owned();
        let event_id = event_id.into();
        if name.is_empty() || event_id.is_empty() {
            return Err(MembershipError::EmptyIdentifier);
        }
        validate_display_name(&name)?;
        let signed = SignedMembershipEvent::sign(
            &self.group_id,
            MembershipEvent::Renamed(MemberRenamed {
                event_id,
                member_id: self.local_member_id.clone(),
                issuer_id: self.local_member_id.clone(),
                parents: self.log.authenticated_frontier(),
                name,
            }),
            local_key,
        )
        .map_err(MembershipError::InvalidSignedEvent)?;
        self.insert_local(signed.clone())?;
        Ok(signed)
    }

    pub fn export_bundle(&self) -> MembershipBundle {
        MembershipBundle {
            group_id: self.group_id.clone(),
            events: self.signed_events.values().cloned().collect(),
            checkpoint: self.checkpoint.clone(),
        }
    }

    /// Restore a previously confirmed local group. The persisted checkpoint is
    /// the trust root; a self-consistent but rolled-back bundle is insufficient.
    pub fn restore(
        local_member_id: impl Into<String>,
        persisted_checkpoint: MembershipCheckpoint,
        bundle: MembershipBundle,
    ) -> Result<Self, MembershipError> {
        let local_member_id = local_member_id.into();
        Self::from_bundle(&bundle, &local_member_id, Some(&persisted_checkpoint))
    }

    /// Establish a local trust root after pairing. The full bundle must contain
    /// both the inviter and joiner as active members.
    pub fn join_from_pairing(
        trust: PairingTrust,
        bundle: MembershipBundle,
        local_key: &SigningKey,
    ) -> Result<Self, MembershipError> {
        let local_id = member_id_for_key(&local_key.verifying_key());
        if trust.group_id != bundle.group_id || trust.joiner_id != local_id {
            return Err(MembershipError::PairingTrustMismatch);
        }
        let joined = Self::from_bundle(&bundle, &local_id, None)?;
        if !joined.is_member(&trust.inviter_id)
            || !joined.is_member(&local_id)
            || !joined.signed_events.values().any(|signed| {
                matches!(
                    &signed.event,
                    MembershipEvent::Added(event)
                        if event.member_id == local_id && event.issuer_id == trust.inviter_id
                )
            })
        {
            return Err(MembershipError::UnauthorizedPeer);
        }
        Ok(joined)
    }

    /// Import only a complete state that passes this device's anti-rollback
    /// checkpoint. Validation finishes before `self` is changed.
    pub fn synchronize(&mut self, bundle: &MembershipBundle) -> Result<(), MembershipError> {
        let candidate = Self::from_bundle(bundle, &self.local_member_id, Some(&self.checkpoint))?;
        *self = candidate;
        Ok(())
    }

    /// Merge a complete, independently verified peer bundle with local state.
    /// The union keeps every locally checkpointed event, while validation of
    /// the combined causal graph prevents a stale or conflicting peer from
    /// rolling local membership backward.
    pub fn merge_verified(&mut self, bundle: &MembershipBundle) -> Result<(), MembershipError> {
        if bundle.group_id != self.group_id {
            return Err(MembershipError::GroupMismatch);
        }
        let remote = Self::from_bundle(bundle, &self.local_member_id, None)?;
        let mut events = self.signed_events.clone();
        for (event_id, signed) in remote.signed_events {
            match events.get(&event_id) {
                Some(existing) if existing != &signed => {
                    return Err(MembershipError::ConflictingEvent);
                }
                Some(_) => {}
                None => {
                    events.insert(event_id, signed);
                }
            }
        }
        let candidate = Self::from_validated(
            &self.group_id,
            &self.local_member_id,
            events.into_values().collect(),
        )?;
        self.checkpoint
            .verify_signed_events(&self.group_id, candidate.signed_events.values())
            .map_err(MembershipError::Checkpoint)?;
        *self = candidate;
        Ok(())
    }

    /// Verify checkpoint, signatures, full causal log, and active membership
    /// before returning the business-traffic capability.
    pub fn authorize_peer(
        &self,
        peer_id: &str,
        bundle: &MembershipBundle,
    ) -> Result<MembershipAuthorization, MembershipError> {
        if self.revoked_ever.contains(peer_id) {
            return Err(MembershipError::PermanentlyRevoked);
        }
        let candidate = Self::from_bundle(bundle, &self.local_member_id, Some(&self.checkpoint))?;
        if !candidate.is_member(peer_id) {
            return Err(if candidate.revoked_ever.contains(peer_id) {
                MembershipError::PermanentlyRevoked
            } else {
                MembershipError::UnauthorizedPeer
            });
        }
        Ok(MembershipAuthorization {
            group_id: self.group_id.clone(),
            peer_id: peer_id.to_owned(),
        })
    }

    fn from_bundle(
        bundle: &MembershipBundle,
        local_member_id: &str,
        prior_checkpoint: Option<&MembershipCheckpoint>,
    ) -> Result<Self, MembershipError> {
        validate_bundle(bundle)?;
        if let Some(checkpoint) = prior_checkpoint {
            if checkpoint.group_id != bundle.group_id {
                return Err(MembershipError::GroupMismatch);
            }
        }
        if let Some(checkpoint) = prior_checkpoint {
            checkpoint
                .verify_signed_events(&bundle.group_id, bundle.events.iter())
                .map_err(MembershipError::Checkpoint)?;
        }
        let candidate =
            Self::from_validated(&bundle.group_id, local_member_id, bundle.events.clone())?;
        if candidate.checkpoint != bundle.checkpoint {
            return Err(MembershipError::IncompleteBundle);
        }
        Ok(candidate)
    }

    fn from_validated(
        group_id: &str,
        local_member_id: &str,
        events: Vec<SignedMembershipEvent>,
    ) -> Result<Self, MembershipError> {
        validate_identifier(group_id)?;
        validate_identifier(local_member_id)?;
        let mut log = MembershipLog::default();
        let mut signed_events = BTreeMap::new();
        for signed in events {
            validate_signed_event_shape(&signed.event)?;
            match &signed.event {
                MembershipEvent::GroupCreated(event) => {
                    if let Some(profile) = &event.profile {
                        validate_profile_identity(profile, &event.member_id).map_err(|_| {
                            MembershipError::InvalidSignedEvent("invalid member profile")
                        })?;
                    }
                }
                MembershipEvent::Added(event) => {
                    if let Some(profile) = &event.profile {
                        validate_profile_identity(profile, &event.member_id).map_err(|_| {
                            MembershipError::InvalidSignedEvent("invalid member profile")
                        })?;
                    }
                }
                MembershipEvent::Revoked(_) => {}
                MembershipEvent::Renamed(event) => {
                    if event.member_id != event.issuer_id
                        || event.name.trim().is_empty()
                        || event.name.chars().count() > 128
                    {
                        return Err(if event.name.chars().count() > MAX_MEMBER_DISPLAY_NAME_CHARS
                            || event.name.len() > MAX_MEMBER_DISPLAY_NAME_BYTES
                        {
                            MembershipError::DisplayNameTooLong
                        } else {
                            MembershipError::InvalidSignedEvent("invalid member display name")
                        });
                    }
                }
            }
            let event_id = signed
                .verify(group_id)
                .map_err(MembershipError::InvalidSignedEvent)?
                .event_id()
                .to_owned();
            if signed_events.contains_key(&event_id) {
                return Err(MembershipError::DuplicateEvent);
            }
            log.insert_signed(group_id, signed.clone())
                .map_err(MembershipError::InvalidSignedEvent)?;
            signed_events.insert(event_id, signed);
        }
        if signed_events.is_empty()
            || signed_events
                .keys()
                .any(|event_id| log.event_state(event_id) != Some(EventState::Valid))
        {
            return Err(MembershipError::InvalidEvent);
        }
        let checkpoint = log
            .checkpoint(group_id)
            .map_err(MembershipError::Checkpoint)?;
        let revoked_ever = signed_events
            .values()
            .filter_map(|signed| match &signed.event {
                MembershipEvent::Revoked(event) => Some(event.member_id.clone()),
                _ => None,
            })
            .collect();
        let membership = Self {
            group_id: group_id.to_owned(),
            local_member_id: local_member_id.to_owned(),
            log,
            signed_events,
            checkpoint,
            revoked_ever,
        };
        validate_bundle(&membership.export_bundle())?;
        Ok(membership)
    }

    fn insert_local(&mut self, signed: SignedMembershipEvent) -> Result<(), MembershipError> {
        validate_signed_event_shape(&signed.event)?;
        let previous = self.clone();
        let event_id = signed.event.event_id().to_owned();
        if self.signed_events.contains_key(&event_id) {
            return Err(MembershipError::DuplicateEvent);
        }
        self.log
            .insert_signed(&self.group_id, signed.clone())
            .map_err(MembershipError::InvalidSignedEvent)?;
        if self.log.event_state(&event_id) != Some(EventState::Valid) {
            return Err(MembershipError::InvalidEvent);
        }
        if let MembershipEvent::Revoked(event) = &signed.event {
            self.revoked_ever.insert(event.member_id.clone());
        }
        self.signed_events.insert(event_id, signed);
        self.checkpoint = self
            .log
            .checkpoint(&self.group_id)
            .map_err(MembershipError::Checkpoint)?;
        if let Err(error) = validate_bundle(&self.export_bundle()) {
            *self = previous;
            return Err(error);
        }
        Ok(())
    }

    fn check_local_key(&self, key: &SigningKey) -> Result<(), MembershipError> {
        if member_id_for_key(&key.verifying_key()) == self.local_member_id {
            Ok(())
        } else {
            Err(MembershipError::WrongSigningKey)
        }
    }
}

fn event_depth(events: &BTreeMap<String, SignedMembershipEvent>, event_id: &str) -> usize {
    fn visit(
        events: &BTreeMap<String, SignedMembershipEvent>,
        event_id: &str,
        memo: &mut BTreeMap<String, usize>,
    ) -> usize {
        if let Some(depth) = memo.get(event_id) {
            return *depth;
        }
        let depth = events
            .get(event_id)
            .and_then(|signed| match &signed.event {
                MembershipEvent::GroupCreated(_) => None,
                MembershipEvent::Added(event) => Some(&event.parents),
                MembershipEvent::Revoked(event) => Some(&event.parents),
                MembershipEvent::Renamed(event) => Some(&event.parents),
            })
            .map(|parents| {
                1 + parents
                    .iter()
                    .map(|parent| visit(events, parent, memo))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        memo.insert(event_id.to_owned(), depth);
        depth
    }
    visit(events, event_id, &mut BTreeMap::new())
}

pub(crate) fn validate_bundle(bundle: &MembershipBundle) -> Result<(), MembershipError> {
    validate_identifier(&bundle.group_id)?;
    if bundle.events.len() > MAX_MEMBERSHIP_EVENTS
        || bundle.checkpoint.event_ids.len() > MAX_MEMBERSHIP_EVENTS
    {
        return Err(MembershipError::TooManyEvents);
    }
    if bundle.checkpoint.group_id != bundle.group_id {
        return Err(MembershipError::GroupMismatch);
    }
    for event_id in &bundle.checkpoint.event_ids {
        validate_identifier(event_id)?;
    }
    for signed in &bundle.events {
        validate_signed_event_shape(&signed.event)?;
    }
    let encoded = serde_json::to_vec(bundle).map_err(|_| MembershipError::BundleTooLarge)?;
    if encoded.len() > MAX_MEMBERSHIP_BUNDLE_BYTES {
        return Err(MembershipError::BundleTooLarge);
    }
    Ok(())
}

fn validate_signed_event_shape(event: &MembershipEvent) -> Result<(), MembershipError> {
    validate_identifier(event.event_id())?;
    match event {
        MembershipEvent::GroupCreated(value) => {
            validate_identifier(&value.group_id)?;
            validate_identifier(&value.member_id)?;
            validate_profile_shape(value.profile.as_ref())?;
        }
        MembershipEvent::Added(value) => {
            validate_identifier(&value.member_id)?;
            validate_identifier(&value.issuer_id)?;
            validate_parents(&value.parents)?;
            validate_profile_shape(value.profile.as_ref())?;
        }
        MembershipEvent::Revoked(value) => {
            validate_identifier(&value.member_id)?;
            validate_identifier(&value.issuer_id)?;
            validate_parents(&value.parents)?;
        }
        MembershipEvent::Renamed(value) => {
            validate_identifier(&value.member_id)?;
            validate_identifier(&value.issuer_id)?;
            validate_parents(&value.parents)?;
            validate_display_name(&value.name)?;
        }
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<(), MembershipError> {
    if value.is_empty() {
        return Err(MembershipError::EmptyIdentifier);
    }
    if value.len() > MAX_MEMBERSHIP_IDENTIFIER_BYTES {
        return Err(MembershipError::IdentifierTooLong);
    }
    Ok(())
}

fn validate_parents(parents: &BTreeSet<String>) -> Result<(), MembershipError> {
    if parents.len() > MAX_MEMBERSHIP_PARENTS {
        return Err(MembershipError::TooManyParents);
    }
    for parent in parents {
        if parent.is_empty() {
            return Err(MembershipError::EmptyIdentifier);
        }
        if parent.len() > MAX_MEMBERSHIP_IDENTIFIER_BYTES {
            return Err(MembershipError::ParentIdentifierTooLong);
        }
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<(), MembershipError> {
    if name.chars().count() > MAX_MEMBER_DISPLAY_NAME_CHARS
        || name.len() > MAX_MEMBER_DISPLAY_NAME_BYTES
    {
        return Err(MembershipError::DisplayNameTooLong);
    }
    Ok(())
}

fn validate_profile_shape(profile: Option<&MemberProfile>) -> Result<(), MembershipError> {
    if let Some(profile) = profile {
        validate_identifier(&profile.device_id)?;
        validate_display_name(&profile.name)?;
    }
    Ok(())
}

pub(crate) fn validate_offer(offer: &InvitationOffer) -> Result<(), PairingError> {
    validate_identifier(&offer.invitation_id).map_err(|_| PairingError::InputTooLong)?;
    validate_identifier(&offer.group_id).map_err(|_| PairingError::InputTooLong)?;
    validate_display_name(&offer.group_name).map_err(|_| PairingError::InputTooLong)?;
    validate_identifier(&offer.inviter_id).map_err(|_| PairingError::InputTooLong)?;
    Ok(())
}

pub(crate) fn validate_joiner_hello(hello: &JoinerHello) -> Result<(), PairingError> {
    validate_identifier(&hello.invitation_id).map_err(|_| PairingError::InputTooLong)?;
    validate_identifier(&hello.joiner_id).map_err(|_| PairingError::InputTooLong)?;
    if hello.pake_message.len() > MAX_PAIRING_MESSAGE_BYTES {
        return Err(PairingError::InputTooLong);
    }
    Ok(())
}

pub(crate) fn validate_inviter_confirmation(
    confirmation: &InviterConfirmation,
) -> Result<(), PairingError> {
    validate_identifier(&confirmation.invitation_id).map_err(|_| PairingError::InputTooLong)?;
    if confirmation.pake_message.len() > MAX_PAIRING_MESSAGE_BYTES {
        return Err(PairingError::InputTooLong);
    }
    Ok(())
}

pub(crate) fn validate_joiner_confirmation(
    confirmation: &JoinerConfirmation,
) -> Result<(), PairingError> {
    validate_identifier(&confirmation.invitation_id).map_err(|_| PairingError::InputTooLong)?;
    if let Some(profile) = confirmation.profile.as_ref() {
        validate_profile_shape(Some(profile)).map_err(|_| PairingError::InputTooLong)?;
    }
    if confirmation.profile_proof.as_ref().is_some_and(|proof| proof.len() > 128) {
        return Err(PairingError::InputTooLong);
    }
    Ok(())
}

pub(crate) fn validate_envelope(envelope: &SealedMembershipBundle) -> Result<(), PairingError> {
    for value in [
        &envelope.invitation_id,
        &envelope.group_id,
        &envelope.inviter_id,
        &envelope.joiner_id,
    ] {
        validate_identifier(value).map_err(|_| PairingError::InputTooLong)?;
    }
    if envelope.ciphertext.len() > MAX_MEMBERSHIP_BUNDLE_BYTES + AEAD_TAG_BYTES {
        return Err(PairingError::BootstrapRejected);
    }
    Ok(())
}

pub(crate) fn validate_profile_input(profile: &MemberProfile) -> Result<(), PairingError> {
    validate_profile_shape(Some(profile)).map_err(|_| PairingError::InputTooLong)
}

fn pairing_context(
    offer: &InvitationOffer,
    joiner_id: &str,
    joiner_nonce: [u8; 32],
) -> PairingContext {
    PairingContext {
        group_id: offer.group_id.as_bytes().to_vec(),
        group_name: offer.group_name.as_bytes().to_vec(),
        inviter_id: offer.inviter_id.as_bytes().to_vec(),
        joiner_id: joiner_id.as_bytes().to_vec(),
        inviter_nonce: offer.inviter_nonce,
        joiner_nonce,
    }
}

fn profile_proof_bytes(
    invitation_id: &str,
    confirmation: &[u8; 32],
    profile: &MemberProfile,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    field(&mut bytes, PROFILE_PROOF_DOMAIN);
    field(&mut bytes, invitation_id.as_bytes());
    bytes.extend_from_slice(confirmation);
    field(&mut bytes, profile.device_id.as_bytes());
    field(&mut bytes, profile.name.as_bytes());
    bytes.extend_from_slice(&profile.signing_public_key);
    bytes.extend_from_slice(&profile.noise_public_key);
    bytes.push(profile.codex_capable as u8);
    bytes.extend_from_slice(&profile.protocol_version.to_be_bytes());
    bytes
}

fn verify_profile(
    profile: &MemberProfile,
    expected_id: &str,
    message: &[u8],
    proof: &[u8],
) -> Result<(), ()> {
    let key = validate_profile_identity(profile, expected_id)?;
    let signature = Signature::from_slice(proof).map_err(|_| ())?;
    key.verify_strict(message, &signature).map_err(|_| ())
}

fn validate_profile_identity(
    profile: &MemberProfile,
    expected_id: &str,
) -> Result<VerifyingKey, ()> {
    let key = VerifyingKey::from_bytes(&profile.signing_public_key).map_err(|_| ())?;
    if profile.device_id != expected_id || member_id_for_key(&key) != profile.device_id {
        return Err(());
    }
    Ok(key)
}

fn field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn bootstrap_aad(envelope: &SealedMembershipBundle) -> Vec<u8> {
    let mut aad = Vec::new();
    aad_field(&mut aad, BOOTSTRAP_AAD_DOMAIN);
    aad_field(&mut aad, envelope.invitation_id.as_bytes());
    aad_field(&mut aad, envelope.group_id.as_bytes());
    aad_field(&mut aad, envelope.inviter_id.as_bytes());
    aad_field(&mut aad, envelope.joiner_id.as_bytes());
    aad
}

fn aad_field(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn pair_and_add(
        inviter: &mut GroupMembership,
        inviter_key: &SigningKey,
        joiner_key: &SigningKey,
        invitation_id: &str,
        event_id: &str,
    ) -> GroupMembership {
        let (inviter_trust, joiner_trust) = pairing_trusts(inviter, joiner_key, invitation_id);
        inviter
            .add_paired_member(&inviter_trust, event_id, inviter_key)
            .unwrap();
        let sealed = inviter_trust.seal_bundle(&inviter.export_bundle()).unwrap();
        let wire = serde_json::to_string(&sealed).unwrap();
        assert!(!wire.contains("session_key"));
        assert!(!wire.contains("short_code"));
        let received = joiner_trust.open_bundle(&sealed).unwrap();
        GroupMembership::join_from_pairing(joiner_trust, received, joiner_key).unwrap()
    }

    fn pairing_trusts(
        inviter: &GroupMembership,
        joiner_key: &SigningKey,
        invitation_id: &str,
    ) -> (PairingTrust, PairingTrust) {
        let (mut invitation, code) = inviter
            .create_invitation(invitation_id, 100, 200, policy())
            .unwrap();
        let offer_json = serde_json::to_string(invitation.offer()).unwrap();
        assert!(!offer_json.contains(code.as_str()));
        let (joiner, hello) = JoinerPairing::start(invitation.offer(), joiner_key, code.as_bytes());
        let attempt = invitation.begin_attempt(100, &hello).unwrap();
        let response = attempt.response().clone();
        let (confirmation, joiner_trust) = joiner.finish(&response).unwrap();
        let inviter_trust = invitation
            .confirm_attempt(100, attempt, &confirmation)
            .unwrap();
        (inviter_trust, joiner_trust)
    }

    #[test]
    fn profile_proof_rejects_tampering_after_pake() {
        let inviter_key = key(31);
        let joiner_key = key(32);
        let inviter = GroupMembership::create("group", "root", &inviter_key).unwrap();
        let (mut invitation, code) = inviter
            .create_invitation("invite", 1, 10, policy())
            .unwrap();
        let joiner_id = member_id_for_key(&joiner_key.verifying_key());
        let profile = MemberProfile {
            device_id: joiner_id,
            name: "joiner".into(),
            signing_public_key: joiner_key.verifying_key().to_bytes(),
            noise_public_key: [9; 32],
            codex_capable: true,
            protocol_version: 1,
        };
        let (pairing, hello) = JoinerPairing::start_with_profile(
            invitation.offer(),
            &joiner_key,
            code.as_bytes(),
            Some(profile),
        );
        let attempt = invitation.begin_attempt(1, &hello).unwrap();
        let (mut confirmation, _) = pairing.finish(attempt.response()).unwrap();
        confirmation.profile.as_mut().unwrap().name = "tampered".into();
        assert!(matches!(
            invitation.confirm_attempt(1, attempt, &confirmation),
            Err(PairingError::ProfileProof)
        ));
    }

    #[test]
    fn signed_rename_propagates_without_changing_member_identity() {
        let local_key = key(33);
        let member_id = member_id_for_key(&local_key.verifying_key());
        let profile = MemberProfile {
            device_id: member_id.clone(),
            name: "old name".into(),
            signing_public_key: local_key.verifying_key().to_bytes(),
            noise_public_key: [7; 32],
            codex_capable: true,
            protocol_version: 1,
        };
        let mut local =
            GroupMembership::create_with_profile("group", "root", Some(profile), &local_key)
                .unwrap();
        local
            .rename_local("new name", "rename-1", &local_key)
            .unwrap();

        let restored = GroupMembership::restore(
            member_id.clone(),
            local.checkpoint().clone(),
            local.export_bundle(),
        )
        .unwrap();
        let renamed = restored
            .active_member_profiles()
            .into_iter()
            .find(|member| member.device_id == member_id)
            .unwrap();
        assert_eq!(renamed.name, "new name");
        assert!(renamed.codex_capable);
        assert_eq!(renamed.noise_public_key, [7; 32]);
    }

    #[test]
    fn b_can_invite_c_and_they_authorize_each_other_with_a_offline() {
        let a_key = key(1);
        let b_key = key(2);
        let c_key = key(3);
        let mut a = GroupMembership::create("group", "genesis", &a_key).unwrap();
        let mut b = pair_and_add(&mut a, &a_key, &b_key, "a-to-b", "add-b");
        let mut c = pair_and_add(&mut b, &b_key, &c_key, "b-to-c", "add-c");

        // A is not consulted from here onward.
        b.synchronize(&c.export_bundle()).unwrap();
        c.synchronize(&b.export_bundle()).unwrap();
        let b_auth = b
            .authorize_peer(c.local_member_id(), &c.export_bundle())
            .unwrap();
        let c_auth = c
            .authorize_peer(b.local_member_id(), &b.export_bundle())
            .unwrap();
        assert_eq!(b_auth.peer_id(), c.local_member_id());
        assert_eq!(c_auth.peer_id(), b.local_member_id());
    }

    #[test]
    fn stale_and_new_complete_bundles_merge_without_rollback() {
        let a_key = key(1);
        let b_key = key(2);
        let c_key = key(3);
        let mut a = GroupMembership::create("group", "genesis", &a_key).unwrap();
        let mut b = pair_and_add(&mut a, &a_key, &b_key, "a-to-b", "add-b");
        let mut c = pair_and_add(&mut b, &b_key, &c_key, "b-to-c", "add-c");
        let stale_a = a.export_bundle();

        a.merge_verified(&c.export_bundle()).unwrap();
        c.merge_verified(&stale_a).unwrap();

        assert!(a.is_member(c.local_member_id()));
        assert!(c.is_member(a.local_member_id()));
        assert_eq!(a.checkpoint(), c.checkpoint());
    }

    #[test]
    fn unauthorized_peer_is_rejected_before_business_authorization() {
        let a_key = key(1);
        let outsider = member_id_for_key(&key(9).verifying_key());
        let a = GroupMembership::create("group", "genesis", &a_key).unwrap();
        assert_eq!(
            a.authorize_peer(&outsider, &a.export_bundle()),
            Err(MembershipError::UnauthorizedPeer)
        );
    }

    #[test]
    fn revocation_propagates_and_remains_permanent() {
        let a_key = key(1);
        let b_key = key(2);
        let mut a = GroupMembership::create("group", "genesis", &a_key).unwrap();
        let mut b = pair_and_add(&mut a, &a_key, &b_key, "a-to-b", "add-b");
        let b_id = b.local_member_id().to_owned();
        a.revoke_member(&b_id, "revoke-b", &a_key).unwrap();
        b.synchronize(&a.export_bundle()).unwrap();
        assert!(!b.is_member(&b_id));
        assert_eq!(
            b.authorize_peer(&b_id, &a.export_bundle()),
            Err(MembershipError::PermanentlyRevoked)
        );
    }

    #[test]
    fn remote_log_missing_a_checkpoint_event_is_rejected() {
        let a_key = key(1);
        let b_key = key(2);
        let mut a = GroupMembership::create("group", "genesis", &a_key).unwrap();
        let b = pair_and_add(&mut a, &a_key, &b_key, "a-to-b", "add-b");
        let truncated = MembershipBundle {
            group_id: "group".into(),
            events: vec![a.signed_events["genesis"].clone()],
            checkpoint: MembershipCheckpoint::from_event_ids("group", ["genesis"]).unwrap(),
        };
        assert!(matches!(
            b.authorize_peer(a.local_member_id(), &truncated),
            Err(MembershipError::Checkpoint(CheckpointError::MissingEvents(
                _
            )))
        ));
    }

    #[test]
    fn legal_identifier_boundary_fits_the_bundle_budget() {
        let membership = GroupMembership::create(
            "g".repeat(MAX_MEMBERSHIP_IDENTIFIER_BYTES),
            "e".repeat(MAX_MEMBERSHIP_IDENTIFIER_BYTES),
            &key(1),
        )
        .unwrap();
        let bundle = membership.export_bundle();
        assert!(serde_json::to_vec(&bundle).unwrap().len() <= MAX_MEMBERSHIP_BUNDLE_BYTES);
    }

    #[test]
    fn rename_history_is_bounded_without_mutating_on_rejection() {
        let mut membership = GroupMembership::create("group", "genesis", &key(1)).unwrap();
        let mut rejected = None;
        for index in 0..MAX_MEMBERSHIP_EVENTS {
            let before = membership.export_bundle();
            if let Err(error) = membership.rename_local(
                format!("rename-{index}"),
                format!("rename-{index}"),
                &key(1),
            ) {
                assert_eq!(membership.export_bundle(), before);
                rejected = Some(error);
                break;
            }
        }
        assert!(matches!(
            rejected,
            Some(
                MembershipError::BundleTooLarge
                    | MembershipError::TooManyEvents
                    | MembershipError::TooManyParents
            )
        ));
        assert!(serde_json::to_vec(&membership.export_bundle())
            .unwrap()
            .len()
            <= MAX_MEMBERSHIP_BUNDLE_BYTES);
    }

    #[test]
    fn oversized_sync_bundle_is_rejected_before_adoption() {
        let mut membership = GroupMembership::create("group", "genesis", &key(1)).unwrap();
        let mut oversized = membership.export_bundle();
        oversized.events = vec![oversized.events[0].clone(); MAX_MEMBERSHIP_EVENTS + 1];
        assert_eq!(
            membership.synchronize(&oversized),
            Err(MembershipError::TooManyEvents)
        );
        assert_eq!(membership.export_bundle().events.len(), 1);
    }

    #[test]
    fn bootstrap_bundle_rejects_tampering_wrong_aad_and_another_pairing() {
        let a_key = key(1);
        let b_key = key(2);
        let mut a = GroupMembership::create("group", "genesis", &a_key).unwrap();
        let (inviter_trust, joiner_trust) = pairing_trusts(&a, &b_key, "a-to-b");
        a.add_paired_member(&inviter_trust, "add-b", &a_key)
            .unwrap();
        let bundle = a.export_bundle();
        let sealed = inviter_trust.seal_bundle(&bundle).unwrap();
        assert_eq!(joiner_trust.open_bundle(&sealed), Ok(bundle));
        let wire = serde_json::to_string(&sealed).unwrap();
        assert!(!wire.contains("genesis"));
        assert!(!wire.contains("session_key"));

        let mut tampered = sealed.clone();
        tampered.ciphertext[0] ^= 1;
        assert_eq!(
            joiner_trust.open_bundle(&tampered),
            Err(PairingError::BootstrapRejected)
        );

        let mut wrong_aad = sealed.clone();
        wrong_aad.invitation_id.push_str("-altered");
        assert_eq!(
            joiner_trust.open_bundle(&wrong_aad),
            Err(PairingError::BootstrapRejected)
        );

        // Even an independently confirmed exchange with identical public AAD
        // has a different PAKE session key and cannot open this ciphertext.
        let (_other_inviter, other_joiner) = pairing_trusts(&a, &b_key, "a-to-b");
        assert_eq!(
            other_joiner.open_bundle(&sealed),
            Err(PairingError::BootstrapRejected)
        );
    }

    #[test]
    fn wrong_short_code_never_yields_a_joiner_confirmation() {
        let a_key = key(1);
        let b_key = key(2);
        let a = GroupMembership::create("group", "genesis", &a_key).unwrap();
        let (mut invitation, _correct_code) =
            a.create_invitation("a-to-b", 100, 200, policy()).unwrap();
        let (joiner, hello) = JoinerPairing::start(invitation.offer(), &b_key, b"WRONG999");
        let attempt = invitation.begin_attempt(100, &hello).unwrap();
        assert!(matches!(
            joiner.finish(attempt.response()),
            Err(PairingError::ConfirmationFailed)
        ));
        invitation.record_failed(100).unwrap();
        assert_eq!(
            invitation.state(100),
            Ok(nekodex_domain::invitation::InvitationState::Active)
        );
    }

    #[test]
    fn oversized_handshake_inputs_are_rejected_before_crypto_or_persistence() {
        let inviter = GroupMembership::create("group", "genesis", &key(1)).unwrap();
        assert!(matches!(
            inviter.create_named_invitation(
                "invite", "x".repeat(MAX_MEMBER_DISPLAY_NAME_BYTES + 1), 1, 2, policy()
            ),
            Err(PairingError::InputTooLong)
        ));

        let (mut invitation, code) = inviter.create_invitation("invite", 1, 2, policy()).unwrap();
        let (_, mut hello) = JoinerPairing::start(invitation.offer(), &key(2), code.as_bytes());
        hello.pake_message = vec![0; MAX_PAIRING_MESSAGE_BYTES + 1];
        assert!(matches!(
            invitation.begin_attempt(1, &hello),
            Err(PairingError::InputTooLong)
        ));

        let profile = MemberProfile {
            device_id: "id".into(),
            name: "n".repeat(MAX_MEMBER_DISPLAY_NAME_BYTES + 1),
            signing_public_key: [0; 32],
            noise_public_key: [0; 32],
            codex_capable: false,
            protocol_version: 1,
        };
        assert_eq!(validate_profile_input(&profile), Err(PairingError::InputTooLong));
    }
}
