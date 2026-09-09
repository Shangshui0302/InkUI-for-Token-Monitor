//! Shared wire types for the Codex Monitor agent and monitor.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const SERVICE_NAME: &str = "codex-monitor";

/// Public discovery data. Credentials and Codex account data never belong here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveryAnnouncement {
    pub service: String,
    pub version: u16,
    pub host_id: String,
    pub alias: String,
    pub port: u16,
    pub public_key: String,
    pub capabilities: Vec<String>,
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
            capabilities: vec!["sessions".to_string()],
        };

        let json = serde_json::to_string(&value).expect("serialize announcement");
        let decoded: DiscoveryAnnouncement =
            serde_json::from_str(&json).expect("deserialize announcement");
        assert_eq!(decoded, value);
    }
}
