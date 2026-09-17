//! mDNS discovery, manual candidates, remembered addresses, and reconnect policy.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use crate::types::{AddressCandidate, CandidateSource, TRANSPORT_PROTOCOL_VERSION};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

/// The service name is deliberately independent from any device identity.
pub const MDNS_SERVICE_NAME: &str = "_nekodex-v2._tcp.local.";
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
pub const OFFLINE_AFTER: Duration = Duration::from_secs(45);
pub const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(30);
pub const MAX_MDNS_CANDIDATES: usize = 128;
pub const MAX_MDNS_HINT_CHARS: usize = 128;

/// The only data that may be placed in an mDNS TXT record.  None of this is
/// an authentication or trust decision; the authenticated handshake remains
/// the authority for identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MdnsAnnouncement {
    pub protocol_version: u16,
    pub device_hint: Option<String>,
    pub invitation_expires_at_unix_seconds: Option<u64>,
    pub port: u16,
}

impl MdnsAnnouncement {
    pub fn new(port: u16, device_hint: Option<String>) -> Self {
        Self {
            protocol_version: TRANSPORT_PROTOCOL_VERSION,
            device_hint,
            invitation_expires_at_unix_seconds: None,
            port,
        }
    }
    pub fn with_active_invitation(mut self, expires_at_unix_seconds: u64) -> Self {
        self.invitation_expires_at_unix_seconds = Some(expires_at_unix_seconds);
        self
    }
    pub fn to_txt(&self) -> Vec<(String, String)> {
        let mut t = vec![("v".into(), self.protocol_version.to_string())];
        if let Some(h) = &self.device_hint {
            t.push(("device".into(), h.clone()));
        }
        if let Some(expires_at) = self.invitation_expires_at_unix_seconds {
            t.push(("inviteUntil".into(), expires_at.to_string()));
        }
        t
    }
    pub fn from_txt(port: u16, version: Option<&str>, hint: Option<&str>) -> Option<Self> {
        let protocol_version = version?.parse().ok()?;
        if protocol_version != TRANSPORT_PROTOCOL_VERSION {
            return None;
        }
        Some(Self {
            protocol_version,
            device_hint: hint.map(str::to_owned),
            invitation_expires_at_unix_seconds: None,
            port,
        })
    }
}

pub struct MdnsAdvertiser {
    daemon: ServiceDaemon,
    fullname: String,
}
impl MdnsAdvertiser {
    pub fn start(
        instance: &str,
        host: &str,
        address: IpAddr,
        a: &MdnsAnnouncement,
    ) -> mdns_sd::Result<Self> {
        Self::start_many(instance, host, std::slice::from_ref(&address), a)
    }

    pub fn start_many(
        instance: &str,
        host: &str,
        addresses: &[IpAddr],
        a: &MdnsAnnouncement,
    ) -> mdns_sd::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let info = ServiceInfo::new(
            MDNS_SERVICE_NAME,
            instance,
            host,
            addresses,
            a.port,
            a.to_txt()
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>(),
        )?;
        let fullname = info.get_fullname().to_owned();
        daemon.register(info)?;
        Ok(Self { daemon, fullname })
    }
    pub fn stop(self) -> mdns_sd::Result<()> {
        let unregister_result = self.daemon.unregister(&self.fullname);
        let shutdown_result = self.daemon.shutdown().map(|_| ());
        unregister_result.and(shutdown_result)
    }
}
pub struct MdnsBrowser {
    daemon: ServiceDaemon,
    receiver: mdns_sd::Receiver<ServiceEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MdnsCandidate {
    /// The hint is display-only and must never be copied into an authenticated
    /// `device_id` before the Noise membership proof succeeds.
    pub device_hint: Option<String>,
    /// Public discovery metadata only. It narrows the chooser to devices that
    /// recently opened an invitation; the PAKE exchange remains authoritative.
    pub invitation_expires_at_unix_seconds: Option<u64>,
    pub address: AddressCandidate,
}

impl MdnsBrowser {
    pub fn start() -> mdns_sd::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let receiver = daemon.browse(MDNS_SERVICE_NAME)?;
        Ok(Self { daemon, receiver })
    }
    pub fn recv(&self) -> Option<ServiceEvent> {
        self.receiver.recv().ok()
    }
    pub fn recv_timeout(&self, duration: Duration) -> Option<ServiceEvent> {
        self.receiver.recv_timeout(duration).ok()
    }
    pub fn next_candidate(event: &ServiceEvent, now: i64) -> Vec<MdnsCandidate> {
        let ServiceEvent::ServiceResolved(i) = event else {
            return vec![];
        };
        let expected_version = TRANSPORT_PROTOCOL_VERSION.to_string();
        if i.get_property_val_str("v") != Some(expected_version.as_str()) {
            return vec![];
        }
        let hint = i.get_property_val_str("device").and_then(|value| {
            (value.chars().count() <= MAX_MDNS_HINT_CHARS).then(|| value.to_owned())
        });
        let invitation_expires_at_unix_seconds = i
            .get_property_val_str("inviteUntil")
            .and_then(|value| value.parse().ok());
        i.get_addresses()
            .iter()
            .take(MAX_MDNS_CANDIDATES)
            .map(|ip| MdnsCandidate {
                device_hint: hint.clone(),
                invitation_expires_at_unix_seconds,
                address: AddressCandidate {
                    device_id: None,
                    address: SocketAddr::new(*ip, i.get_port()),
                    source: CandidateSource::Mdns,
                    observed_at_unix_seconds: now,
                },
            })
            .collect()
    }
    pub fn stop(self) -> mdns_sd::Result<()> {
        // Try both cleanup operations even when the first one fails.  The
        // daemon is short-lived for discovery, so leaking either browse or
        // daemon state is more harmful than reporting the first error.
        let browse_result = self.daemon.stop_browse(MDNS_SERVICE_NAME);
        let shutdown_result = self.daemon.shutdown().map(|_| ());
        browse_result.and(shutdown_result)
    }
}

/// Discover nearby devices for at most `timeout`.
///
/// TXT properties are used only to filter for this protocol and to provide a
/// display hint and ignore devices without a currently advertised invitation;
/// they are not evidence of identity or trust. The caller must still
/// authenticate any returned candidate before using its hint as an id.
pub fn discover_nearby(
    timeout: Duration,
    observed_at_unix_seconds: i64,
) -> mdns_sd::Result<Vec<MdnsCandidate>> {
    let browser = MdnsBrowser::start()?;
    let deadline = Instant::now() + timeout;
    let mut candidates = Vec::new();

    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Some(event) = browser.recv_timeout(remaining) else {
            break;
        };
        candidates.extend(
            MdnsBrowser::next_candidate(&event, observed_at_unix_seconds)
                .into_iter()
                .filter(|candidate| {
                    candidate
                        .invitation_expires_at_unix_seconds
                        .is_some_and(|expires_at| {
                            expires_at > observed_at_unix_seconds.max(0) as u64
                        })
                }),
        );
    }

    // Address is the transport-level identity for unauthenticated mDNS
    // observations. Keep one candidate per address and make output ordering
    // deterministic for callers and UI rendering.
    candidates.sort_by_key(|candidate| (std::cmp::Reverse(candidate.address.observed_at_unix_seconds), candidate.address.address));
    candidates.dedup_by_key(|candidate| candidate.address.address);
    candidates.truncate(MAX_MDNS_CANDIDATES);

    browser.stop()?;
    Ok(candidates)
}

#[derive(Default)]
pub struct CandidateDirectory {
    candidates: HashMap<String, AddressCandidate>,
}

impl CandidateDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a candidate. Device ids are stable keys, so an address
    /// change never creates a new device. Address-only observations are keyed
    /// by address until authentication supplies the id.
    pub fn insert(&mut self, candidate: AddressCandidate) {
        let key = candidate
            .device_id
            .clone()
            .unwrap_or_else(|| candidate.address.to_string());
        match self.candidates.get(&key) {
            Some(old) if priority(old.source) > priority(candidate.source) => {}
            Some(old)
                if priority(old.source) == priority(candidate.source)
                    && old.observed_at_unix_seconds > candidate.observed_at_unix_seconds => {}
            _ => {
                self.candidates.insert(key, candidate);
            }
        }
    }

    pub fn remove_expired(&mut self, now_unix_seconds: i64, max_age_seconds: i64) {
        self.candidates.retain(|_, c| {
            now_unix_seconds.saturating_sub(c.observed_at_unix_seconds) <= max_age_seconds
        });
    }

    pub fn candidates(&self) -> Vec<AddressCandidate> {
        let mut result: Vec<_> = self.candidates.values().cloned().collect();
        result.sort_by_key(|c| (std::cmp::Reverse(priority(c.source)), c.address));
        result
    }
}

fn priority(source: CandidateSource) -> u8 {
    match source {
        CandidateSource::Manual => 3,
        CandidateSource::Mdns => 2,
        CandidateSource::Remembered => 1,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Liveness {
    Online,
    Offline,
}

#[derive(Clone, Debug)]
pub struct ReconnectPolicy {
    failures: u32,
    next_delay: Duration,
    jitter: Duration,
    last_heartbeat: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            failures: 0,
            next_delay: Duration::ZERO,
            jitter: Duration::ZERO,
            last_heartbeat: Duration::ZERO,
        }
    }
}

impl ReconnectPolicy {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_jitter(mut self, jitter: Duration) -> Self {
        self.jitter = jitter;
        self
    }
    pub fn heartbeat_due(&self, elapsed: Duration) -> bool {
        elapsed >= self.last_heartbeat + HEARTBEAT_INTERVAL
    }
    pub fn mark_heartbeat(&mut self, elapsed: Duration) {
        self.last_heartbeat = elapsed;
    }
    pub fn liveness(elapsed_since_heartbeat: Duration) -> Liveness {
        if elapsed_since_heartbeat >= OFFLINE_AFTER {
            Liveness::Offline
        } else {
            Liveness::Online
        }
    }
    pub fn record_failure(&mut self) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let exp = 1u64
            .checked_shl(self.failures.saturating_sub(1).min(5))
            .unwrap_or(32);
        self.next_delay = (Duration::from_secs(exp) + self.jitter).min(MAX_RECONNECT_BACKOFF);
        self.next_delay
    }
    pub fn record_success(&mut self) {
        self.failures = 0;
        self.next_delay = Duration::ZERO;
    }
    pub fn failures(&self) -> u32 {
        self.failures
    }
    pub fn next_delay(&self) -> Duration {
        self.next_delay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn c(source: CandidateSource, id: Option<&str>, port: u16, t: i64) -> AddressCandidate {
        AddressCandidate {
            device_id: id.map(str::to_owned),
            address: SocketAddr::from(([127, 0, 0, 1], port)),
            source,
            observed_at_unix_seconds: t,
        }
    }
    #[test]
    fn priority_and_dedup() {
        let mut d = CandidateDirectory::new();
        d.insert(c(CandidateSource::Mdns, Some("x"), 1, 1));
        d.insert(c(CandidateSource::Manual, Some("x"), 2, 2));
        assert_eq!(d.candidates()[0].address.port(), 2);
    }
    #[test]
    fn expiry_and_boundaries() {
        let mut d = CandidateDirectory::new();
        d.insert(c(CandidateSource::Mdns, None, 1, 0));
        d.remove_expired(45, 45);
        assert_eq!(d.candidates().len(), 1);
        assert_eq!(
            ReconnectPolicy::liveness(Duration::from_secs(45)),
            Liveness::Offline
        );
    }
    #[test]
    fn backoff_caps_and_success_resets() {
        let mut p = ReconnectPolicy::new().with_jitter(Duration::from_secs(1));
        for _ in 0..20 {
            p.record_failure();
        }
        assert!(p.next_delay() <= MAX_RECONNECT_BACKOFF);
        p.record_success();
        assert_eq!(p.next_delay(), Duration::ZERO);
        assert_eq!(p.failures(), 0);
    }

    #[test]
    fn incompatible_or_untrusted_mdns_metadata_never_becomes_identity() {
        let announcement = MdnsAnnouncement::new(8787, Some("claimed-device".into()));
        assert_eq!(
            MdnsAnnouncement::from_txt(
                8787,
                Some(&TRANSPORT_PROTOCOL_VERSION.to_string()),
                Some("claimed-device")
            ),
            Some(announcement)
        );
        assert!(MdnsAnnouncement::from_txt(8787, Some("not-a-version"), None).is_none());
    }

    /// Real multicast smoke test. It is ignored in the normal suite because
    /// CI and Android emulators may block multicast; run explicitly on a LAN.
    #[test]
    #[ignore = "requires a multicast-capable network interface"]
    fn real_mdns_advertise_and_discover() {
        let browser = MdnsBrowser::start().unwrap();
        let hint = format!("probe-{}", std::process::id());
        let announcement = MdnsAnnouncement::new(18791, Some(hint.clone()));
        let advertiser = MdnsAdvertiser::start(
            &hint,
            "nekodex-probe.local.",
            "127.0.0.1".parse().unwrap(),
            &announcement,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut discovered = None;
        while Instant::now() < deadline {
            let Some(event) = browser.recv_timeout(Duration::from_millis(500)) else {
                continue;
            };
            discovered = MdnsBrowser::next_candidate(&event, 1)
                .into_iter()
                .find(|candidate| candidate.device_hint.as_deref() == Some(hint.as_str()));
            if discovered.is_some() {
                break;
            }
        }
        let discovered = discovered.expect("advertised service was not discovered");
        assert_eq!(discovered.address.address.port(), 18791);
        assert_eq!(discovered.address.device_id, None);
        advertiser.stop().unwrap();
        browser.stop().unwrap();
    }
}
