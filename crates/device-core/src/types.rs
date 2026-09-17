use std::net::SocketAddr;

use nekodex_domain::MemberProfile;
use serde::{Deserialize, Serialize};

pub const TRANSPORT_PROTOCOL_VERSION: u16 = 2;
/// Compatibility alias for persisted profile fields created before transport
/// and business protocol versions were named independently.
pub const DEVICE_PROTOCOL_VERSION: u16 = TRANSPORT_PROTOCOL_VERSION;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceProfile {
    pub device_id: String,
    pub name: String,
    pub signing_public_key: [u8; 32],
    pub noise_public_key: [u8; 32],
    pub codex_capable: bool,
    /// Transport generation advertised when this profile joined the group.
    /// Live compatibility is decided by the authenticated handshake, not by
    /// comparing this historical value after an application upgrade.
    pub protocol_version: u16,
}

impl From<&DeviceProfile> for MemberProfile {
    fn from(value: &DeviceProfile) -> Self {
        Self {
            device_id: value.device_id.clone(),
            name: value.name.clone(),
            signing_public_key: value.signing_public_key,
            noise_public_key: value.noise_public_key,
            codex_capable: value.codex_capable,
            protocol_version: value.protocol_version,
        }
    }
}

impl TryFrom<MemberProfile> for DeviceProfile {
    type Error = &'static str;
    fn try_from(value: MemberProfile) -> Result<Self, Self::Error> {
        let key = ed25519_dalek::VerifyingKey::from_bytes(&value.signing_public_key)
            .map_err(|_| "invalid signing key")?;
        if nekodex_domain::signing::member_id_for_key(&key) != value.device_id {
            return Err("profile identity mismatch");
        }
        Ok(Self {
            device_id: value.device_id,
            name: value.name,
            signing_public_key: value.signing_public_key,
            noise_public_key: value.noise_public_key,
            codex_capable: value.codex_capable,
            protocol_version: value.protocol_version,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupProfile {
    pub group_id: String,
    pub name: String,
    pub local_device_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CandidateSource {
    Mdns,
    Remembered,
    Manual,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressCandidate {
    pub device_id: Option<String>,
    pub address: SocketAddr,
    pub source: CandidateSource,
    pub observed_at_unix_seconds: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Authenticating,
    SynchronizingMembership,
    Online,
    AuthenticationFailed,
    Incompatible,
}
