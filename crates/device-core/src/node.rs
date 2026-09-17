//! Durable composition of identity, membership, storage, and transport gates.

use std::sync::{Arc, RwLock as SyncRwLock};

use async_trait::async_trait;
use ed25519_dalek::VerifyingKey;
use nekodex_domain::signing::member_id_for_key;
use tokio::sync::RwLock;

use crate::identity::DeviceIdentity;
use crate::join_transport::InvitationCommitter;
use crate::membership::{
    ActiveInvitation, GroupMembership, MembershipBundle, MembershipError, PairingError,
    PairingTrust, SealedMembershipBundle, ShortCode, MAX_MEMBERSHIP_BUNDLE_BYTES,
};
use crate::storage::{Storage, StorageError};
use crate::transport::{MembershipAuthorizer, PeerCredentials, TransportError};
use crate::types::{DeviceProfile, GroupProfile};
use nekodex_domain::{MemberProfile, MembershipEvent};

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("membership error: {0}")]
    Membership(#[from] MembershipError),
    #[error("pairing error: {0}")]
    Pairing(#[from] PairingError),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("persisted group was not found")]
    GroupNotFound,
    #[error("persisted membership checkpoint was not found")]
    CheckpointNotFound,
    #[error("persisted local profile does not match the device identity")]
    IdentityMismatch,
}

pub struct JoinedNode {
    identity: DeviceIdentity,
    profile: DeviceProfile,
    display_name: SyncRwLock<String>,
    group: GroupProfile,
    membership: Arc<RwLock<GroupMembership>>,
    storage: Arc<Storage>,
}

impl JoinedNode {
    #[allow(clippy::too_many_arguments)]
    pub fn create_group(
        identity: DeviceIdentity,
        storage: Arc<Storage>,
        group_id: impl Into<String>,
        group_name: impl Into<String>,
        genesis_event_id: impl Into<String>,
        device_name: impl Into<String>,
        codex_capable: bool,
    ) -> Result<Self, NodeError> {
        let profile = identity.profile(device_name, codex_capable);
        let membership = GroupMembership::create_with_profile(
            group_id,
            genesis_event_id,
            Some((&profile).into()),
            identity.signing_key(),
        )?;
        let group = GroupProfile {
            group_id: membership.group_id().to_owned(),
            name: group_name.into(),
            local_device_id: profile.device_id.clone(),
        };
        persist(&storage, &group, &profile, &membership.export_bundle())?;
        Ok(Self::new(identity, storage, profile, group, membership))
    }

    pub fn join_group(
        identity: DeviceIdentity,
        storage: Arc<Storage>,
        group_name: impl Into<String>,
        device_name: impl Into<String>,
        codex_capable: bool,
        trust: PairingTrust,
        sealed_bundle: &SealedMembershipBundle,
    ) -> Result<Self, NodeError> {
        let bundle = trust.open_bundle(sealed_bundle)?;
        let profile = identity.profile(device_name, codex_capable);
        let membership = GroupMembership::join_from_pairing(trust, bundle, identity.signing_key())?;
        let group = GroupProfile {
            group_id: membership.group_id().to_owned(),
            name: group_name.into(),
            local_device_id: profile.device_id.clone(),
        };
        persist(&storage, &group, &profile, &membership.export_bundle())?;
        Ok(Self::new(identity, storage, profile, group, membership))
    }

    pub fn restore(
        identity: DeviceIdentity,
        storage: Arc<Storage>,
        group_id: &str,
    ) -> Result<Self, NodeError> {
        let group = storage
            .get_group(group_id)?
            .ok_or(NodeError::GroupNotFound)?;
        if group.local_device_id != identity.device_id() {
            return Err(NodeError::IdentityMismatch);
        }
        let profile = storage
            .get_device(&group.local_device_id)?
            .ok_or(NodeError::IdentityMismatch)?;
        if profile.signing_public_key != identity.signing_public_key()
            || profile.noise_public_key != identity.noise_public_key()
        {
            return Err(NodeError::IdentityMismatch);
        }
        let checkpoint = storage
            .membership_checkpoint(group_id)?
            .ok_or(NodeError::CheckpointNotFound)?;
        let bundle = MembershipBundle {
            group_id: group_id.to_owned(),
            events: storage.membership_events(group_id)?,
            checkpoint: checkpoint.clone(),
        };
        let membership = GroupMembership::restore(identity.device_id(), checkpoint, bundle)?;
        Ok(Self::new(identity, storage, profile, group, membership))
    }

    fn new(
        identity: DeviceIdentity,
        storage: Arc<Storage>,
        profile: DeviceProfile,
        group: GroupProfile,
        membership: GroupMembership,
    ) -> Self {
        let display_name = membership
            .active_member_profiles()
            .into_iter()
            .find(|member| member.device_id == profile.device_id)
            .map(|member| member.name)
            .unwrap_or_else(|| profile.name.clone());
        Self {
            identity,
            profile,
            display_name: SyncRwLock::new(display_name),
            group,
            membership: Arc::new(RwLock::new(membership)),
            storage,
        }
    }

    pub fn profile(&self) -> &DeviceProfile {
        &self.profile
    }

    pub fn device_name(&self) -> String {
        self.display_name
            .read()
            .expect("device display name lock poisoned")
            .clone()
    }

    pub fn group(&self) -> &GroupProfile {
        &self.group
    }

    pub fn credentials(&self) -> PeerCredentials {
        self.identity.peer_credentials(self.group.group_id.clone())
    }

    pub fn authorizer(&self) -> MembershipService {
        MembershipService {
            group: Arc::clone(&self.membership),
            storage: Arc::clone(&self.storage),
        }
    }

    pub fn storage(&self) -> Arc<Storage> {
        Arc::clone(&self.storage)
    }

    pub async fn export_bundle(&self) -> MembershipBundle {
        self.membership.read().await.export_bundle()
    }

    pub async fn is_member(&self, device_id: &str) -> bool {
        self.membership.read().await.is_member(device_id)
    }

    pub async fn active_member_ids(&self) -> Vec<String> {
        self.membership.read().await.active_member_ids()
    }

    /// Return only profiles carried by authenticated membership events.
    pub async fn active_member_profiles(&self) -> Vec<MemberProfile> {
        self.membership.read().await.active_member_profiles()
    }

    pub async fn add_paired_member_with_profile(
        &self,
        trust: &PairingTrust,
        event_id: impl Into<String>,
        profile: MemberProfile,
    ) -> Result<(), NodeError> {
        let mut guard = self.membership.write().await;
        let mut candidate = guard.clone();
        candidate.add_paired_member_with_profile(
            trust,
            event_id,
            Some(profile),
            self.identity.signing_key(),
        )?;
        self.storage
            .put_membership_bundle(&candidate.export_bundle())?;
        *guard = candidate;
        Ok(())
    }

    pub async fn create_invitation(
        &self,
        invitation_id: impl Into<String>,
        now: u64,
        expires_at: u64,
        policy: nekodex_domain::invitation::InvitationPolicy,
    ) -> Result<(ActiveInvitation, ShortCode), NodeError> {
        self.membership
            .read()
            .await
            .create_named_invitation(
                invitation_id,
                self.group.name.clone(),
                now,
                expires_at,
                policy,
            )
            .map_err(NodeError::from)
    }

    pub async fn add_paired_member(
        &self,
        trust: &PairingTrust,
        event_id: impl Into<String>,
    ) -> Result<(), NodeError> {
        let mut guard = self.membership.write().await;
        let mut candidate = guard.clone();
        candidate.add_paired_member_with_profile(
            trust,
            event_id,
            trust.joiner_profile().cloned(),
            self.identity.signing_key(),
        )?;
        self.storage
            .put_membership_bundle(&candidate.export_bundle())?;
        *guard = candidate;
        Ok(())
    }

    pub async fn commit_paired_member_and_seal(
        &self,
        trust: &PairingTrust,
        event_id: impl Into<String>,
    ) -> Result<SealedMembershipBundle, NodeError> {
        let mut guard = self.membership.write().await;
        let mut candidate = guard.clone();
        candidate.add_paired_member_with_profile(
            trust,
            event_id,
            trust.joiner_profile().cloned(),
            self.identity.signing_key(),
        )?;
        let bundle = candidate.export_bundle();
        let sealed = trust.seal_bundle(&bundle)?;
        self.storage.put_membership_bundle(&bundle)?;
        *guard = candidate;
        Ok(sealed)
    }

    pub fn invitation_committer(
        self: &Arc<Self>,
        event_id: impl Into<String>,
    ) -> NodeInvitationCommitter {
        NodeInvitationCommitter {
            node: Arc::clone(self),
            event_id: event_id.into(),
        }
    }

    pub async fn revoke_member(
        &self,
        member_id: impl Into<String>,
        event_id: impl Into<String>,
    ) -> Result<(), NodeError> {
        let mut guard = self.membership.write().await;
        let mut candidate = guard.clone();
        candidate.revoke_member(member_id, event_id, self.identity.signing_key())?;
        self.storage
            .put_membership_bundle(&candidate.export_bundle())?;
        *guard = candidate;
        Ok(())
    }

    pub async fn rename_local(
        &self,
        name: impl Into<String>,
        event_id: impl Into<String>,
    ) -> Result<String, NodeError> {
        let mut guard = self.membership.write().await;
        let mut candidate = guard.clone();
        let signed = candidate.rename_local(name, event_id, self.identity.signing_key())?;
        let MembershipEvent::Renamed(rename) = &signed.event else {
            unreachable!("rename_local must create a rename event")
        };
        self.storage
            .put_membership_bundle(&candidate.export_bundle())?;
        *self
            .display_name
            .write()
            .expect("device display name lock poisoned") = rename.name.clone();
        *guard = candidate;
        Ok(rename.name.clone())
    }
}

pub struct NodeInvitationCommitter {
    node: Arc<JoinedNode>,
    event_id: String,
}

#[async_trait]
impl InvitationCommitter for NodeInvitationCommitter {
    type Error = NodeError;

    async fn add_paired_member_and_seal(
        &mut self,
        trust: &PairingTrust,
    ) -> Result<SealedMembershipBundle, Self::Error> {
        self.node
            .commit_paired_member_and_seal(trust, self.event_id.clone())
            .await
    }
}

fn persist(
    storage: &Storage,
    group: &GroupProfile,
    profile: &DeviceProfile,
    bundle: &MembershipBundle,
) -> Result<(), StorageError> {
    storage.put_initial_group_state(group, profile, bundle)
}

#[derive(Clone)]
pub struct MembershipService {
    group: Arc<RwLock<GroupMembership>>,
    storage: Arc<Storage>,
}

#[async_trait]
impl MembershipAuthorizer for MembershipService {
    async fn authorize(
        &self,
        group: &str,
        device: &str,
        key: &[u8; 32],
    ) -> Result<(), TransportError> {
        let key = VerifyingKey::from_bytes(key).map_err(|_| TransportError::Unauthorized)?;
        if member_id_for_key(&key) != device {
            return Err(TransportError::Unauthorized);
        }
        let membership = self.group.read().await;
        if membership.group_id() != group || !membership.is_member(device) {
            return Err(TransportError::Unauthorized);
        }
        Ok(())
    }

    async fn export_sync(&self, group: &str) -> Result<Vec<u8>, TransportError> {
        let membership = self.group.read().await;
        if membership.group_id() != group {
            return Err(TransportError::Unauthorized);
        }
        let bundle = membership.export_bundle();
        let encoded = serde_json::to_vec(&bundle).map_err(|_| TransportError::Protocol)?;
        if encoded.len() > MAX_MEMBERSHIP_BUNDLE_BYTES {
            return Err(TransportError::Oversized);
        }
        Ok(encoded)
    }

    async fn apply_sync(
        &self,
        group: &str,
        device: &str,
        log: &[u8],
    ) -> Result<(), TransportError> {
        if log.len() > MAX_MEMBERSHIP_BUNDLE_BYTES {
            return Err(TransportError::Unauthorized);
        }
        let bundle: MembershipBundle =
            serde_json::from_slice(log).map_err(|_| TransportError::Unauthorized)?;
        if bundle.group_id != group {
            return Err(TransportError::Unauthorized);
        }
        let mut current = self.group.write().await;
        let mut candidate = current.clone();
        candidate
            .merge_verified(&bundle)
            .map_err(|_| TransportError::Unauthorized)?;
        if !candidate.is_member(device) {
            return Err(TransportError::Unauthorized);
        }
        let merged = candidate.export_bundle();
        self.storage
            .put_membership_bundle(&merged)
            .map_err(|_| TransportError::Protocol)?;
        *current = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nekodex_domain::invitation::InvitationPolicy;

    use super::*;
    use crate::identity::MemorySecretStore;
    use crate::membership::JoinerPairing;
    use crate::transport::{ConnectionProvider, DirectWebSocketProvider, accept_direct, establish};

    fn identity() -> DeviceIdentity {
        DeviceIdentity::load_or_generate(&mut MemorySecretStore::new()).unwrap()
    }

    fn policy() -> InvitationPolicy {
        InvitationPolicy {
            max_failures: 3,
            window_limit: 2,
            window: 10,
        }
    }

    async fn pair_node(
        inviter: &JoinedNode,
        joiner_identity: DeviceIdentity,
        joiner_store: Arc<Storage>,
        invitation_id: &str,
        event_id: &str,
        name: &str,
    ) -> JoinedNode {
        let (mut invitation, code) = inviter
            .membership
            .read()
            .await
            .create_invitation(invitation_id, 100, 200, policy())
            .unwrap();
        let (joiner, hello) = JoinerPairing::start(
            invitation.offer(),
            joiner_identity.signing_key(),
            code.as_bytes(),
        );
        let attempt = invitation.begin_attempt(100, &hello).unwrap();
        let (confirmation, joiner_trust) = joiner.finish(attempt.response()).unwrap();
        let inviter_trust = invitation
            .confirm_attempt(100, attempt, &confirmation)
            .unwrap();
        inviter
            .add_paired_member(&inviter_trust, event_id)
            .await
            .unwrap();
        let sealed = inviter_trust
            .seal_bundle(&inviter.export_bundle().await)
            .unwrap();
        JoinedNode::join_group(
            joiner_identity,
            joiner_store,
            inviter.group.name.clone(),
            name,
            name != "Poke3",
            joiner_trust,
            &sealed,
        )
        .unwrap()
    }

    async fn direct_exchange(left: &JoinedNode, right: &JoinedNode) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let address = format!("ws://127.0.0.1:{port}/device");
        let left_credentials = left.credentials();
        let right_credentials = right.credentials();
        let left_authorizer = left.authorizer();
        let right_authorizer = right.authorizer();
        let client = async {
            let socket = DirectWebSocketProvider.connect(&address).await.unwrap();
            establish(socket, true, &left_credentials, &left_authorizer).await
        };
        let server = async {
            let (stream, _) = listener.accept().await.unwrap();
            let socket = accept_direct(stream).await.unwrap();
            establish(socket, false, &right_credentials, &right_authorizer).await
        };
        let (client, server) = tokio::join!(client, server);
        let (mut client, mut server) = (client.unwrap(), server.unwrap());
        client.enqueue(b"p1-business".to_vec()).unwrap();
        let (sent, received) = tokio::join!(client.flush(), server.receive());
        sent.unwrap();
        assert_eq!(received.unwrap(), b"p1-business");
        port
    }

    #[tokio::test]
    async fn joined_state_is_atomic_and_restorable() {
        let a_store = Arc::new(Storage::open_in_memory().unwrap());
        let b_store = Arc::new(Storage::open_in_memory().unwrap());
        let a = JoinedNode::create_group(
            identity(),
            Arc::clone(&a_store),
            "group",
            "My devices",
            "genesis",
            "A",
            true,
        )
        .unwrap();
        let mut b_secrets = MemorySecretStore::new();
        let b_identity = DeviceIdentity::load_or_generate(&mut b_secrets).unwrap();
        let (mut invitation, code) = a
            .membership
            .read()
            .await
            .create_invitation("a-b", 100, 200, policy())
            .unwrap();
        let (joiner, hello) = JoinerPairing::start(
            invitation.offer(),
            b_identity.signing_key(),
            code.as_bytes(),
        );
        let attempt = invitation.begin_attempt(100, &hello).unwrap();
        let (confirmation, joiner_trust) = joiner.finish(attempt.response()).unwrap();
        let inviter_trust = invitation
            .confirm_attempt(100, attempt, &confirmation)
            .unwrap();
        a.add_paired_member(&inviter_trust, "add-b").await.unwrap();
        let sealed = inviter_trust.seal_bundle(&a.export_bundle().await).unwrap();
        let b = JoinedNode::join_group(
            b_identity,
            Arc::clone(&b_store),
            "My devices",
            "B",
            true,
            joiner_trust,
            &sealed,
        )
        .unwrap();

        let reloaded_identity = DeviceIdentity::load_or_generate(&mut b_secrets).unwrap();
        let restored =
            JoinedNode::restore(reloaded_identity, Arc::clone(&b_store), "group").unwrap();
        assert_eq!(restored.profile(), b.profile());
        assert_eq!(b_store.membership_events("group").unwrap().len(), 2);
        assert!(
            b.membership
                .read()
                .await
                .is_member(a.profile().device_id.as_str())
        );
    }

    #[test]
    fn restore_rejects_another_identity() {
        let storage = Arc::new(Storage::open_in_memory().unwrap());
        let node = JoinedNode::create_group(
            identity(),
            Arc::clone(&storage),
            "group",
            "My devices",
            "genesis",
            "A",
            true,
        )
        .unwrap();
        assert!(matches!(
            JoinedNode::restore(identity(), storage, node.group().group_id.as_str()),
            Err(NodeError::IdentityMismatch)
        ));
    }

    #[tokio::test]
    async fn oversized_membership_sync_is_rejected_without_adoption() {
        let storage = Arc::new(Storage::open_in_memory().unwrap());
        let node = JoinedNode::create_group(
            identity(),
            Arc::clone(&storage),
            "group",
            "My devices",
            "genesis",
            "A",
            true,
        )
        .unwrap();
        let mut oversized = node.export_bundle().await;
        oversized.events = vec![oversized.events[0].clone();
            crate::membership::MAX_MEMBERSHIP_EVENTS + 1];
        let encoded = serde_json::to_vec(&oversized).unwrap();
        assert!(encoded.len() > MAX_MEMBERSHIP_BUNDLE_BYTES);
        assert!(matches!(
            node.authorizer()
                .apply_sync("group", node.profile().device_id.as_str(), &encoded)
                .await,
            Err(TransportError::Unauthorized)
        ));
        assert_eq!(node.export_bundle().await.events.len(), 1);
    }

    #[tokio::test]
    async fn p1_three_node_offline_inviter_address_change_and_revocation() {
        let a = JoinedNode::create_group(
            identity(),
            Arc::new(Storage::open_in_memory().unwrap()),
            "group",
            "My devices",
            "genesis",
            "A",
            true,
        )
        .unwrap();
        let b = pair_node(
            &a,
            identity(),
            Arc::new(Storage::open_in_memory().unwrap()),
            "a-b",
            "add-b",
            "B",
        )
        .await;
        let c = pair_node(
            &b,
            identity(),
            Arc::new(Storage::open_in_memory().unwrap()),
            "b-poke3",
            "add-poke3",
            "Poke3",
        )
        .await;

        // The original inviter is no longer involved in discovery, transport,
        // membership synchronization, or business traffic.
        drop(a);
        let first_address = direct_exchange(&b, &c).await;
        let changed_address = direct_exchange(&b, &c).await;
        assert_ne!(first_address, changed_address);

        let c_id = c.profile.device_id.clone();
        assert!(c.membership.read().await.is_member(&c_id));
        c.rename_local("Poke3 renamed", "rename-poke3")
            .await
            .unwrap();
        b.revoke_member(&c_id, "revoke-poke3").await.unwrap();

        // During a partition C has not learned the revocation and cannot claim
        // group-wide immediate enforcement. B already rejects C locally.
        assert!(c.membership.read().await.is_member(&c_id));
        let bundle = b.export_bundle().await;
        let encoded = serde_json::to_vec(&bundle).unwrap();
        c.authorizer()
            .apply_sync("group", b.profile.device_id.as_str(), &encoded)
            .await
            .unwrap();
        assert!(!c.membership.read().await.is_member(&c_id));
        assert_eq!(
            c.storage.membership_checkpoint("group").unwrap(),
            Some(c.export_bundle().await.checkpoint),
            "the durable checkpoint must describe the merged local and remote history"
        );
        assert!(matches!(
            c.authorizer()
                .authorize("group", &c_id, &c.profile.signing_public_key,)
                .await,
            Err(TransportError::Unauthorized)
        ));
    }
}
