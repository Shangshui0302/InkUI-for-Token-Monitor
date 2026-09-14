use codex_monitor_protocol::{constant_time_hex_eq, pairing_code_proof};
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCodeView {
    pub code: String,
    pub generation: u64,
}

#[derive(Debug)]
struct PairingCodeState {
    code: String,
    generation: u64,
    reserved: bool,
}

/// Process-local one-time code state. Its short critical sections are
/// synchronous so a reservation can be safely released from `Drop` if a TCP
/// handshake exits early or is cancelled.
#[derive(Debug, Clone)]
pub struct PairingCodeManager(Arc<Mutex<PairingCodeState>>);

#[derive(Debug)]
pub struct PairingCodeReservation {
    manager: PairingCodeManager,
    generation: u64,
    active: bool,
}

#[derive(Debug)]
pub enum PairingCommitError {
    InvalidReservation,
    Store(io::Error),
}

impl PairingCodeManager {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(PairingCodeState {
            code: generate_code(),
            generation: 1,
            reserved: false,
        })))
    }

    /// Every authorized GET rotates first, making all older values invalid.
    pub fn rotate(&self) -> PairingCodeView {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.code = generate_code();
        state.generation = state.generation.wrapping_add(1);
        state.reserved = false;
        PairingCodeView {
            code: state.code.clone(),
            generation: state.generation,
        }
    }

    #[cfg(test)]
    pub fn reserve(&self, supplied: &str) -> Option<PairingCodeReservation> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.reserved || state.code != supplied {
            return None;
        }
        state.reserved = true;
        Some(PairingCodeReservation {
            manager: self.clone(),
            generation: state.generation,
            active: true,
        })
    }

    pub fn reserve_proof(
        &self,
        supplied_proof: &str,
        role: &str,
        transcript: &[u8],
    ) -> Option<PairingCodeReservation> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let expected = pairing_code_proof(&state.code, role, transcript);
        if state.reserved || !constant_time_hex_eq(&expected, supplied_proof) {
            return None;
        }
        state.reserved = true;
        Some(PairingCodeReservation {
            manager: self.clone(),
            generation: state.generation,
            active: true,
        })
    }

    pub fn reserved_proof(&self, role: &str, transcript: &[u8]) -> String {
        let state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        pairing_code_proof(&state.code, role, transcript)
    }

    /// Persist trust and consume the reserved code as one serialized action.
    /// A failed persistence releases the reservation without rotating the code.
    pub fn commit<T>(
        &self,
        mut reservation: PairingCodeReservation,
        persist: impl FnOnce() -> io::Result<T>,
    ) -> Result<T, PairingCommitError> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if !reservation.active
            || !Arc::ptr_eq(&self.0, &reservation.manager.0)
            || state.generation != reservation.generation
            || !state.reserved
        {
            reservation.active = false;
            return Err(PairingCommitError::InvalidReservation);
        }
        let persisted = persist();
        state.reserved = false;
        reservation.active = false;
        match persisted {
            Ok(value) => {
                state.code = generate_code();
                state.generation = state.generation.wrapping_add(1);
                Ok(value)
            }
            Err(error) => Err(PairingCommitError::Store(error)),
        }
    }
}

impl Default for PairingCodeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PairingCodeReservation {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self
            .manager
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.generation == self.generation {
            state.reserved = false;
        }
        self.active = false;
    }
}

pub fn generate_code() -> String {
    const DIGITS: &[u8] = b"0123456789";
    const UPPERCASE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const LOWERCASE: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

    let mut rng = rand::rngs::OsRng;
    let mut code = Vec::with_capacity(codex_monitor_protocol::PAIRING_CODE_LENGTH);
    code.push(DIGITS[rng.gen_range(0..DIGITS.len())]);
    code.push(UPPERCASE[rng.gen_range(0..UPPERCASE.len())]);
    code.push(LOWERCASE[rng.gen_range(0..LOWERCASE.len())]);
    for _ in 3..codex_monitor_protocol::PAIRING_CODE_LENGTH {
        code.push(ALPHANUMERIC[rng.gen_range(0..ALPHANUMERIC.len())]);
    }
    code.shuffle(&mut rng);
    String::from_utf8(code).expect("pairing alphabet is ASCII")
}

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
        let previous = self
            .hosts
            .insert(host_id.to_string(), public_key.to_string());
        if let Err(error) = self.save() {
            match previous {
                Some(value) => {
                    self.hosts.insert(host_id.to_string(), value);
                }
                None => {
                    self.hosts.remove(host_id);
                }
            }
            return Err(error);
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::{generate_code, PairingCodeManager};

    #[test]
    fn generated_codes_are_eight_mixed_ascii_characters() {
        for _ in 0..64 {
            let code = generate_code();
            assert_eq!(code.len(), 8);
            assert!(code.bytes().all(|byte| byte.is_ascii_alphanumeric()));
            assert!(code.bytes().any(|byte| byte.is_ascii_digit()));
            assert!(code.bytes().any(|byte| byte.is_ascii_uppercase()));
            assert!(code.bytes().any(|byte| byte.is_ascii_lowercase()));
        }
    }

    #[test]
    fn refresh_invalidates_old_code_and_increments_generation() {
        let manager = PairingCodeManager::new();
        let first = manager.rotate();
        let second = manager.rotate();
        assert_ne!(first.code, second.code);
        assert!(second.generation > first.generation);
        assert!(manager.reserve(&first.code).is_none());
        assert!(manager.reserve(&second.code).is_some());
    }

    #[test]
    fn code_proof_rejects_wrong_role_and_transcript_without_reserving() {
        use codex_monitor_protocol::{pairing_code_proof, pairing_transcript};
        let manager = PairingCodeManager::new();
        let code = manager.rotate().code;
        let transcript = pairing_transcript("a", "ka", "pa", "b", "kb", "pb", "net", "nonce");
        let proof = pairing_code_proof(&code, "initiator", &transcript);
        assert!(manager
            .reserve_proof(&proof, "acceptor", &transcript)
            .is_none());
        let changed = pairing_transcript("a", "ka", "pa", "b", "kb", "pb", "other", "nonce");
        assert!(manager
            .reserve_proof(&proof, "initiator", &changed)
            .is_none());
        let reservation = manager
            .reserve_proof(&proof, "initiator", &transcript)
            .unwrap();
        manager.commit(reservation, || Ok(())).unwrap();
        assert!(manager
            .reserve_proof(&proof, "initiator", &transcript)
            .is_none());
    }

    #[test]
    fn one_code_has_only_one_concurrent_reservation() {
        let manager = PairingCodeManager::new();
        let current = manager.rotate();
        let first = manager.reserve(&current.code).expect("first reservation");
        assert!(manager.reserve(&current.code).is_none());
        drop(first);
        assert!(manager.reserve(&current.code).is_some());
    }

    #[test]
    fn successful_commit_consumes_code_and_rotates_generation() {
        let manager = PairingCodeManager::new();
        let current = manager.rotate();
        let reservation = manager.reserve(&current.code).expect("reservation");
        manager.commit(reservation, || Ok(())).expect("commit");
        let state = manager.0.lock().unwrap();
        assert_ne!(current.code, state.code);
        assert_eq!(state.generation, current.generation + 1);
        assert!(!state.reserved);
        drop(state);
        assert!(manager.reserve(&current.code).is_none());
    }

    #[test]
    fn failed_commit_releases_but_does_not_consume_code() {
        let manager = PairingCodeManager::new();
        let current = manager.rotate();
        let reservation = manager.reserve(&current.code).expect("reservation");
        assert!(manager
            .commit(reservation, || Err::<(), _>(std::io::Error::other(
                "test failure"
            )))
            .is_err());
        assert!(manager.reserve(&current.code).is_some());
    }

    #[test]
    fn refresh_invalidates_an_in_flight_reservation() {
        let manager = PairingCodeManager::new();
        let current = manager.rotate();
        let reservation = manager.reserve(&current.code).expect("reservation");
        let refreshed = manager.rotate();
        assert!(manager.commit(reservation, || Ok(())).is_err());
        assert!(manager.reserve(&refreshed.code).is_some());
    }

    #[test]
    fn concurrent_attempts_with_same_code_reserve_exactly_once() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let manager = PairingCodeManager::new();
        let current = manager.rotate();
        let start = Arc::new(Barrier::new(8));
        let finish = Arc::new(Barrier::new(8));
        let attempts = (0..8)
            .map(|_| {
                let manager = manager.clone();
                let start = Arc::clone(&start);
                let finish = Arc::clone(&finish);
                let code = current.code.clone();
                thread::spawn(move || {
                    start.wait();
                    let reservation = manager.reserve(&code);
                    let succeeded = reservation.is_some();
                    finish.wait();
                    drop(reservation);
                    succeeded
                })
            })
            .collect::<Vec<_>>();
        let reserved = attempts
            .into_iter()
            .map(|attempt| attempt.join().unwrap())
            .filter(|reserved| *reserved)
            .count();
        assert_eq!(reserved, 1);
    }
}
