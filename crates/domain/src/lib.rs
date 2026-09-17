use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub mod checkpoint;
pub mod invitation;
pub mod pairing;
pub mod signing;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GroupCreated {
    pub event_id: String,
    pub group_id: String,
    pub member_id: String,
    #[serde(default)]
    pub profile: Option<MemberProfile>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemberProfile {
    pub device_id: String,
    pub name: String,
    pub signing_public_key: [u8; 32],
    pub noise_public_key: [u8; 32],
    pub codex_capable: bool,
    pub protocol_version: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemberAdded {
    pub event_id: String,
    pub member_id: String,
    pub issuer_id: String,
    pub parents: BTreeSet<String>,
    #[serde(default)]
    pub profile: Option<MemberProfile>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemberRevoked {
    pub event_id: String,
    pub member_id: String,
    pub issuer_id: String,
    pub parents: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemberRenamed {
    pub event_id: String,
    pub member_id: String,
    pub issuer_id: String,
    pub parents: BTreeSet<String>,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MembershipEvent {
    GroupCreated(GroupCreated),
    Added(MemberAdded),
    Revoked(MemberRevoked),
    Renamed(MemberRenamed),
}

impl MembershipEvent {
    pub fn event_id(&self) -> &str {
        match self {
            Self::GroupCreated(event) => &event.event_id,
            Self::Added(event) => &event.event_id,
            Self::Revoked(event) => &event.event_id,
            Self::Renamed(event) => &event.event_id,
        }
    }

    fn parents(&self) -> Option<&BTreeSet<String>> {
        match self {
            Self::GroupCreated(_) => None,
            Self::Added(event) => Some(&event.parents),
            Self::Revoked(event) => Some(&event.parents),
            Self::Renamed(event) => Some(&event.parents),
        }
    }

    fn issuer_id(&self) -> Option<&str> {
        match self {
            Self::GroupCreated(_) => None,
            Self::Added(event) => Some(&event.issuer_id),
            Self::Revoked(event) => Some(&event.issuer_id),
            Self::Renamed(event) => Some(&event.issuer_id),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventState {
    Valid,
    PendingParents(BTreeSet<String>),
    Invalid(&'static str),
}

#[derive(Clone, Default)]
pub struct MembershipLog {
    events: BTreeMap<String, MembershipEvent>,
    authenticated: BTreeMap<String, signing::SignedMembershipEvent>,
}

impl MembershipLog {
    pub fn insert_signed(
        &mut self,
        expected_group_id: &str,
        signed: signing::SignedMembershipEvent,
    ) -> Result<(), &'static str> {
        let event = signed.verify(expected_group_id)?.clone();
        let event_id = event.event_id().to_owned();
        self.insert(event)?;
        self.authenticated.insert(event_id, signed);
        Ok(())
    }

    pub fn signed_event(&self, event_id: &str) -> Option<&signing::SignedMembershipEvent> {
        self.authenticated.get(event_id)
    }

    /// Iterate over authenticated envelopes in deterministic event-id order.
    /// Pending and invalid events are included so peers can exchange the full
    /// evidence needed to resolve out-of-order arrivals.
    pub fn signed_events(&self) -> impl Iterator<Item = &signing::SignedMembershipEvent> {
        self.authenticated.values()
    }

    pub fn authenticated_event_ids(&self) -> BTreeSet<String> {
        self.authenticated.keys().cloned().collect()
    }

    /// Return valid authenticated events that are not parents of another valid
    /// authenticated event. New membership changes should name this frontier
    /// as their direct causal parents.
    pub fn authenticated_frontier(&self) -> BTreeSet<String> {
        let states = self.evaluate();
        let valid: BTreeSet<_> = self
            .authenticated
            .keys()
            .filter(|id| states.get(*id) == Some(&EventState::Valid))
            .cloned()
            .collect();
        let referenced: BTreeSet<_> = self
            .events
            .iter()
            .filter(|(id, _)| valid.contains(*id))
            .filter_map(|(_, event)| event.parents())
            .flatten()
            .filter(|parent| valid.contains(*parent))
            .cloned()
            .collect();
        valid.difference(&referenced).cloned().collect()
    }

    /// An authenticated connection must never rely on an event inserted only
    /// through internal test construction.
    pub fn is_fully_authenticated(&self) -> bool {
        self.events.len() == self.authenticated.len()
    }

    /// Pin every currently valid, authenticated event as a device-local
    /// anti-rollback fence. The caller is responsible for persisting it.
    pub fn checkpoint(
        &self,
        expected_group_id: &str,
    ) -> Result<checkpoint::MembershipCheckpoint, checkpoint::CheckpointError> {
        let states = self.evaluate();
        checkpoint::MembershipCheckpoint::from_signed_events(
            expected_group_id,
            self.authenticated.iter().filter_map(|(event_id, signed)| {
                (states.get(event_id) == Some(&EventState::Valid)).then_some(signed)
            }),
        )
    }

    /// Check that this authenticated log has not lost any event fixed by a
    /// previously persisted checkpoint. Causal validation remains separate.
    pub fn verify_checkpoint(
        &self,
        expected_group_id: &str,
        checkpoint: &checkpoint::MembershipCheckpoint,
    ) -> Result<(), checkpoint::CheckpointError> {
        checkpoint.verify_signed_events(expected_group_id, self.authenticated.values())
    }

    fn insert(&mut self, event: MembershipEvent) -> Result<(), &'static str> {
        let id = event.event_id().to_owned();
        if id.is_empty() {
            return Err("event id is empty");
        }
        if self
            .events
            .get(&id)
            .is_some_and(|current| current != &event)
        {
            return Err("event id collision");
        }
        self.events.insert(id, event);
        Ok(())
    }

    pub fn event_state(&self, event_id: &str) -> Option<EventState> {
        self.evaluate().get(event_id).cloned()
    }

    pub fn is_revoked(&self, member_id: &str) -> bool {
        let states = self.evaluate();
        self.events.iter().any(|(id, event)| {
            states.get(id) == Some(&EventState::Valid)
                && matches!(event, MembershipEvent::Revoked(value) if value.member_id == member_id)
        })
    }

    pub fn is_member(&self, member_id: &str) -> bool {
        if self.is_revoked(member_id) {
            return false;
        }
        let states = self.evaluate();
        self.events.iter().any(|(id, event)| {
            states.get(id) == Some(&EventState::Valid)
                && match event {
                    MembershipEvent::GroupCreated(value) => value.member_id == member_id,
                    MembershipEvent::Added(value) => value.member_id == member_id,
                    MembershipEvent::Revoked(_) => false,
                    MembershipEvent::Renamed(_) => false,
                }
        })
    }

    fn evaluate(&self) -> BTreeMap<String, EventState> {
        let mut states = BTreeMap::new();
        let genesis: Vec<_> = self
            .events
            .iter()
            .filter_map(|(id, event)| {
                matches!(event, MembershipEvent::GroupCreated(_)).then_some(id)
            })
            .collect();

        for (id, event) in &self.events {
            if matches!(event, MembershipEvent::GroupCreated(_)) {
                states.insert(
                    id.clone(),
                    if genesis.len() == 1 {
                        EventState::Valid
                    } else {
                        EventState::Invalid("membership log must contain exactly one genesis event")
                    },
                );
            }
        }

        loop {
            let mut progressed = false;
            for (id, event) in &self.events {
                if states.contains_key(id) {
                    continue;
                }
                let parents = event.parents().expect("non-genesis event has parents");
                if parents.is_empty() {
                    states.insert(id.clone(), EventState::Invalid("event has no parents"));
                    progressed = true;
                    continue;
                }
                if parents.contains(id) {
                    states.insert(id.clone(), EventState::Invalid("event references itself"));
                    progressed = true;
                    continue;
                }
                let missing: BTreeSet<_> = parents
                    .iter()
                    .filter(|parent| !self.events.contains_key(*parent))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    continue;
                }
                if parents
                    .iter()
                    .any(|parent| matches!(states.get(parent), Some(EventState::Invalid(_))))
                {
                    states.insert(
                        id.clone(),
                        EventState::Invalid("event descends from an invalid parent"),
                    );
                    progressed = true;
                    continue;
                }
                if parents.iter().any(|parent| !states.contains_key(parent)) {
                    continue;
                }

                let history = self.ancestor_ids(parents);
                let issuer = event.issuer_id().expect("non-genesis event has issuer");
                if matches!(event, MembershipEvent::Renamed(value) if value.member_id != value.issuer_id)
                {
                    states.insert(
                        id.clone(),
                        EventState::Invalid("members may only rename themselves"),
                    );
                } else if !self.member_in_history(issuer, &history, &states) {
                    states.insert(
                        id.clone(),
                        EventState::Invalid("issuer is not active in causal history"),
                    );
                } else {
                    states.insert(id.clone(), EventState::Valid);
                }
                progressed = true;
            }
            if !progressed {
                break;
            }
        }

        for (id, event) in &self.events {
            if states.contains_key(id) {
                continue;
            }
            let missing: BTreeSet<String> = event
                .parents()
                .into_iter()
                .flatten()
                .filter(|parent| !self.events.contains_key(*parent))
                .cloned()
                .collect();
            states.insert(
                id.clone(),
                if !missing.is_empty() {
                    EventState::PendingParents(missing)
                } else {
                    EventState::Invalid("event graph contains a cycle")
                },
            );
        }
        states
    }

    fn ancestor_ids(&self, roots: &BTreeSet<String>) -> BTreeSet<String> {
        let mut ancestors = BTreeSet::new();
        let mut pending: Vec<_> = roots.iter().cloned().collect();
        while let Some(id) = pending.pop() {
            if !ancestors.insert(id.clone()) {
                continue;
            }
            if let Some(parents) = self.events.get(&id).and_then(MembershipEvent::parents) {
                pending.extend(parents.iter().cloned());
            }
        }
        ancestors
    }

    fn member_in_history(
        &self,
        member_id: &str,
        history: &BTreeSet<String>,
        states: &BTreeMap<String, EventState>,
    ) -> bool {
        let events: Vec<_> = history
            .iter()
            .filter(|id| states.get(*id) == Some(&EventState::Valid))
            .filter_map(|id| self.events.get(id))
            .collect();
        let added = events.iter().any(|event| match event {
            MembershipEvent::GroupCreated(value) => value.member_id == member_id,
            MembershipEvent::Added(value) => value.member_id == member_id,
            MembershipEvent::Revoked(_) => false,
            MembershipEvent::Renamed(_) => false,
        });
        let revoked = events.iter().any(|event| {
            matches!(event, MembershipEvent::Revoked(value) if value.member_id == member_id)
        });
        added && !revoked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parents(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn genesis() -> MembershipEvent {
        MembershipEvent::GroupCreated(GroupCreated {
            event_id: "genesis".into(),
            group_id: "group-1".into(),
            member_id: "a".into(),
            profile: None,
        })
    }

    fn add(
        event_id: &str,
        member_id: &str,
        issuer_id: &str,
        parent_ids: &[&str],
    ) -> MembershipEvent {
        MembershipEvent::Added(MemberAdded {
            event_id: event_id.into(),
            member_id: member_id.into(),
            issuer_id: issuer_id.into(),
            parents: parents(parent_ids),
            profile: None,
        })
    }

    fn revoke(
        event_id: &str,
        member_id: &str,
        issuer_id: &str,
        parent_ids: &[&str],
    ) -> MembershipEvent {
        MembershipEvent::Revoked(MemberRevoked {
            event_id: event_id.into(),
            member_id: member_id.into(),
            issuer_id: issuer_id.into(),
            parents: parents(parent_ids),
        })
    }

    #[test]
    fn three_nodes_remain_independent_after_the_inviter_goes_offline() {
        let mut log = MembershipLog::default();
        for event in [
            genesis(),
            add("add-b", "b", "a", &["genesis"]),
            add("add-c", "c", "b", &["add-b"]),
        ] {
            log.insert(event).unwrap();
        }
        assert!(log.is_member("b"));
        assert!(log.is_member("c"));
        assert_eq!(log.event_state("add-c"), Some(EventState::Valid));
    }

    #[test]
    fn revocation_wins_independent_of_arrival_order_and_prevents_reactivation() {
        let events = [
            genesis(),
            add("add-b", "b", "a", &["genesis"]),
            revoke("revoke-b", "b", "a", &["add-b"]),
            add("readd-b", "b", "a", &["revoke-b"]),
        ];
        for order in [[0, 1, 2, 3], [3, 2, 1, 0]] {
            let mut log = MembershipLog::default();
            for index in order {
                log.insert(events[index].clone()).unwrap();
            }
            assert!(!log.is_member("b"));
            assert!(log.is_revoked("b"));
        }
    }

    #[test]
    fn authenticated_iteration_and_frontier_only_expose_signed_evidence() {
        use ed25519_dalek::SigningKey;
        use signing::{SignedMembershipEvent, member_id_for_key};

        let a = SigningKey::from_bytes(&[21; 32]);
        let b = SigningKey::from_bytes(&[22; 32]);
        let a_id = member_id_for_key(&a.verifying_key());
        let b_id = member_id_for_key(&b.verifying_key());
        let mut log = MembershipLog::default();
        for event in [
            MembershipEvent::GroupCreated(GroupCreated {
                event_id: "root".into(),
                group_id: "group".into(),
                member_id: a_id.clone(),
                profile: None,
            }),
            MembershipEvent::Added(MemberAdded {
                event_id: "add-b".into(),
                member_id: b_id,
                issuer_id: a_id,
                parents: ["root".into()].into(),
                profile: None,
            }),
        ] {
            log.insert_signed(
                "group",
                SignedMembershipEvent::sign("group", event, &a).unwrap(),
            )
            .unwrap();
        }

        assert!(log.is_fully_authenticated());
        assert_eq!(
            log.authenticated_event_ids(),
            BTreeSet::from(["add-b".into(), "root".into()])
        );
        assert_eq!(
            log.authenticated_frontier(),
            BTreeSet::from(["add-b".into()])
        );
        assert_eq!(log.signed_events().count(), 2);
    }

    #[test]
    fn revoked_issuer_cannot_authorize_a_descendant_event() {
        let mut log = MembershipLog::default();
        for event in [
            genesis(),
            add("add-b", "b", "a", &["genesis"]),
            revoke("revoke-b", "b", "a", &["add-b"]),
            add("add-c", "c", "b", &["revoke-b"]),
        ] {
            log.insert(event).unwrap();
        }
        assert_eq!(
            log.event_state("add-c"),
            Some(EventState::Invalid(
                "issuer is not active in causal history"
            ))
        );
        assert!(!log.is_member("c"));
    }

    #[test]
    fn missing_parent_is_pending_and_cycles_are_invalid() {
        let mut log = MembershipLog::default();
        log.insert(genesis()).unwrap();
        log.insert(add("pending", "b", "a", &["unknown"])).unwrap();
        log.insert(add("cycle-a", "c", "a", &["cycle-b"])).unwrap();
        log.insert(add("cycle-b", "d", "a", &["cycle-a"])).unwrap();
        assert_eq!(
            log.event_state("pending"),
            Some(EventState::PendingParents(parents(&["unknown"])))
        );
        assert_eq!(
            log.event_state("cycle-a"),
            Some(EventState::Invalid("event graph contains a cycle"))
        );
    }

    #[test]
    fn event_ids_cannot_be_reused_for_different_content() {
        let mut log = MembershipLog::default();
        log.insert(add("same", "b", "a", &["genesis"])).unwrap();
        assert_eq!(
            log.insert(add("same", "c", "a", &["genesis"])),
            Err("event id collision")
        );
    }
}
