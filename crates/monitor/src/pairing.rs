use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PairingStore {
    path: PathBuf,
    hosts: BTreeMap<String, String>,
}

impl PairingStore {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let hosts = serde_json::from_str(&contents).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("pairing store: {error}"),
                    )
                })?;
                Ok(Self { path, hosts })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self {
                path,
                hosts: BTreeMap::new(),
            }),
            Err(error) => Err(error),
        }
    }

    pub fn public_key_for(&self, host_id: &str) -> Option<&str> {
        self.hosts.get(host_id).map(String::as_str)
    }

    pub fn remember(&mut self, host_id: &str, public_key: &str) -> io::Result<()> {
        self.hosts
            .insert(host_id.to_string(), public_key.to_string());
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_vec_pretty(&self.hosts)
            .map_err(|error| io::Error::other(format!("serialize pairing store: {error}")))?;
        // The file only contains public keys; a torn write is recoverable by re-pairing.
        // Direct replacement keeps the M1 prototype portable (Windows rename does not
        // overwrite an existing file).
        fs::write(&self.path, contents)
    }
}

pub fn default_pairing_path() -> PathBuf {
    if let Ok(path) = std::env::var("CODEX_MONITOR_PAIRING_PATH") {
        return PathBuf::from(path);
    }

    if cfg!(windows) {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return root.join("CodexMonitor").join("paired-hosts.json");
    }

    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    root.join("codex-monitor").join("paired-hosts.json")
}
