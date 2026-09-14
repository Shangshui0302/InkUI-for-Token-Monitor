use codex_monitor_protocol::DiscoveryAnnouncement;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
struct TrustedMonitor {
    host_id: String,
    public_key: String,
}

#[derive(Debug)]
pub struct MonitorTrustStore {
    path: PathBuf,
    monitor: Option<TrustedMonitor>,
}

impl MonitorTrustStore {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let monitor = serde_json::from_str(&contents).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("monitor trust store: {error}"),
                    )
                })?;
                Ok(Self {
                    path,
                    monitor: Some(monitor),
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self {
                path,
                monitor: None,
            }),
            Err(error) => Err(error),
        }
    }

    pub fn accepts(&self, announcement: &DiscoveryAnnouncement) -> bool {
        self.monitor.as_ref().is_some_and(|trusted| {
            trusted.host_id == announcement.host_id
                && (trusted.public_key == announcement.public_key
                    || announcement.pairing_public_key.as_deref()
                        == Some(trusted.public_key.as_str()))
        })
    }

    pub fn public_key_for(&self, announcement: &DiscoveryAnnouncement) -> Option<&str> {
        self.monitor
            .as_ref()
            .filter(|trusted| trusted.host_id == announcement.host_id)
            .map(|trusted| trusted.public_key.as_str())
    }

    pub fn remember(
        &mut self,
        announcement: &DiscoveryAnnouncement,
        handshake_public_key: &str,
    ) -> io::Result<()> {
        self.monitor = Some(TrustedMonitor {
            host_id: announcement.host_id.clone(),
            public_key: handshake_public_key.to_string(),
        });
        self.save()
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_vec_pretty(self.monitor.as_ref().expect("trusted monitor"))
            .map_err(|error| {
            io::Error::other(format!("serialize monitor trust store: {error}"))
        })?;
        // A torn trust file only requires deleting it and pairing again.
        fs::write(&self.path, contents)
    }
}

pub fn default_monitor_trust_path() -> PathBuf {
    if let Ok(path) = std::env::var("CODEX_MONITOR_TRUST_PATH") {
        return PathBuf::from(path);
    }
    codex_monitor_protocol::default_identity_path("agent").with_file_name("trusted-monitor.json")
}
