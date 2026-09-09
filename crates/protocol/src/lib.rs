//! Shared discovery, pairing, and control-link types.

mod identity;

pub use identity::{default_identity_path, pairing_message, Identity};

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
pub const MAX_WIRE_FRAME_BYTES: usize = 16 * 1024;

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
