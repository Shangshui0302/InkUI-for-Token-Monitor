//! Shared discovery, pairing, and control-link types.

mod identity;
mod network;

pub use identity::{
    constant_time_hex_eq, default_identity_path, default_pairing_identity_path,
    pairing_acceptance_message, pairing_code_proof, pairing_message, pairing_transcript,
    sign_envelope, validate_node_identity, validate_pairing_public_key, verify_envelope,
    verify_pairing_signature, Identity, PairingIdentity,
};
pub use network::discovery_interface_v4;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const SERVICE_NAME: &str = "codex-monitor";
pub const DISCOVERY_GROUP_V4: [u8; 4] = [239, 255, 77, 77];
pub const DISCOVERY_PORT: u16 = 37991;
pub const MONITOR_PORT: u16 = 8787;
pub const ANNOUNCE_INTERVAL_SECS: u64 = 5;
pub const ANNOUNCEMENT_TTL_SECS: u64 = 20;
pub const PAIRING_CODE_LENGTH: usize = 8;
pub const HEARTBEAT_INTERVAL_SECS: u64 = 5;
pub const HEARTBEAT_TIMEOUT_SECS: u64 = 30;
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
/// Peer JSONL frames must fit a normal session snapshot without making the
/// transport an unbounded input sink.
pub const MAX_WIRE_FRAME_BYTES: usize = 512 * 1024;
pub const STATE_SNAPSHOT_CAPABILITY: &str = "state-snapshot-v1";
/// Capability negotiated before exchanging peer-owned state snapshots.
pub const PEER_SYNC_CAPABILITY: &str = "peer-sync-v1";
/// Peer snapshots signed by the P-256 pairing identity for Android 10 clients.
pub const P256_PEER_SYNC_CAPABILITY: &str = "peer-sync-p256-v1";
/// Version of the language-neutral envelope. The legacy WireMessage protocol remains v1.
pub const ENVELOPE_VERSION: u16 = 2;
pub const MESH_PROTOCOL_VERSION: u16 = ENVELOPE_VERSION;
pub const STATE_SNAPSHOT_KIND: &str = "state_snapshot";
pub const CONTROL_REQUEST_KIND: &str = "control_request";
pub const CONTROL_RESULT_KIND: &str = "control_result";
pub const MAX_STATE_SESSIONS: usize = 25;
pub const MAX_STATE_BUCKETS: usize = 32;
pub const MAX_STATE_DAILY_USAGE: usize = 400;
pub const MAX_STATE_ID_BYTES: usize = 128;
pub const MAX_STATE_TEXT_BYTES: usize = 1024;
pub const MAX_STATE_DATE_BYTES: usize = 10;

/// Pairing codes are short, case-sensitive ASCII alphanumeric strings.
/// Generation additionally guarantees at least one digit, uppercase letter,
/// and lowercase letter; validation deliberately only enforces the shared
/// wire/configuration format.
pub fn validate_pairing_code(value: &str) -> Result<(), &'static str> {
    if value.len() != PAIRING_CODE_LENGTH || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err("must be exactly 8 ASCII letters and digits");
    }
    Ok(())
}

const MAX_ENVELOPE_KIND_BYTES: usize = 64;
const MAX_ENVELOPE_REQUEST_ID_BYTES: usize = 128;
const ED25519_SIGNATURE_HEX_BYTES: usize = 128;

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
pub struct DailyUsageBucket {
    pub start_date: String,
    pub tokens: u64,
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
    #[serde(default)]
    pub daily_usage: Vec<DailyUsageBucket>,
    pub observed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    pub sequence: u64,
    pub captured_at: u64,
    pub collector_status: CollectorStatus,
    pub error: Option<String>,
    pub sessions: Vec<SessionState>,
    pub account: Option<AccountState>,
    /// The logical network that owns this observation. It is optional so a v1
    /// snapshot can still be decoded during the migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    /// The node that collected this observation. For v1 snapshots this falls
    /// back to `host_id` via [`StateSnapshot::source_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_node_id: Option<String>,
}

impl StateSnapshot {
    /// Return the stable source identity used for deduplication and merging.
    pub fn source_id(&self) -> &str {
        self.source_node_id.as_deref().unwrap_or(&self.host_id)
    }

    /// Return the deterministic ordering tuple used when snapshots conflict.
    /// Sequence is authoritative for one source; captured time and source id
    /// make the comparison total for malformed or colliding inputs.
    pub fn ordering_key(&self) -> (u64, u64, &str) {
        (self.sequence, self.captured_at, self.source_id())
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.host_id.is_empty() || self.host_id.len() > MAX_STATE_ID_BYTES {
            return Err("invalid state host id");
        }
        if self.alias.is_empty() || self.alias.len() > MAX_STATE_ID_BYTES {
            return Err("invalid state alias");
        }
        if self
            .os
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 64)
        {
            return Err("invalid state operating system");
        }
        if self.sequence == 0 {
            return Err("invalid state sequence");
        }
        if self
            .network_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_STATE_ID_BYTES)
        {
            return Err("invalid state network id");
        }
        if self
            .source_node_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_STATE_ID_BYTES)
        {
            return Err("invalid state source node id");
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
                .turn_id
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > MAX_STATE_ID_BYTES)
            {
                return Err("invalid state turn id");
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
            if account.daily_usage.len() > MAX_STATE_DAILY_USAGE {
                return Err("too many daily usage buckets");
            }
            for bucket in &account.daily_usage {
                if !valid_date(&bucket.start_date) {
                    return Err("invalid daily usage date");
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Interrupt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlRequest {
    pub action: ControlAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_node_id: Option<String>,
    pub thread_id: String,
    pub turn_id: String,
    pub request_id: String,
    pub expires_at: u64,
}

impl ControlRequest {
    pub fn validate(&self, now: u64) -> Result<(), &'static str> {
        if self.thread_id.is_empty() || self.thread_id.len() > MAX_STATE_ID_BYTES {
            return Err("invalid control thread id");
        }
        if self.turn_id.is_empty() || self.turn_id.len() > MAX_STATE_ID_BYTES {
            return Err("invalid control turn id");
        }
        if self.request_id.is_empty() || self.request_id.len() > MAX_ENVELOPE_REQUEST_ID_BYTES {
            return Err("invalid control request id");
        }
        if self
            .target_node_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_STATE_ID_BYTES)
        {
            return Err("invalid control target node id");
        }
        if self.expires_at < now || self.expires_at.saturating_sub(now) > 120 {
            return Err("control request expired or too far in the future");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControlResult {
    pub request_id: String,
    pub ok: bool,
    pub message: String,
}

impl ControlResult {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.request_id.is_empty() || self.request_id.len() > MAX_ENVELOPE_REQUEST_ID_BYTES {
            return Err("invalid control result request id");
        }
        if self.message.len() > MAX_STATE_TEXT_BYTES {
            return Err("control result message is too long");
        }
        Ok(())
    }
}

fn valid_date(value: &str) -> bool {
    if value.len() != MAX_STATE_DATE_BYTES {
        return false;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return false;
    }
    let month = u16::from(bytes[5] - b'0') * 10 + u16::from(bytes[6] - b'0');
    let day = u16::from(bytes[8] - b'0') * 10 + u16::from(bytes[9] - b'0');
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// Public discovery data. Credentials and Codex account data never belong here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryAnnouncement {
    pub service: String,
    pub version: u16,
    pub host_id: String,
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    pub port: u16,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "httpPort",
        alias = "http_port"
    )]
    pub http_port: Option<u16>,
    pub public_key: String,
    /// Optional P-256 SPKI key used for Android-compatible pairing signatures.
    /// `public_key` remains the legacy Ed25519 node identity.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "pairingPublicKey",
        alias = "pairing_public_key"
    )]
    pub pairing_public_key: Option<String>,
    pub nonce: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub capabilities: Vec<String>,
    /// Optional during the v1 migration; paired nodes use it to avoid joining
    /// a different logical mesh on the same LAN.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "networkId",
        alias = "network_id"
    )]
    pub network_id: Option<String>,
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
        if self
            .os
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 64)
        {
            return Err("invalid operating system");
        }
        if self.port == 0 {
            return Err("invalid port");
        }
        if self.http_port == Some(0) {
            return Err("invalid http port");
        }
        validate_node_identity(
            &self.host_id,
            &self.public_key,
            self.pairing_public_key.as_deref(),
        )?;
        if self.nonce.is_empty() || self.nonce.len() > 128 {
            return Err("invalid nonce");
        }
        if self.expires_at < self.issued_at || now > self.expires_at {
            return Err("expired announcement");
        }
        if self.expires_at.saturating_sub(self.issued_at) > ANNOUNCEMENT_TTL_SECS * 2 {
            return Err("announcement ttl is too long");
        }
        if self
            .network_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_STATE_ID_BYTES)
        {
            return Err("invalid announcement network id");
        }
        if self.capabilities.len() > 32 || self.capabilities.iter().any(|v| v.len() > 64) {
            return Err("invalid capabilities");
        }
        Ok(())
    }
}

/// Versioned, language-neutral message envelope used for peer sync and future
/// embedded clients. Its fields deliberately stay independent of the legacy
/// [`WireMessage`] framing so the v1 TCP JSONL path can migrate incrementally.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageEnvelope<T> {
    pub version: u16,
    pub kind: String,
    #[serde(rename = "networkId", alias = "network_id")]
    pub network_id: String,
    #[serde(rename = "senderId", alias = "sender_id")]
    pub sender_id: String,
    pub sequence: u64,
    #[serde(rename = "sentAt", alias = "sent_at")]
    pub sent_at: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "requestId",
        alias = "request_id"
    )]
    pub request_id: Option<String>,
    pub payload: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl<T> MessageEnvelope<T> {
    pub fn new(
        kind: impl Into<String>,
        network_id: impl Into<String>,
        sender_id: impl Into<String>,
        sequence: u64,
        sent_at: u64,
        request_id: Option<String>,
        payload: T,
    ) -> Self {
        Self {
            version: ENVELOPE_VERSION,
            kind: kind.into(),
            network_id: network_id.into(),
            sender_id: sender_id.into(),
            sequence,
            sent_at,
            request_id,
            payload,
            signature: None,
        }
    }

    pub fn with_signature(mut self, signature: String) -> Self {
        self.signature = Some(signature);
        self
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != ENVELOPE_VERSION {
            return Err("unsupported envelope version");
        }
        if self.kind.is_empty() || self.kind.len() > MAX_ENVELOPE_KIND_BYTES {
            return Err("invalid envelope kind");
        }
        if self.network_id.is_empty() || self.network_id.len() > MAX_STATE_ID_BYTES {
            return Err("invalid envelope network id");
        }
        if self.sender_id.is_empty() || self.sender_id.len() > MAX_STATE_ID_BYTES {
            return Err("invalid envelope sender id");
        }
        if self
            .request_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > MAX_ENVELOPE_REQUEST_ID_BYTES)
        {
            return Err("invalid envelope request id");
        }
        if self.signature.as_ref().is_some_and(|value| {
            value.len() != ED25519_SIGNATURE_HEX_BYTES || hex::decode(value).is_err()
        }) {
            return Err("invalid envelope signature");
        }
        Ok(())
    }
}

impl<T: Serialize> MessageEnvelope<T> {
    /// Serialize the envelope without its signature. Struct field order is
    /// fixed, giving all supported clients the same signing bytes.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        #[derive(Serialize)]
        struct UnsignedEnvelope<'a, T: Serialize + ?Sized> {
            version: u16,
            kind: &'a str,
            #[serde(rename = "networkId")]
            network_id: &'a str,
            #[serde(rename = "senderId")]
            sender_id: &'a str,
            sequence: u64,
            #[serde(rename = "sentAt")]
            sent_at: u64,
            #[serde(skip_serializing_if = "Option::is_none", rename = "requestId")]
            request_id: Option<&'a str>,
            payload: &'a T,
        }

        serde_json::to_vec(&UnsignedEnvelope {
            version: self.version,
            kind: &self.kind,
            network_id: &self.network_id,
            sender_id: &self.sender_id,
            sequence: self.sequence,
            sent_at: self.sent_at,
            request_id: self.request_id.as_deref(),
            payload: &self.payload,
        })
    }
}

/// Newline-delimited messages used by the M1 bootstrap transport.
/// M2 may replace the framing with WebSocket without changing these payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum WireMessage {
    AgentHello {
        version: u16,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "networkId",
            alias = "network_id"
        )]
        network_id: Option<String>,
        host_id: String,
        alias: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        os: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "httpPort",
            alias = "http_port"
        )]
        http_port: Option<u16>,
        public_key: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "pairingPublicKey",
            alias = "pairing_public_key"
        )]
        pairing_public_key: Option<String>,
        capabilities: Vec<String>,
    },
    PairChallenge {
        challenge: String,
        monitor_id: String,
        signature: String,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "publicKey")]
        public_key: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "pairingPublicKey",
            alias = "pairing_public_key"
        )]
        pairing_public_key: Option<String>,
    },
    PairResponse {
        host_id: String,
        signature: String,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "codeProof")]
        code_proof: Option<String>,
    },
    PairAccepted {
        host_id: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "networkId",
            alias = "network_id"
        )]
        network_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "codeProof")]
        code_proof: Option<String>,
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
    PeerSnapshot {
        envelope: MessageEnvelope<StateSnapshot>,
    },
    /// Exact JSON bytes plus a P-256 signature for Android 10 peers.
    AndroidSnapshot {
        #[serde(rename = "networkId")]
        network_id: String,
        #[serde(rename = "senderId")]
        sender_id: String,
        #[serde(rename = "payloadJson")]
        payload_json: String,
        signature: String,
    },
    ControlRequest {
        envelope: MessageEnvelope<ControlRequest>,
    },
    ControlResponse {
        envelope: MessageEnvelope<ControlResult>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_format_is_exactly_eight_ascii_alphanumeric_bytes() {
        assert!(validate_pairing_code("aB3dE7qK").is_ok());
        assert!(validate_pairing_code("12345678").is_ok());
        assert!(validate_pairing_code("abcdefgh").is_ok());
        assert!(validate_pairing_code("ABCDEFGH").is_ok());
        assert!(validate_pairing_code("Ab3dE7q").is_err());
        assert!(validate_pairing_code("Ab3dE7qK9").is_err());
        assert!(validate_pairing_code("Ab3dE7q!").is_err());
        assert!(validate_pairing_code("Ab3dE7é").is_err());
    }

    #[test]
    fn pairing_code_hmac_and_transcript_are_canonical() {
        let transcript = pairing_transcript(
            "initiator-id",
            "ed:aa",
            "p256:ii",
            "acceptor-id",
            "ed:bb",
            "p256:jj",
            "test-network",
            "010203",
        );
        assert_eq!(
            pairing_code_proof("Ab3dE7qK", "initiator", &transcript),
            "d3c36aa348cdf4d9e468cd777935154e3151affe24684827d1ecb4aae026b94c",
        );
        assert_ne!(
            pairing_code_proof("Ab3dE7qK", "initiator", &transcript),
            pairing_code_proof("Ab3dE7qK", "acceptor", &transcript),
        );
        let changed = pairing_transcript(
            "initiator-id",
            "ed:aa",
            "p256:ii",
            "acceptor-id",
            "ed:bb",
            "p256:jj",
            "other-network",
            "010203",
        );
        assert_ne!(
            pairing_code_proof("Ab3dE7qK", "initiator", &transcript),
            pairing_code_proof("Ab3dE7qK", "initiator", &changed),
        );
    }

    #[test]
    fn agent_hello_has_no_pairing_code_wire_field() {
        let hello = WireMessage::AgentHello {
            version: PROTOCOL_VERSION,
            network_id: Some("test-network".into()),
            host_id: "host-a".into(),
            alias: "node".into(),
            os: None,
            http_port: None,
            public_key: "primary".into(),
            pairing_public_key: Some("pairing".into()),
            capabilities: vec![],
        };
        let json = serde_json::to_value(hello).unwrap();
        assert!(json["data"].get("pairing_code").is_none());
    }

    #[test]
    fn announcement_round_trips_as_json() {
        let value = DiscoveryAnnouncement {
            service: SERVICE_NAME.to_string(),
            version: PROTOCOL_VERSION,
            host_id: "host-a".to_string(),
            alias: "NixOS".to_string(),
            os: None,
            port: 8787,
            http_port: None,
            public_key: "test-key".to_string(),
            pairing_public_key: None,
            nonce: "nonce".to_string(),
            issued_at: 10,
            expires_at: 20,
            capabilities: vec!["sessions".to_string()],
            network_id: None,
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
    fn old_announcement_without_os_still_decodes() {
        let json = format!(
            r#"{{"service":"{SERVICE_NAME}","version":{PROTOCOL_VERSION},"host_id":"ed25519:0000000000000000","alias":"old","port":8787,"public_key":"{}","nonce":"n","issued_at":10,"expires_at":20,"capabilities":[]}}"#,
            "00".repeat(32)
        );
        let decoded: DiscoveryAnnouncement = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.os, None);
    }

    #[test]
    fn state_snapshot_round_trips_and_validates() {
        let value = StateSnapshot {
            host_id: "ed25519:0000000000000000".to_string(),
            alias: "NixOS".to_string(),
            os: None,
            sequence: 1,
            captured_at: 10,
            collector_status: CollectorStatus::Ready,
            error: None,
            sessions: vec![SessionState {
                thread_id: "thread-1".to_string(),
                turn_id: None,
                title: Some("Example".to_string()),
                status: "idle".to_string(),
                last_output_summary: Some("Done".to_string()),
                created_at: Some(1),
                updated_at: Some(2),
            }],
            account: None,
            network_id: Some("network-1".to_string()),
            source_node_id: Some("ed25519:0000000000000000".to_string()),
        };
        assert!(value.validate().is_ok());
        let message = WireMessage::StateSnapshot { snapshot: value };
        let json = serde_json::to_string(&message).expect("serialize state snapshot");
        let decoded: WireMessage = serde_json::from_str(&json).expect("deserialize state snapshot");
        assert_eq!(decoded, message);
    }

    #[test]
    fn largest_peer_snapshot_fits_wire_frame_after_json_escaping() {
        let escaped_id = "\0".repeat(MAX_STATE_ID_BYTES);
        let escaped_text = "\0".repeat(MAX_STATE_TEXT_BYTES);
        let escaped_short = "\0".repeat(64);
        let snapshot = StateSnapshot {
            host_id: escaped_id.clone(),
            alias: escaped_id.clone(),
            os: None,
            sequence: 1,
            captured_at: 1,
            collector_status: CollectorStatus::Degraded,
            error: Some(escaped_text.clone()),
            sessions: (0..MAX_STATE_SESSIONS)
                .map(|_| SessionState {
                    thread_id: escaped_id.clone(),
                    turn_id: Some(escaped_id.clone()),
                    title: Some(escaped_text.clone()),
                    status: escaped_short.clone(),
                    last_output_summary: Some(escaped_text.clone()),
                    created_at: Some(u64::MAX),
                    updated_at: Some(u64::MAX),
                })
                .collect(),
            account: Some(AccountState {
                account_key: Some(escaped_id.clone()),
                auth_mode: Some(escaped_short.clone()),
                plan_type: Some(escaped_short.clone()),
                requires_openai_auth: true,
                rate_limits: (0..MAX_STATE_BUCKETS)
                    .map(|_| RateLimitBucket {
                        limit_id: escaped_id.clone(),
                        limit_name: Some(escaped_id.clone()),
                        primary: Some(RateLimitWindow {
                            used_percent: Some(100.0),
                            window_duration_mins: Some(u64::MAX),
                            resets_at: Some(u64::MAX),
                        }),
                        secondary: Some(RateLimitWindow {
                            used_percent: Some(100.0),
                            window_duration_mins: Some(u64::MAX),
                            resets_at: Some(u64::MAX),
                        }),
                        credits: Some(CreditBalance {
                            has_credits: Some(true),
                            unlimited: Some(true),
                            balance: Some(escaped_short.clone()),
                        }),
                        plan_type: Some(escaped_short.clone()),
                        rate_limit_reached_type: Some(escaped_short.clone()),
                    })
                    .collect(),
                reset_credits_available: Some(u64::MAX),
                usage_lifetime_tokens: Some(u64::MAX),
                usage_peak_daily_tokens: Some(u64::MAX),
                daily_usage: (0..MAX_STATE_DAILY_USAGE)
                    .map(|_| DailyUsageBucket {
                        start_date: "9999-12-31".to_string(),
                        tokens: u64::MAX,
                    })
                    .collect(),
                observed_at: u64::MAX,
            }),
            network_id: Some(escaped_id.clone()),
            source_node_id: Some(escaped_id.clone()),
        };
        assert!(snapshot.validate().is_ok());

        let envelope = MessageEnvelope::new(
            STATE_SNAPSHOT_KIND,
            escaped_id.clone(),
            escaped_id,
            u64::MAX,
            u64::MAX,
            None,
            snapshot,
        )
        .with_signature("00".repeat(64));
        assert!(envelope.validate().is_ok());

        let encoded = serde_json::to_vec(&WireMessage::PeerSnapshot { envelope })
            .expect("serialize maximum peer snapshot");
        assert!(encoded.len() + 1 <= MAX_WIRE_FRAME_BYTES);
    }

    #[test]
    fn announcement_validation_rejects_expired_values() {
        let value = DiscoveryAnnouncement {
            service: SERVICE_NAME.to_string(),
            version: PROTOCOL_VERSION,
            host_id: format!("ed25519:{}", "00".repeat(8)),
            alias: "Monitor".to_string(),
            os: None,
            port: MONITOR_PORT,
            http_port: None,
            public_key: "00".repeat(32),
            pairing_public_key: None,
            nonce: "nonce".to_string(),
            issued_at: 10,
            expires_at: 20,
            capabilities: Vec::new(),
            network_id: None,
        };
        assert_eq!(value.validate(21), Err("expired announcement"));
    }

    #[test]
    fn control_request_round_trips_and_rejects_expired_values() {
        let request = ControlRequest {
            action: ControlAction::Interrupt,
            target_node_id: Some("node-a".to_string()),
            thread_id: "thread-1".to_string(),
            turn_id: "turn-1".to_string(),
            request_id: "request-1".to_string(),
            expires_at: 110,
        };
        assert!(request.validate(100).is_ok());
        assert_eq!(
            request.validate(111),
            Err("control request expired or too far in the future")
        );
        let message = WireMessage::ControlRequest {
            envelope: MessageEnvelope::new(
                CONTROL_REQUEST_KIND,
                "network-1",
                "node-a",
                1,
                100,
                Some(request.request_id.clone()),
                request,
            ),
        };
        let json = serde_json::to_string(&message).expect("serialize control request");
        let decoded: WireMessage =
            serde_json::from_str(&json).expect("deserialize control request");
        assert_eq!(decoded, message);
    }

    #[test]
    fn invalid_utf8_date_shape_is_rejected_without_panicking() {
        assert!(!valid_date("2024-😀-01"));
        assert!(!valid_date("2024-13-01"));
        assert!(valid_date("2024-02-29"));
    }
}
