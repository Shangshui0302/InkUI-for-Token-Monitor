//! Versioned, domain-separated membership event authentication.
//!
//! Authentication does not authorize an issuer: the membership log must still
//! check the issuer's membership in the event's causal history. The expected
//! group must come from the caller's trusted group context, not this envelope.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::MembershipEvent;

const EVENT_DOMAIN: &[u8] = b"nekodex/membership-event/ed25519/v1";
const IDENTITY_DOMAIN: &[u8] = b"nekodex/member-identity/ed25519/v1";

/// A self-certifying identity. Key rotation creates a new member identity.
pub fn member_id_for_key(key: &VerifyingKey) -> String {
    let mut hash = Sha256::new();
    hash.update(IDENTITY_DOMAIN);
    hash.update(key.as_bytes());
    let digest = hash.finalize();
    let mut id = String::from("ed25519:");
    for byte in digest {
        use std::fmt::Write;
        write!(&mut id, "{byte:02x}").expect("writing to String cannot fail");
    }
    id
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedMembershipEvent {
    pub group_id: String,
    pub signer_public_key: [u8; 32],
    pub event: MembershipEvent,
    /// Exactly 64 bytes; kept as a Vec for portable serde support.
    pub signature: Vec<u8>,
}

impl SignedMembershipEvent {
    pub fn sign(
        group_id: &str,
        event: MembershipEvent,
        key: &SigningKey,
    ) -> Result<Self, &'static str> {
        validate_context(group_id, &event, &key.verifying_key())?;
        let mut signed = Self {
            group_id: group_id.to_owned(),
            signer_public_key: key.verifying_key().to_bytes(),
            event,
            signature: Vec::new(),
        };
        signed.signature = key.sign(&signed.signing_bytes()).to_bytes().to_vec();
        Ok(signed)
    }

    /// Verify in an independently selected group context before log insertion.
    pub fn verify(&self, expected_group_id: &str) -> Result<&MembershipEvent, &'static str> {
        if self.group_id != expected_group_id {
            return Err("membership signature belongs to a different group");
        }
        let key = VerifyingKey::from_bytes(&self.signer_public_key)
            .map_err(|_| "invalid member public key")?;
        validate_context(expected_group_id, &self.event, &key)?;
        let signature = Signature::from_slice(&self.signature)
            .map_err(|_| "invalid membership signature length")?;
        key.verify_strict(&self.signing_bytes(), &signature)
            .map_err(|_| "invalid membership signature")?;
        Ok(&self.event)
    }

    // Canonical binary encoding, independent of JSON field ordering or escaping:
    // length-prefixed domain and group, fixed-width public key, one-byte variant,
    // length-prefixed UTF-8 fields, then a u64 parent count and sorted parents.
    // Every integer uses big-endian u64; no text normalization is performed.
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        field(&mut bytes, EVENT_DOMAIN);
        field(&mut bytes, self.group_id.as_bytes());
        bytes.extend_from_slice(&self.signer_public_key);
        match &self.event {
            MembershipEvent::GroupCreated(event) => {
                bytes.push(0);
                field(&mut bytes, event.event_id.as_bytes());
                field(&mut bytes, event.group_id.as_bytes());
                field(&mut bytes, event.member_id.as_bytes());
            }
            MembershipEvent::Added(event) => {
                bytes.push(1);
                field(&mut bytes, event.event_id.as_bytes());
                field(&mut bytes, event.member_id.as_bytes());
                field(&mut bytes, event.issuer_id.as_bytes());
                parents(&mut bytes, &event.parents);
            }
            MembershipEvent::Revoked(event) => {
                bytes.push(2);
                field(&mut bytes, event.event_id.as_bytes());
                field(&mut bytes, event.member_id.as_bytes());
                field(&mut bytes, event.issuer_id.as_bytes());
                parents(&mut bytes, &event.parents);
            }
        }
        bytes
    }
}

fn validate_context(
    group_id: &str,
    event: &MembershipEvent,
    key: &VerifyingKey,
) -> Result<(), &'static str> {
    if group_id.is_empty() {
        return Err("group id is empty");
    }
    let issuer = match event {
        MembershipEvent::GroupCreated(event) => {
            if event.group_id != group_id {
                return Err("genesis group does not match signature group");
            }
            &event.member_id
        }
        MembershipEvent::Added(event) => &event.issuer_id,
        MembershipEvent::Revoked(event) => &event.issuer_id,
    };
    if issuer != &member_id_for_key(key) {
        return Err("issuer identity does not match signing key");
    }
    Ok(())
}

fn field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn parents(bytes: &mut Vec<u8>, values: &std::collections::BTreeSet<String>) {
    bytes.extend_from_slice(&(values.len() as u64).to_be_bytes());
    for value in values {
        field(bytes, value.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GroupCreated, MemberAdded, MemberRevoked, MembershipLog};

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    fn added() -> MembershipEvent {
        MembershipEvent::Added(MemberAdded {
            event_id: "add-1".into(),
            member_id: member_id_for_key(&SigningKey::from_bytes(&[8; 32]).verifying_key()),
            issuer_id: member_id_for_key(&key().verifying_key()),
            parents: ["root".into(), "other".into()].into(),
        })
    }

    #[test]
    fn valid_variants_round_trip() {
        let events = [
            MembershipEvent::GroupCreated(GroupCreated {
                event_id: "root".into(),
                group_id: "group".into(),
                member_id: member_id_for_key(&key().verifying_key()),
            }),
            added(),
            MembershipEvent::Revoked(MemberRevoked {
                event_id: "revoke".into(),
                member_id: "target".into(),
                issuer_id: member_id_for_key(&key().verifying_key()),
                parents: ["root".into()].into(),
            }),
        ];
        for event in events {
            let signed = SignedMembershipEvent::sign("group", event.clone(), &key()).unwrap();
            assert_eq!(signed.verify("group"), Ok(&event));
            let json = serde_json::to_string(&signed).unwrap();
            let decoded: SignedMembershipEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded.verify("group"), Ok(&event));
        }
    }

    #[test]
    fn replacement_replay_and_impersonation_fail() {
        let original = SignedMembershipEvent::sign("group", added(), &key()).unwrap();
        assert!(original.verify("elsewhere").is_err());
        let mut changed = original.clone();
        changed.group_id = "elsewhere".into();
        assert!(changed.verify("elsewhere").is_err());
        let mut changed = original.clone();
        changed.signer_public_key = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
        assert!(changed.verify("group").is_err());
        for mutation in 0..4 {
            let mut changed = original.clone();
            if let MembershipEvent::Added(event) = &mut changed.event {
                match mutation {
                    0 => event.event_id.push('x'),
                    1 => event.member_id.push('x'),
                    2 => event.issuer_id.push('x'),
                    _ => {
                        event.parents.insert("injected".into());
                    }
                }
            }
            assert!(changed.verify("group").is_err());
        }
        let mut changed = original.clone();
        if let MembershipEvent::Added(event) = changed.event {
            changed.event = MembershipEvent::Revoked(MemberRevoked {
                event_id: event.event_id,
                member_id: event.member_id,
                issuer_id: event.issuer_id,
                parents: event.parents,
            });
        }
        assert!(changed.verify("group").is_err());
        assert!(
            SignedMembershipEvent::sign("group", added(), &SigningKey::from_bytes(&[9; 32]))
                .is_err()
        );
        let mut changed = original;
        changed.signature.pop();
        assert!(changed.verify("group").is_err());
    }

    #[test]
    fn genesis_is_bound_to_group_and_founder() {
        let event = MembershipEvent::GroupCreated(GroupCreated {
            event_id: "root".into(),
            group_id: "group".into(),
            member_id: member_id_for_key(&key().verifying_key()),
        });
        assert!(SignedMembershipEvent::sign("different", event.clone(), &key()).is_err());
        assert!(
            SignedMembershipEvent::sign("group", event, &SigningKey::from_bytes(&[9; 32])).is_err()
        );
        assert!(SignedMembershipEvent::sign("", added(), &key()).is_err());
    }

    #[test]
    fn encoding_is_unambiguous_and_parent_order_is_canonical() {
        let a = SignedMembershipEvent::sign("group", added(), &key()).unwrap();
        let mut b = added();
        if let MembershipEvent::Added(event) = &mut b {
            event.parents = ["other".into(), "root".into()].into();
        }
        let b = SignedMembershipEvent::sign("group", b, &key()).unwrap();
        assert_eq!(a.signature, b.signature);
        let mut left = Vec::new();
        field(&mut left, b"ab");
        field(&mut left, b"c");
        let mut right = Vec::new();
        field(&mut right, b"a");
        field(&mut right, b"bc");
        assert_ne!(left, right);
    }

    #[test]
    fn membership_log_accepts_only_verified_envelopes_through_its_public_path() {
        let event = MembershipEvent::GroupCreated(GroupCreated {
            event_id: "root".into(),
            group_id: "group".into(),
            member_id: member_id_for_key(&key().verifying_key()),
        });
        let signed = SignedMembershipEvent::sign("group", event, &key()).unwrap();
        let mut log = MembershipLog::default();
        log.insert_signed("group", signed.clone()).unwrap();
        assert_eq!(log.signed_event("root"), Some(&signed));

        let mut replay = signed;
        replay.group_id = "elsewhere".into();
        assert!(log.insert_signed("group", replay).is_err());
    }

    #[test]
    fn persisted_checkpoint_rejects_a_truncated_authenticated_log() {
        let founder = key();
        let founder_id = member_id_for_key(&founder.verifying_key());
        let genesis = SignedMembershipEvent::sign(
            "group",
            MembershipEvent::GroupCreated(GroupCreated {
                event_id: "root".into(),
                group_id: "group".into(),
                member_id: founder_id.clone(),
            }),
            &founder,
        )
        .unwrap();
        let add = SignedMembershipEvent::sign(
            "group",
            MembershipEvent::Added(MemberAdded {
                event_id: "add-b".into(),
                member_id: member_id_for_key(&SigningKey::from_bytes(&[8; 32]).verifying_key()),
                issuer_id: founder_id,
                parents: ["root".into()].into(),
            }),
            &founder,
        )
        .unwrap();

        let mut complete = MembershipLog::default();
        complete.insert_signed("group", genesis.clone()).unwrap();
        complete.insert_signed("group", add).unwrap();
        let checkpoint = complete.checkpoint("group").unwrap();

        let mut truncated = MembershipLog::default();
        truncated.insert_signed("group", genesis).unwrap();
        assert!(matches!(
            truncated.verify_checkpoint("group", &checkpoint),
            Err(crate::checkpoint::CheckpointError::MissingEvents(_))
        ));
    }

    #[test]
    fn independently_held_signed_logs_do_not_depend_on_the_inviter_runtime() {
        let founder = key();
        let member_b = SigningKey::from_bytes(&[8; 32]);
        let member_c = SigningKey::from_bytes(&[9; 32]);
        let founder_id = member_id_for_key(&founder.verifying_key());
        let member_b_id = member_id_for_key(&member_b.verifying_key());
        let member_c_id = member_id_for_key(&member_c.verifying_key());
        let events = [
            SignedMembershipEvent::sign(
                "group",
                MembershipEvent::GroupCreated(GroupCreated {
                    event_id: "root".into(),
                    group_id: "group".into(),
                    member_id: founder_id.clone(),
                }),
                &founder,
            )
            .unwrap(),
            SignedMembershipEvent::sign(
                "group",
                MembershipEvent::Added(MemberAdded {
                    event_id: "add-b".into(),
                    member_id: member_b_id.clone(),
                    issuer_id: founder_id,
                    parents: ["root".into()].into(),
                }),
                &founder,
            )
            .unwrap(),
            SignedMembershipEvent::sign(
                "group",
                MembershipEvent::Added(MemberAdded {
                    event_id: "add-c".into(),
                    member_id: member_c_id.clone(),
                    issuer_id: member_b_id.clone(),
                    parents: ["add-b".into()].into(),
                }),
                &member_b,
            )
            .unwrap(),
        ];

        let mut inviter_log = MembershipLog::default();
        let mut b_log = MembershipLog::default();
        let mut c_log = MembershipLog::default();
        for event in events {
            inviter_log.insert_signed("group", event.clone()).unwrap();
            b_log.insert_signed("group", event.clone()).unwrap();
            c_log.insert_signed("group", event).unwrap();
        }
        drop(inviter_log);

        assert!(b_log.is_member(&member_c_id));
        assert!(c_log.is_member(&member_b_id));
    }
}
