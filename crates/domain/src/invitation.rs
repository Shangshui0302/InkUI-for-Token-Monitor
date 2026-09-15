//! Pure lifecycle and abuse-control state for a PAKE invitation.
//!
//! This module deliberately does not know how a short code is represented or
//! verified.  The PAKE implementation reports only whether an attempt
//! succeeded; the invitation never stores (or returns) the code.

use std::collections::VecDeque;

/// Opaque identifier of an invitation.  It is not the short code.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InvitationId(String);

impl InvitationId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvitationConfigError> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvitationConfigError::EmptyId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Limits applied to failed PAKE attempts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvitationPolicy {
    /// The invitation is locked after this many failed attempts.
    pub max_failures: u32,
    /// At most this many failed attempts are accepted in `window` ticks.
    pub window_limit: u32,
    /// A caller-defined time window.  Zero is invalid.
    pub window: u64,
}

impl InvitationPolicy {
    pub fn validate(self) -> Result<Self, InvitationConfigError> {
        if self.max_failures == 0 {
            return Err(InvitationConfigError::ZeroMaxFailures);
        }
        if self.window_limit == 0 {
            return Err(InvitationConfigError::ZeroWindowLimit);
        }
        if self.window == 0 {
            return Err(InvitationConfigError::ZeroWindow);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PakeAttempt {
    Succeeded,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvitationState {
    Active,
    Locked,
    Consumed,
    Expired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvitationConfigError {
    EmptyId,
    NonPositiveLifetime,
    ZeroMaxFailures,
    ZeroWindowLimit,
    ZeroWindow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvitationError {
    Expired,
    Locked,
    Consumed,
    RateLimited,
    BeforeCreation,
}

/// Result of an accepted PAKE outcome.  A successful outcome consumes the
/// invitation immediately and cannot be submitted again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceptedAttempt {
    pub state: InvitationState,
}

/// One-time invitation lifecycle.  `now` values are supplied by the caller;
/// this type never reads a clock and therefore remains deterministic and easy
/// to persist/replay.
pub struct Invitation {
    id: InvitationId,
    created_at: u64,
    expires_at: u64,
    policy: InvitationPolicy,
    failures: u32,
    failure_times: VecDeque<u64>,
    consumed: bool,
    locked: bool,
}

impl Invitation {
    pub fn new(
        id: InvitationId,
        created_at: u64,
        expires_at: u64,
        policy: InvitationPolicy,
    ) -> Result<Self, InvitationConfigError> {
        if expires_at <= created_at {
            return Err(InvitationConfigError::NonPositiveLifetime);
        }
        Ok(Self {
            id,
            created_at,
            expires_at,
            policy: policy.validate()?,
            failures: 0,
            failure_times: VecDeque::new(),
            consumed: false,
            locked: false,
        })
    }

    pub fn id(&self) -> &InvitationId {
        &self.id
    }

    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub fn failure_count(&self) -> u32 {
        self.failures
    }

    /// Expiry is a half-open boundary: `now == expires_at` is expired.
    pub fn state(&self, now: u64) -> Result<InvitationState, InvitationError> {
        if now < self.created_at {
            return Err(InvitationError::BeforeCreation);
        }
        if self.consumed {
            return Ok(InvitationState::Consumed);
        }
        if now >= self.expires_at {
            return Ok(InvitationState::Expired);
        }
        if self.locked {
            return Ok(InvitationState::Locked);
        }
        Ok(InvitationState::Active)
    }

    /// Check admission before performing PAKE, so a rate-limited peer cannot
    /// consume cryptographic work and then report an outcome.
    pub fn can_attempt(&mut self, now: u64) -> Result<(), InvitationError> {
        match self.state(now)? {
            InvitationState::Active => {}
            InvitationState::Expired => return Err(InvitationError::Expired),
            InvitationState::Locked => return Err(InvitationError::Locked),
            InvitationState::Consumed => return Err(InvitationError::Consumed),
        }

        self.prune_failures(now);
        if self.failure_times.len() >= self.policy.window_limit as usize {
            return Err(InvitationError::RateLimited);
        }
        Ok(())
    }

    /// Record only the PAKE result; no short-code material is accepted or
    /// retained here. The caller must call `can_attempt` before doing PAKE;
    /// this method repeats the admission check to fail closed.
    pub fn record(
        &mut self,
        now: u64,
        attempt: PakeAttempt,
    ) -> Result<AcceptedAttempt, InvitationError> {
        self.can_attempt(now)?;

        match attempt {
            PakeAttempt::Succeeded => {
                self.consumed = true;
                Ok(AcceptedAttempt {
                    state: InvitationState::Consumed,
                })
            }
            PakeAttempt::Failed => {
                self.failures += 1;
                self.failure_times.push_back(now);
                if self.failures >= self.policy.max_failures {
                    self.locked = true;
                }
                Ok(AcceptedAttempt {
                    state: if self.locked {
                        InvitationState::Locked
                    } else {
                        InvitationState::Active
                    },
                })
            }
        }
    }

    fn prune_failures(&mut self, now: u64) {
        let cutoff = now.saturating_sub(self.policy.window);
        while self
            .failure_times
            .front()
            .is_some_and(|timestamp| *timestamp < cutoff)
        {
            self.failure_times.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invitation() -> Invitation {
        Invitation::new(
            InvitationId::new("invite-1").unwrap(),
            100,
            200,
            InvitationPolicy {
                max_failures: 3,
                window_limit: 2,
                window: 10,
            },
        )
        .unwrap()
    }

    #[test]
    fn success_consumes_invitation_once() {
        let mut invitation = invitation();
        assert_eq!(
            invitation.record(100, PakeAttempt::Succeeded),
            Ok(AcceptedAttempt {
                state: InvitationState::Consumed
            })
        );
        assert_eq!(
            invitation.record(101, PakeAttempt::Succeeded),
            Err(InvitationError::Consumed)
        );
    }

    #[test]
    fn expiry_is_at_the_exact_boundary() {
        let mut invitation = invitation();
        assert_eq!(invitation.state(199), Ok(InvitationState::Active));
        assert_eq!(invitation.state(200), Ok(InvitationState::Expired));
        assert_eq!(
            invitation.record(200, PakeAttempt::Succeeded),
            Err(InvitationError::Expired)
        );
    }

    #[test]
    fn failures_lock_and_rate_limit_in_a_sliding_window() {
        let mut invitation = invitation();
        assert_eq!(
            invitation.record(100, PakeAttempt::Failed).unwrap().state,
            InvitationState::Active
        );
        assert_eq!(
            invitation.record(105, PakeAttempt::Failed).unwrap().state,
            InvitationState::Active
        );
        assert_eq!(
            invitation.record(106, PakeAttempt::Failed),
            Err(InvitationError::RateLimited)
        );
        assert_eq!(
            invitation.record(106, PakeAttempt::Succeeded),
            Err(InvitationError::RateLimited)
        );
        assert_eq!(
            invitation.record(111, PakeAttempt::Failed).unwrap().state,
            InvitationState::Locked
        );
        assert_eq!(
            invitation.record(111, PakeAttempt::Failed),
            Err(InvitationError::Locked)
        );
    }

    #[test]
    fn creation_boundary_and_invalid_configuration_are_explicit() {
        let mut invitation = invitation();
        assert_eq!(invitation.state(99), Err(InvitationError::BeforeCreation));
        assert_eq!(
            invitation.record(99, PakeAttempt::Succeeded),
            Err(InvitationError::BeforeCreation)
        );
        assert_eq!(InvitationId::new(""), Err(InvitationConfigError::EmptyId));
        assert!(matches!(
            Invitation::new(
                InvitationId::new("x").unwrap(),
                1,
                1,
                InvitationPolicy {
                    max_failures: 1,
                    window_limit: 1,
                    window: 1
                }
            ),
            Err(InvitationConfigError::NonPositiveLifetime)
        ));
    }
}
