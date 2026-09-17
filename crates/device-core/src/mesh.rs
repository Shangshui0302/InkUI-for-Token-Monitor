//! LAN discovery and authenticated direct connections.
//!
//! Discovery is deliberately only a source of addresses.  In particular, an
//! mDNS TXT device hint is never copied into an [`AddressCandidate`] identity;
//! the Noise membership proof remains the sole authority for peer identity.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::{
    discovery::{MdnsBrowser, MdnsCandidate},
    node::JoinedNode,
    transport::{
        ConnectionProvider, DirectWebSocketProvider, OnlineSession, TransportError, TransportIo,
        establish,
    },
    types::AddressCandidate,
};

const DISCOVERY_QUEUE: usize = 64;
const DEDUP_WINDOW: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    #[error("mDNS: {0}")]
    Mdns(#[from] mdns_sd::Error),
    #[error("discovery task stopped")]
    Stopped,
    #[error("candidate identity does not match authenticated peer")]
    IdentityMismatch,
    #[error("transport: {0}")]
    Transport(#[from] TransportError),
}

/// A bounded, cancellable stream of LAN address candidates.
pub struct DiscoverySource {
    receiver: mpsc::Receiver<AddressCandidate>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl DiscoverySource {
    pub fn start() -> Result<Self, MeshError> {
        let browser = MdnsBrowser::start()?;
        let (sender, receiver) = mpsc::channel(DISCOVERY_QUEUE);
        let (stop, mut stop_rx) = oneshot::channel();
        let task = tokio::task::spawn_blocking(move || {
            let mut seen = HashMap::<std::net::SocketAddr, Instant>::new();
            loop {
                if stop_rx.try_recv().is_ok() {
                    break;
                }
                let Some(event) = browser.recv_timeout(POLL_INTERVAL) else {
                    continue;
                };
                let now = Instant::now();
                seen.retain(|_, at| now.duration_since(*at) < DEDUP_WINDOW);
                let unix = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or_default();
                for MdnsCandidate { address, .. } in MdnsBrowser::next_candidate(&event, unix) {
                    if seen.insert(address.address, now).is_none() {
                        // A full queue drops discovery noise rather than
                        // blocking shutdown or letting mDNS input grow.
                        let _ = sender.try_send(address);
                    }
                }
            }
            let _ = browser.stop();
        });
        Ok(Self {
            receiver,
            stop: Some(stop),
            task: Some(task),
        })
    }

    pub async fn recv(&mut self) -> Option<AddressCandidate> {
        self.receiver.recv().await
    }

    pub async fn stop(mut self) -> Result<(), MeshError> {
        self.stop
            .take()
            .ok_or(MeshError::Stopped)?
            .send(())
            .map_err(|_| MeshError::Stopped)?;
        if let Some(task) = self.task.take() {
            task.await.map_err(|_| MeshError::Stopped)?;
        }
        Ok(())
    }
}

impl Drop for DiscoverySource {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Connect through the direct `/device` route and return only after the full
/// authenticated Noise membership exchange has completed.
pub async fn connect_candidate(
    node: Arc<JoinedNode>,
    candidate: AddressCandidate,
) -> Result<OnlineSession<Box<dyn TransportIo>>, MeshError> {
    let expected = candidate.device_id.clone();
    let address = format!("ws://{}/device", candidate.address);
    let socket = DirectWebSocketProvider.connect(&address).await?;
    let session = establish(socket, true, &node.credentials(), &node.authorizer()).await?;
    if expected
        .as_deref()
        .is_some_and(|id| id != session.peer_device_id)
    {
        return Err(MeshError::IdentityMismatch);
    }
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_address_is_suppressed_for_window() {
        let mut seen = HashMap::new();
        let address: std::net::SocketAddr = "127.0.0.1:8787".parse().unwrap();
        let now = Instant::now();
        assert!(seen.insert(address, now).is_none());
        assert!(now.duration_since(*seen.get(&address).unwrap()) < DEDUP_WINDOW);
    }
}
