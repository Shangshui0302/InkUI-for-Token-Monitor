use codex_monitor_protocol::DiscoveryAnnouncement;
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::SocketAddr;

const MAX_DISCOVERIES: usize = 128;

#[derive(Clone)]
pub struct DiscoveredPeer {
    pub announcement: DiscoveryAnnouncement,
    pub address: SocketAddr,
    pub last_seen: u64,
    pairing_state: &'static str,
}

pub struct DiscoveryCache {
    peers: BTreeMap<String, DiscoveredPeer>,
    available: bool,
    reason: Option<String>,
}

impl Default for DiscoveryCache {
    fn default() -> Self {
        Self {
            peers: BTreeMap::new(),
            available: false,
            reason: Some("discovery listener is not ready".into()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryView {
    pub node_id: String,
    pub alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    pub last_seen: u64,
    pub expires_at: u64,
    pub pairing_state: &'static str,
    pub network_compatible: bool,
}

impl DiscoveryCache {
    pub fn set_available(&mut self) {
        self.available = true;
        self.reason = None;
    }
    pub fn available(&self) -> bool {
        self.available
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
    pub fn remember(&mut self, announcement: DiscoveryAnnouncement, address: SocketAddr, now: u64) {
        self.prune(now);
        if self.peers.len() >= MAX_DISCOVERIES && !self.peers.contains_key(&announcement.host_id) {
            if let Some(oldest) = self
                .peers
                .values()
                .min_by_key(|peer| peer.last_seen)
                .map(|peer| peer.announcement.host_id.clone())
            {
                self.peers.remove(&oldest);
            }
        }
        let pairing_state = self
            .peers
            .get(&announcement.host_id)
            .map_or("discovered", |peer| peer.pairing_state);
        self.peers.insert(
            announcement.host_id.clone(),
            DiscoveredPeer {
                announcement,
                address,
                last_seen: now,
                pairing_state,
            },
        );
    }

    pub fn get(&mut self, node_id: &str, now: u64) -> Option<DiscoveredPeer> {
        self.prune(now);
        self.peers.get(node_id).cloned()
    }

    pub fn views(&mut self, now: u64, network_id: &str) -> Vec<DiscoveryView> {
        self.prune(now);
        self.peers
            .values()
            .map(|peer| DiscoveryView {
                node_id: peer.announcement.host_id.clone(),
                alias: peer.announcement.alias.clone(),
                os: peer.announcement.os.clone(),
                last_seen: peer.last_seen,
                expires_at: peer.announcement.expires_at,
                pairing_state: peer.pairing_state,
                network_compatible: peer
                    .announcement
                    .network_id
                    .as_deref()
                    .map_or(true, |id| id == network_id),
            })
            .collect()
    }

    pub fn mark_connected(&mut self, node_id: &str) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            peer.pairing_state = "connected";
        }
    }

    pub fn mark_paired(&mut self, node_id: &str) {
        if let Some(peer) = self.peers.get_mut(node_id) {
            peer.pairing_state = "paired";
        }
    }

    fn prune(&mut self, now: u64) {
        self.peers
            .retain(|_, peer| peer.announcement.expires_at >= now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_monitor_protocol::{ANNOUNCEMENT_TTL_SECS, PROTOCOL_VERSION, SERVICE_NAME};

    fn announcement(id: &str, expires_at: u64) -> DiscoveryAnnouncement {
        let key = "11".repeat(32);
        DiscoveryAnnouncement {
            service: SERVICE_NAME.into(),
            version: PROTOCOL_VERSION,
            host_id: id.into(),
            alias: "peer".into(),
            os: None,
            port: 8787,
            http_port: None,
            public_key: key,
            nonce: "n".into(),
            issued_at: expires_at - ANNOUNCEMENT_TTL_SECS,
            expires_at,
            capabilities: vec![],
            network_id: None,
        }
    }

    #[test]
    fn expires_entries_before_lookup() {
        let mut cache = DiscoveryCache::default();
        cache.remember(announcement("peer", 20), "127.0.0.1:1".parse().unwrap(), 10);
        assert!(cache.get("peer", 21).is_none());
    }
}
