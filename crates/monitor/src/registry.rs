use codex_monitor_protocol::{AccountState, CollectorStatus, SessionState, StateSnapshot};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::PathBuf;

const STALE_AFTER_SECS: u64 = 30;
const API_VERSION: u16 = 1;
const MAX_SAFE_CURSOR: u64 = (1u64 << 53) - 1;

#[derive(Debug, Default)]
pub struct Registry {
    hosts: BTreeMap<String, HostRecord>,
    revision: u64,
    identity: Option<IdentityView>,
    cache_path: Option<PathBuf>,
}

#[derive(Debug)]
struct HostRecord {
    alias: String,
    os: Option<String>,
    capabilities: Vec<String>,
    http_endpoint: Option<String>,
    http_endpoints: Vec<String>,
    connected: bool,
    last_seen: u64,
    last_snapshot_received_at: Option<u64>,
    last_sequence: u64,
    snapshot: Option<StateSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    pub host_id: String,
    pub alias: String,
    pub os: Option<String>,
    pub status: String,
    pub capabilities: Vec<String>,
    pub last_seen: u64,
    pub last_snapshot_at: Option<u64>,
    pub collector_status: Option<CollectorStatus>,
    pub error: Option<String>,
    pub sessions: Vec<SessionState>,
    pub account: Option<AccountState>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub account_key: String,
    pub account: AccountState,
    pub source_hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorState {
    pub api_version: u16,
    pub generated_at: u64,
    pub revision: u64,
    pub cursor: u64,
    pub hosts: Vec<HostView>,
    pub accounts: Vec<AccountView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentityView>,
    #[serde(default)]
    pub peers: Vec<PeerView>,
}

/// Public identity metadata. It intentionally contains no private key or account data.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityView {
    pub node_id: Option<String>,
    pub alias: Option<String>,
    pub os: Option<String>,
    pub network_id: Option<String>,
    pub capabilities: Vec<String>,
}

/// A source node known by this registry. State remains source-owned; this view only
/// exposes liveness and the latest observation metadata.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    pub node_id: String,
    pub alias: String,
    pub os: Option<String>,
    pub status: String,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http_endpoints: Vec<String>,
    pub last_seen: u64,
    pub last_snapshot_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    pub status: &'static str,
    pub generated_at: u64,
    pub revision: u64,
    pub peer_count: usize,
    pub stale_peer_count: usize,
}

impl Registry {
    /// Load the last source-owned snapshots. Runtime liveness is intentionally
    /// reset to offline until the node reconnects, so a restart never pretends
    /// that cached data is live.
    pub fn load(cache_path: PathBuf, network_id: Option<&str>) -> io::Result<Self> {
        let mut registry = Self {
            cache_path: Some(cache_path.clone()),
            ..Self::default()
        };
        let contents = match fs::read_to_string(&cache_path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(registry),
            Err(error) => return Err(error),
        };
        let snapshots: BTreeMap<String, StateSnapshot> = match serde_json::from_str(&contents) {
            Ok(snapshots) => snapshots,
            Err(error) => {
                eprintln!(
                    "ignoring invalid state cache {}: {error}",
                    cache_path.display()
                );
                return Ok(registry);
            }
        };
        for (host_id, snapshot) in snapshots {
            if host_id != snapshot.host_id
                || snapshot.source_id() != host_id
                || snapshot.validate().is_err()
                || network_id
                    .is_some_and(|expected| snapshot.network_id.as_deref() != Some(expected))
            {
                continue;
            }
            registry.hosts.insert(
                host_id,
                HostRecord {
                    alias: snapshot.alias.clone(),
                    os: snapshot.os.clone(),
                    capabilities: Vec::new(),
                    http_endpoint: None,
                    http_endpoints: Vec::new(),
                    connected: false,
                    last_seen: snapshot.captured_at,
                    last_snapshot_received_at: Some(snapshot.captured_at),
                    last_sequence: snapshot.sequence,
                    snapshot: Some(snapshot),
                },
            );
        }
        Ok(registry)
    }

    /// Set the local node metadata used by the v1 API. The old registry API does
    /// not require identity metadata, so it remains optional for compatibility.
    pub fn set_identity(
        &mut self,
        node_id: String,
        alias: String,
        network_id: Option<String>,
        capabilities: Vec<String>,
    ) {
        self.identity = Some(IdentityView {
            node_id: Some(node_id.clone()),
            alias: Some(alias.clone()),
            os: Some(std::env::consts::OS.to_string()),
            network_id,
            capabilities: capabilities.clone(),
        });
        if let Some(host) = self.hosts.get_mut(&node_id) {
            host.alias = alias;
            host.capabilities = capabilities;
        }
        self.revision = self.revision.saturating_add(1);
    }

    pub fn identity(&self) -> IdentityView {
        self.identity.clone().unwrap_or_default()
    }

    pub fn peers(&self, now: u64) -> Vec<PeerView> {
        let local_node_id = self
            .identity
            .as_ref()
            .and_then(|identity| identity.node_id.as_deref());
        self.hosts
            .iter()
            .filter(|(node_id, _)| Some(node_id.as_str()) != local_node_id)
            .map(|(node_id, host)| PeerView {
                node_id: node_id.clone(),
                alias: host.alias.clone(),
                os: host.os.clone(),
                status: host_status(host, now).to_string(),
                capabilities: host.capabilities.clone(),
                http_endpoint: host.http_endpoint.clone(),
                http_endpoints: host.http_endpoints.clone(),
                last_seen: host.last_seen,
                last_snapshot_at: host.snapshot.as_ref().map(|value| value.captured_at),
            })
            .collect()
    }

    pub fn health(&self, now: u64) -> HealthView {
        let peers = self.peers(now);
        HealthView {
            status: "ok",
            generated_at: now,
            revision: self.revision,
            peer_count: peers.len(),
            stale_peer_count: peers
                .iter()
                .filter(|peer| peer.status == "stale" || peer.status == "offline")
                .count(),
        }
    }

    pub fn register(
        &mut self,
        host_id: String,
        alias: String,
        capabilities: Vec<String>,
        now: u64,
    ) {
        let host = self.hosts.entry(host_id).or_insert_with(|| HostRecord {
            alias: alias.clone(),
            os: None,
            capabilities: capabilities.clone(),
            http_endpoint: None,
            http_endpoints: Vec::new(),
            connected: false,
            last_seen: now,
            last_snapshot_received_at: None,
            last_sequence: 0,
            snapshot: None,
        });
        if !host.connected {
            // A collector sequence is scoped to one authenticated connection.
            // Preserve the last snapshot for display, but accept the new connection's
            // sequence starting at one.
            host.last_sequence = 0;
        }
        host.alias = alias;
        host.capabilities = capabilities;
        host.connected = true;
        host.last_seen = now;
        self.revision = self.revision.saturating_add(1);
    }

    pub fn set_http_endpoints(&mut self, host_id: &str, endpoints: Vec<String>) {
        let mut endpoints = endpoints
            .into_iter()
            .filter(|endpoint| !endpoint.is_empty())
            .collect::<Vec<_>>();
        endpoints.dedup();
        endpoints.truncate(8);
        if let Some(host) = self.hosts.get_mut(host_id) {
            let primary = endpoints.first().cloned();
            if host.http_endpoint != primary || host.http_endpoints != endpoints {
                host.http_endpoint = primary;
                host.http_endpoints = endpoints;
                self.revision = self.revision.saturating_add(1);
            }
        }
    }

    pub fn set_os(&mut self, host_id: &str, os: Option<String>) {
        if let Some(host) = self.hosts.get_mut(host_id) {
            host.os = os;
        }
    }

    pub fn touch(&mut self, host_id: &str, now: u64) {
        if let Some(host) = self.hosts.get_mut(host_id) {
            host.connected = true;
            host.last_seen = now;
            self.revision = self.revision.saturating_add(1);
        }
    }

    pub fn disconnect(&mut self, host_id: &str, now: u64) {
        if let Some(host) = self.hosts.get_mut(host_id) {
            host.connected = false;
            host.last_seen = now;
            self.revision = self.revision.saturating_add(1);
        }
    }

    pub fn update_snapshot(
        &mut self,
        snapshot: StateSnapshot,
        received_at: u64,
    ) -> Result<(), &'static str> {
        snapshot.validate()?;
        let host = self
            .hosts
            .entry(snapshot.host_id.clone())
            .or_insert_with(|| HostRecord {
                alias: snapshot.alias.clone(),
                os: snapshot.os.clone(),
                capabilities: Vec::new(),
                http_endpoint: None,
                http_endpoints: Vec::new(),
                connected: true,
                last_seen: received_at,
                last_snapshot_received_at: None,
                last_sequence: 0,
                snapshot: None,
            });
        if snapshot.sequence <= host.last_sequence {
            return Ok(());
        }
        host.alias = snapshot.alias.clone();
        host.os = snapshot.os.clone();
        host.connected = true;
        host.last_seen = received_at;
        // Use the source timestamp for stale state so every peer derives the
        // same liveness view after receiving the same snapshot.
        host.last_snapshot_received_at = Some(snapshot.captured_at);
        host.last_sequence = snapshot.sequence;
        host.snapshot = Some(snapshot);
        self.revision = self.revision.saturating_add(1);
        if let Err(error) = self.persist_cache() {
            eprintln!("state cache write failed: {error}");
        }
        Ok(())
    }

    fn persist_cache(&self) -> io::Result<()> {
        let Some(path) = self.cache_path.as_ref() else {
            return Ok(());
        };
        let snapshots: BTreeMap<&str, &StateSnapshot> = self
            .hosts
            .iter()
            .filter_map(|(host_id, host)| {
                host.snapshot
                    .as_ref()
                    .map(|snapshot| (host_id.as_str(), snapshot))
            })
            .collect();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&snapshots)
            .map_err(|error| io::Error::other(format!("serialize state cache: {error}")))?;
        // The cache is recoverable local state; a direct write keeps Windows and
        // Unix behavior identical without adding a storage dependency.
        fs::write(path, bytes)
    }

    pub fn state(&self, now: u64) -> MonitorState {
        let hosts: Vec<HostView> = self
            .hosts
            .iter()
            .map(|(host_id, host)| {
                let snapshot = host.snapshot.as_ref();
                HostView {
                    host_id: host_id.clone(),
                    alias: host.alias.clone(),
                    os: host.os.clone(),
                    status: host_status(host, now).to_string(),
                    capabilities: host.capabilities.clone(),
                    last_seen: host.last_seen,
                    last_snapshot_at: snapshot.map(|value| value.captured_at),
                    collector_status: snapshot.map(|value| value.collector_status.clone()),
                    error: snapshot.and_then(|value| value.error.clone()),
                    sessions: snapshot
                        .map(|value| value.sessions.clone())
                        .unwrap_or_default(),
                    account: snapshot.and_then(|value| value.account.clone()),
                }
            })
            .collect();

        let mut accounts: BTreeMap<String, AccountAccumulator> = BTreeMap::new();
        for (host_id, host) in &self.hosts {
            let Some(account) = host
                .snapshot
                .as_ref()
                .and_then(|value| value.account.as_ref())
            else {
                continue;
            };
            let key = account
                .account_key
                .clone()
                .unwrap_or_else(|| format!("host:{host_id}"));
            let entry = accounts
                .entry(key.clone())
                .or_insert_with(|| AccountAccumulator {
                    account: account.clone(),
                    source_hosts: BTreeSet::new(),
                });
            entry.source_hosts.insert(host_id.clone());
            if account.observed_at >= entry.account.observed_at {
                entry.account = account.clone();
            }
        }
        let accounts = accounts
            .into_iter()
            .map(|(account_key, value)| AccountView {
                account_key,
                account: value.account,
                source_hosts: value.source_hosts.into_iter().collect(),
            })
            .collect();

        MonitorState {
            api_version: API_VERSION,
            generated_at: now,
            revision: self.revision,
            cursor: self.state_cursor(now),
            hosts,
            accounts,
            identity: self.identity.clone(),
            peers: self.peers(now),
        }
    }

    fn state_cursor(&self, now: u64) -> u64 {
        // The cursor is opaque to clients. Include derived liveness so an equal
        // mutation revision does not hide a connected -> stale transition.
        let mut cursor = 14_695_981_039_346_656_037u64 ^ self.revision;
        for (host_id, host) in &self.hosts {
            for byte in host_id
                .as_bytes()
                .iter()
                .chain(host_status(host, now).as_bytes())
            {
                cursor ^= u64::from(*byte);
                cursor = cursor.wrapping_mul(1_099_511_628_211);
            }
        }
        // Keep the opaque cursor exactly representable by JavaScript clients;
        // the API still accepts the value as a u64 for non-JS consumers.
        cursor & MAX_SAFE_CURSOR
    }
}

pub type NetworkView = MonitorState;

fn host_status(host: &HostRecord, now: u64) -> &'static str {
    if !host.connected {
        "offline"
    } else if host.snapshot.is_none() {
        "unknown"
    } else if now.saturating_sub(host.last_snapshot_received_at.unwrap_or(now)) > STALE_AFTER_SECS {
        "stale"
    } else {
        "connected"
    }
}

#[derive(Debug)]
struct AccountAccumulator {
    account: AccountState,
    source_hosts: BTreeSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_monitor_protocol::CollectorStatus;

    fn snapshot(host_id: &str, account_key: Option<&str>, sequence: u64) -> StateSnapshot {
        StateSnapshot {
            host_id: host_id.to_string(),
            alias: host_id.to_string(),
            os: None,
            sequence,
            captured_at: sequence,
            collector_status: CollectorStatus::Ready,
            error: None,
            sessions: Vec::new(),
            account: account_key.map(|key| AccountState {
                account_key: Some(key.to_string()),
                auth_mode: Some("chatgpt".to_string()),
                plan_type: Some("plus".to_string()),
                requires_openai_auth: true,
                rate_limits: Vec::new(),
                reset_credits_available: None,
                usage_lifetime_tokens: None,
                usage_peak_daily_tokens: None,
                daily_usage: Vec::new(),
                observed_at: sequence,
            }),
            network_id: Some("network-a".to_string()),
            source_node_id: Some(host_id.to_string()),
        }
    }

    #[test]
    fn renaming_identity_preserves_node_id() {
        let mut registry = Registry::default();
        registry.set_identity(
            "node-a".into(),
            "old".into(),
            Some("network".into()),
            vec![],
        );
        let before = registry.identity().node_id;
        registry.set_identity(
            "node-a".into(),
            "new".into(),
            Some("network".into()),
            vec![],
        );
        assert_eq!(registry.identity().node_id, before);
        assert_eq!(registry.identity().alias.as_deref(), Some("new"));
    }

    #[test]
    fn same_account_is_aggregated_without_adding_limits() {
        let mut registry = Registry::default();
        registry.register("host-a".to_string(), "A".to_string(), Vec::new(), 1);
        registry.register("host-b".to_string(), "B".to_string(), Vec::new(), 1);
        registry
            .update_snapshot(snapshot("host-a", Some("same"), 1), 2)
            .unwrap();
        registry
            .update_snapshot(snapshot("host-b", Some("same"), 2), 3)
            .unwrap();

        let state = registry.state(4);
        assert_eq!(state.accounts.len(), 1);
        assert_eq!(state.accounts[0].source_hosts, vec!["host-a", "host-b"]);
    }

    #[test]
    fn stale_state_keeps_the_last_snapshot() {
        let mut registry = Registry::default();
        registry.register("host-a".to_string(), "A".to_string(), Vec::new(), 1);
        registry
            .update_snapshot(snapshot("host-a", None, 1), 2)
            .unwrap();
        let state = registry.state(33);
        assert_eq!(state.hosts[0].status, "stale");
        assert_eq!(state.hosts[0].alias, "host-a");
    }

    #[test]
    fn reconnect_accepts_a_new_collector_sequence() {
        let mut registry = Registry::default();
        registry.register("host-a".to_string(), "A".to_string(), Vec::new(), 1);
        registry
            .update_snapshot(snapshot("host-a", None, 4), 5)
            .unwrap();
        registry.disconnect("host-a", 6);
        registry.register("host-a".to_string(), "A".to_string(), Vec::new(), 7);
        registry
            .update_snapshot(snapshot("host-a", None, 1), 8)
            .unwrap();

        let state = registry.state(8);
        assert_eq!(state.hosts[0].status, "connected");
        assert_eq!(state.hosts[0].last_snapshot_at, Some(1));
    }

    #[test]
    fn peer_endpoint_is_exposed_for_failover() {
        let mut registry = Registry::default();
        registry.register("host-a".to_string(), "A".to_string(), Vec::new(), 1);
        registry.set_http_endpoints(
            "host-a",
            vec![
                "http://10.0.0.2:8788".to_string(),
                "http://host-a:8788".to_string(),
            ],
        );
        assert_eq!(
            registry.peers(1)[0].http_endpoint.as_deref(),
            Some("http://10.0.0.2:8788")
        );
        assert_eq!(
            registry.peers(1)[0].http_endpoints,
            vec!["http://10.0.0.2:8788", "http://host-a:8788"]
        );
    }

    #[test]
    fn cache_restores_snapshots_as_offline_after_restart() {
        let path = std::env::temp_dir().join(format!(
            "codex-monitor-state-cache-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let mut registry = Registry::load(path.clone(), Some("network-a")).unwrap();
        registry.register("host-a".to_string(), "A".to_string(), Vec::new(), 1);
        registry
            .update_snapshot(snapshot("host-a", None, 1), 2)
            .unwrap();

        let restored = Registry::load(path.clone(), Some("network-a")).unwrap();
        let state = restored.state(3);
        assert_eq!(state.hosts[0].status, "offline");
        assert_eq!(state.hosts[0].last_snapshot_at, Some(1));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cache_does_not_cross_network_boundaries() {
        let path = std::env::temp_dir().join(format!(
            "codex-monitor-network-cache-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let mut registry = Registry::load(path.clone(), Some("network-a")).unwrap();
        registry.register("host-a".to_string(), "A".to_string(), Vec::new(), 1);
        registry
            .update_snapshot(snapshot("host-a", None, 1), 2)
            .unwrap();

        let restored = Registry::load(path.clone(), Some("network-b")).unwrap();
        assert!(restored.state(3).hosts.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn state_cursor_changes_when_liveness_becomes_stale() {
        let mut registry = Registry::default();
        registry.register("host-a".to_string(), "A".to_string(), Vec::new(), 1);
        registry
            .update_snapshot(snapshot("host-a", None, 1), 1)
            .unwrap();
        assert_ne!(registry.state(1).cursor, registry.state(32).cursor);
    }

    #[test]
    fn state_cursor_fits_javascript_safe_integer_range() {
        let registry = Registry::default();
        assert!(registry.state(1).cursor <= MAX_SAFE_CURSOR);
    }
}
