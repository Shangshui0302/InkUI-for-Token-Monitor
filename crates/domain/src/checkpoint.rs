//! Device-local membership checkpoints.
//!
//! A checkpoint is a local anti-rollback fence: once a device has accepted a
//! set of events, a later state must still contain every one of those events.
//! It is not a proof of complete history for a device which has never held the
//! checkpoint (nor is it a replacement for signature and log validation).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::signing::SignedMembershipEvent;

const CHECKPOINT_DOMAIN: &[u8] = b"nekodex/membership-checkpoint/v1";

/// The locally persisted fence for one membership group.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipCheckpoint {
    pub group_id: String,
    pub event_ids: BTreeSet<String>,
    /// Canonical digest of `group_id` and `event_ids`, for corruption detection.
    pub event_ids_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckpointError {
    EmptyGroup,
    EmptyEventId,
    GroupMismatch,
    InvalidCheckpointDigest,
    MissingEvents(BTreeSet<String>),
    InvalidSignedEvent(&'static str),
}

impl MembershipCheckpoint {
    /// Create a checkpoint from event IDs already accepted by the caller's log.
    pub fn from_event_ids<I, S>(group_id: &str, event_ids: I) -> Result<Self, CheckpointError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let event_ids = collect_event_ids(event_ids)?;
        Ok(Self {
            group_id: checked_group(group_id)?,
            event_ids_digest: digest(group_id, &event_ids),
            event_ids,
        })
    }

    /// Create a checkpoint from authenticated events accepted by the log.
    pub fn from_signed_events<'a, I>(group_id: &str, events: I) -> Result<Self, CheckpointError>
    where
        I: IntoIterator<Item = &'a SignedMembershipEvent>,
    {
        let event_ids = events
            .into_iter()
            .map(|signed| {
                signed
                    .verify(group_id)
                    .map(|event| event.event_id().to_owned())
                    .map_err(CheckpointError::InvalidSignedEvent)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_event_ids(group_id, event_ids)
    }

    /// Reject a state from another group, a state with a corrupted checkpoint,
    /// or a state which has lost any event fixed by this checkpoint.
    ///
    /// Extra events are allowed, so normal forward progress remains possible.
    pub fn verify_event_ids<I, S>(
        &self,
        expected_group_id: &str,
        current_event_ids: I,
    ) -> Result<(), CheckpointError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if self.group_id != expected_group_id {
            return Err(CheckpointError::GroupMismatch);
        }
        checked_group(expected_group_id)?;
        if self.event_ids_digest != digest(&self.group_id, &self.event_ids) {
            return Err(CheckpointError::InvalidCheckpointDigest);
        }

        let current = collect_event_ids(current_event_ids)?;
        let missing: BTreeSet<_> = self.event_ids.difference(&current).cloned().collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(CheckpointError::MissingEvents(missing))
        }
    }

    /// Verify a remote state represented by signed events, including each
    /// event's group and signature before applying the anti-rollback check.
    pub fn verify_signed_events<'a, I>(
        &self,
        expected_group_id: &str,
        events: I,
    ) -> Result<(), CheckpointError>
    where
        I: IntoIterator<Item = &'a SignedMembershipEvent>,
    {
        let ids = events
            .into_iter()
            .map(|signed| {
                signed
                    .verify(expected_group_id)
                    .map(|event| event.event_id().to_owned())
                    .map_err(CheckpointError::InvalidSignedEvent)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.verify_event_ids(expected_group_id, ids)
    }
}

fn checked_group(group_id: &str) -> Result<String, CheckpointError> {
    if group_id.is_empty() {
        Err(CheckpointError::EmptyGroup)
    } else {
        Ok(group_id.to_owned())
    }
}

fn collect_event_ids<I, S>(event_ids: I) -> Result<BTreeSet<String>, CheckpointError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    event_ids
        .into_iter()
        .map(|id| {
            let id = id.as_ref();
            if id.is_empty() {
                Err(CheckpointError::EmptyEventId)
            } else {
                Ok(id.to_owned())
            }
        })
        .collect()
}

fn digest(group_id: &str, event_ids: &BTreeSet<String>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    field(&mut hasher, CHECKPOINT_DOMAIN);
    field(&mut hasher, group_id.as_bytes());
    hasher.update((event_ids.len() as u64).to_be_bytes());
    for event_id in event_ids {
        field(&mut hasher, event_id.as_bytes());
    }
    hasher.finalize().into()
}

fn field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fixed_event_is_rejected() {
        let checkpoint =
            MembershipCheckpoint::from_event_ids("group", ["genesis", "add-b"]).unwrap();
        assert_eq!(
            checkpoint.verify_event_ids("group", ["genesis"]),
            Err(CheckpointError::MissingEvents(BTreeSet::from([
                "add-b".into(),
            ])))
        );
    }

    #[test]
    fn different_group_is_rejected() {
        let checkpoint = MembershipCheckpoint::from_event_ids("group", ["genesis"]).unwrap();
        assert_eq!(
            checkpoint.verify_event_ids("other", ["genesis"]),
            Err(CheckpointError::GroupMismatch)
        );
    }

    #[test]
    fn forward_state_with_extra_events_is_accepted() {
        let checkpoint = MembershipCheckpoint::from_event_ids("group", ["genesis"]).unwrap();
        assert_eq!(
            checkpoint.verify_event_ids("group", ["genesis", "add-b"]),
            Ok(())
        );
    }

    #[test]
    fn tampered_checkpoint_digest_is_rejected() {
        let mut checkpoint = MembershipCheckpoint::from_event_ids("group", ["genesis"]).unwrap();
        checkpoint.event_ids.insert("add-b".into());
        assert_eq!(
            checkpoint.verify_event_ids("group", ["genesis", "add-b"]),
            Err(CheckpointError::InvalidCheckpointDigest)
        );
    }
}
