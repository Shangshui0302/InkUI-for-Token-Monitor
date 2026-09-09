use codex_monitor_protocol::{AccountState, CollectorStatus, SessionState, StateSnapshot};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const STALE_AFTER_SECS: u64 = 30;

#[derive(Debug, Default)]
pub struct Registry {
    hosts: BTreeMap<String, HostRecord>,
    revision: u64,
}

#[derive(Debug)]
struct HostRecord {
    alias: String,
    capabilities: Vec<String>,
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
    pub generated_at: u64,
    pub revision: u64,
    pub hosts: Vec<HostView>,
    pub accounts: Vec<AccountView>,
}

impl Registry {
    pub fn register(
        &mut self,
        host_id: String,
        alias: String,
        capabilities: Vec<String>,
        now: u64,
    ) {
        let host = self.hosts.entry(host_id).or_insert_with(|| HostRecord {
            alias: alias.clone(),
            capabilities: capabilities.clone(),
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
                capabilities: Vec::new(),
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
        host.connected = true;
        host.last_seen = received_at;
        host.last_snapshot_received_at = Some(received_at);
        host.last_sequence = snapshot.sequence;
        host.snapshot = Some(snapshot);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn state(&self, now: u64) -> MonitorState {
        let hosts: Vec<HostView> = self
            .hosts
            .iter()
            .map(|(host_id, host)| {
                let snapshot = host.snapshot.as_ref();
                let status = if !host.connected {
                    "offline"
                } else if snapshot.is_none() {
                    "unknown"
                } else if now.saturating_sub(host.last_snapshot_received_at.unwrap_or(now))
                    > STALE_AFTER_SECS
                {
                    "stale"
                } else {
                    "connected"
                };
                HostView {
                    host_id: host_id.clone(),
                    alias: host.alias.clone(),
                    status: status.to_string(),
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
            generated_at: now,
            revision: self.revision,
            hosts,
            accounts,
        }
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
                observed_at: sequence,
            }),
        }
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
}
