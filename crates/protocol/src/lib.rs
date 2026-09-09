//! Shared discovery, pairing, and control-link types.

mod identity;
mod network;

pub use identity::{default_identity_path, pairing_message, Identity};
pub use network::discovery_interface_v4;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const SERVICE_NAME: &str = "codex-monitor";
pub const DISCOVERY_GROUP_V4: [u8; 4] = [239, 255, 77, 77];
pub const DISCOVERY_PORT: u16 = 37991;
pub const MONITOR_PORT: u16 = 8787;
pub const ANNOUNCE_INTERVAL_SECS: u64 = 5;
pub const ANNOUNCEMENT_TTL_SECS: u64 = 20;
pub const HEARTBEAT_INTERVAL_SECS: u64 = 5;
pub const HEARTBEAT_TIMEOUT_SECS: u64 = 30;
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
pub const MAX_WIRE_FRAME_BYTES: usize = 64 * 1024;
pub const STATE_SNAPSHOT_CAPABILITY: &str = "state-snapshot-v1";
pub const MAX_STATE_SESSIONS: usize = 128;
pub const MAX_STATE_BUCKETS: usize = 32;
pub const MAX_STATE_ID_BYTES: usize = 128;
pub const MAX_STATE_TEXT_BYTES: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CollectorStatus {
    Ready,
    Degraded,
    AuthRequired,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    pub used_percent: Option<f64>,
    pub window_duration_mins: Option<u64>,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreditBalance {
    pub has_credits: Option<bool>,
    pub unlimited: Option<bool>,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitBucket {
    pub limit_id: String,
    pub limit_name: Option<String>,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    pub credits: Option<CreditBalance>,
    pub plan_type: Option<String>,
    pub rate_limit_reached_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountState {
    /// A stable, non-secret grouping key. Raw account identifiers never leave the host.
    pub account_key: Option<String>,
    pub auth_mode: Option<String>,
    pub plan_type: Option<String>,
    pub requires_openai_auth: bool,
    pub rate_limits: Vec<RateLimitBucket>,
    pub reset_credits_available: Option<u64>,
    pub usage_lifetime_tokens: Option<u64>,
    pub usage_peak_daily_tokens: Option<u64>,
    pub observed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub thread_id: String,
    pub title: Option<String>,
    pub status: String,
    pub last_output_summary: Option<String>,
    pub created_at: Option<u64>,
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StateSnapshot {
    pub host_id: String,
    pub alias: String,
    pub sequence: u64,
    pub captured_at: u64,
    pub collector_status: CollectorStatus,
    pub error: Option<String>,
    pub sessions: Vec<SessionState>,
    pub account: Option<AccountState>,
}

impl StateSnapshot {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.host_id.is_empty() || self.host_id.len() > MAX_STATE_ID_BYTES {
            return Err("invalid state host id");
        }
        if self.alias.is_empty() || self.alias.len() > MAX_STATE_ID_BYTES {
            return Err("invalid state alias");
        }
        if self
            .error
            .as_ref()
            .is_some_and(|value| value.len() > MAX_STATE_TEXT_BYTES)
        {
            return Err("state error is too long");
        }
        if self.sessions.len() > MAX_STATE_SESSIONS {
            return Err("too many state sessions");
        }
        for session in &self.sessions {
            if session.thread_id.is_empty() || session.thread_id.len() > MAX_STATE_ID_BYTES {
                return Err("invalid state thread id");
            }
            if session
                .title
                .as_ref()
                .is_some_and(|value| value.len() > MAX_STATE_TEXT_BYTES)
                || session
                    .last_output_summary
                    .as_ref()
                    .is_some_and(|value| value.len() > MAX_STATE_TEXT_BYTES)
                || session.status.len() > 64
            {
                return Err("state session text is too long");
            }
        }
        if let Some(account) = &self.account {
            if account
                .account_key
                .as_ref()
                .is_some_and(|value| value.len() > 128)
                || account
                    .auth_mode
                    .as_ref()
                    .is_some_and(|value| value.len() > 64)
                || account
                    .plan_type
                    .as_ref()
                    .is_some_and(|value| value.len() > 64)
                || account.rate_limits.len() > MAX_STATE_BUCKETS
            {
                return Err("invalid state account");
            }
            for bucket in &account.rate_limits {
                if bucket.limit_id.is_empty() || bucket.limit_id.len() > MAX_STATE_ID_BYTES {
                    return Err("invalid state limit id");
                }
                if bucket
                    .limit_name
                    .as_ref()
                    .is_some_and(|value| value.len() > 128)
                    || bucket
                        .plan_type
                        .as_ref()
                        .is_some_and(|value| value.len() > 64)
                    || bucket
                        .rate_limit_reached_type
                        .as_ref()
                        .is_some_and(|value| value.len() > 64)
                {
                    return Err("state rate limit text is too long");
                }
                if let Some(credits) = &bucket.credits {
                    if credits
                        .balance
                        .as_ref()
                        .is_some_and(|value| value.len() > 64)
                    {
                        return Err("state credit balance is too long");
                    }
                }
            }
        }
        Ok(())
    }
}

/// Public discovery data. Credentials and Codex account data never belong here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryAnnouncement {
    pub service: String,
    pub version: u16,
    pub host_id: String,
    pub alias: String,
    pub port: u16,
    pub public_key: String,
    pub nonce: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub capabilities: Vec<String>,
}

impl DiscoveryAnnouncement {
    pub fn validate(&self, now: u64) -> Result<(), &'static str> {
        if self.service != SERVICE_NAME {
            return Err("unexpected service");
        }
        if self.version != PROTOCOL_VERSION {
            return Err("unsupported protocol version");
        }
        if self.host_id.is_empty() || self.host_id.len() > 128 {
            return Err("invalid host id");
        }
        if self.alias.is_empty() || self.alias.len() > 128 {
            return Err("invalid alias");
        }
        if self.port == 0 {
            return Err("invalid port");
        }
        if self.public_key.len() != 64 || hex::decode(&self.public_key).is_err() {
            return Err("invalid public key");
        }
        if self.host_id != format!("ed25519:{}", &self.public_key[..16]) {
            return Err("host id does not match public key");
        }
        if self.nonce.is_empty() || self.nonce.len() > 128 {
            return Err("invalid nonce");
        }
        if self.expires_at < self.issued_at || now > self.expires_at {
            return Err("expired announcement");
        }
        if self.expires_at.saturating_sub(self.issued_at) > ANNOUNCEMENT_TTL_SECS * 2 {
            return Err("announcement ttl is too long");
        }
        if self.capabilities.len() > 32 || self.capabilities.iter().any(|v| v.len() > 64) {
            return Err("invalid capabilities");
        }
        Ok(())
    }
}

/// Newline-delimited messages used by the M1 bootstrap transport.
/// M2 may replace the framing with WebSocket without changing these payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum WireMessage {
    AgentHello {
        version: u16,
        host_id: String,
        alias: String,
        public_key: String,
        pairing_code: Option<String>,
        capabilities: Vec<String>,
    },
    PairChallenge {
        challenge: String,
        monitor_id: String,
        signature: String,
    },
    PairResponse {
        host_id: String,
        signature: String,
    },
    PairAccepted {
        host_id: String,
    },
    PairRejected {
        reason: String,
    },
    Heartbeat {
        host_id: String,
    },
    HeartbeatAck,
    StateSnapshot {
        snapshot: StateSnapshot,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announcement_round_trips_as_json() {
        let value = DiscoveryAnnouncement {
            service: SERVICE_NAME.to_string(),
            version: PROTOCOL_VERSION,
            host_id: "host-a".to_string(),
            alias: "NixOS".to_string(),
            port: 8787,
            public_key: "test-key".to_string(),
            nonce: "nonce".to_string(),
            issued_at: 10,
            expires_at: 20,
            capabilities: vec!["sessions".to_string()],
        };

        let json = serde_json::to_string(&value).expect("serialize announcement");
        let decoded: DiscoveryAnnouncement =
            serde_json::from_str(&json).expect("deserialize announcement");
        assert_eq!(decoded, value);
    }

    #[test]
    fn wire_message_round_trips_as_json() {
        let value = WireMessage::Heartbeat {
            host_id: "host-a".to_string(),
        };
        let json = serde_json::to_string(&value).expect("serialize message");
        let decoded: WireMessage = serde_json::from_str(&json).expect("deserialize message");
        assert_eq!(decoded, value);
    }

    #[test]
    fn state_snapshot_round_trips_and_validates() {
        let value = StateSnapshot {
            host_id: "ed25519:0000000000000000".to_string(),
            alias: "NixOS".to_string(),
            sequence: 1,
            captured_at: 10,
            collector_status: CollectorStatus::Ready,
            error: None,
            sessions: vec![SessionState {
                thread_id: "thread-1".to_string(),
                title: Some("Example".to_string()),
                status: "idle".to_string(),
                last_output_summary: Some("Done".to_string()),
                created_at: Some(1),
                updated_at: Some(2),
            }],
            account: None,
        };
        assert!(value.validate().is_ok());
        let message = WireMessage::StateSnapshot { snapshot: value };
        let json = serde_json::to_string(&message).expect("serialize state snapshot");
        let decoded: WireMessage = serde_json::from_str(&json).expect("deserialize state snapshot");
        assert_eq!(decoded, message);
    }

    #[test]
    fn announcement_validation_rejects_expired_values() {
        let value = DiscoveryAnnouncement {
            service: SERVICE_NAME.to_string(),
            version: PROTOCOL_VERSION,
            host_id: format!("ed25519:{}", "00".repeat(8)),
            alias: "Monitor".to_string(),
            port: MONITOR_PORT,
            public_key: "00".repeat(32),
            nonce: "nonce".to_string(),
            issued_at: 10,
            expires_at: 20,
            capabilities: Vec::new(),
        };
        assert_eq!(value.validate(21), Err("expired announcement"));
    }
}
