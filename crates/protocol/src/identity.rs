use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::Serialize;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::MessageEnvelope;

/// A locally stored Ed25519 identity. The private key never belongs in a wire message.
pub struct Identity {
    signing_key: SigningKey,
}

impl Identity {
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "identity key length")
                })?;
                return Ok(Self {
                    signing_key: SigningKey::from_bytes(&key),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let identity = Self {
            signing_key: SigningKey::generate(&mut OsRng),
        };
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, identity.signing_key.to_bytes())?;
        fs::rename(&temporary, path)?;
        set_private_permissions(path)?;
        Ok(identity)
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    pub fn host_id(&self) -> String {
        Self::host_id_from_public_key(&self.public_key_hex()).expect("identity public key is valid")
    }

    pub fn host_id_from_public_key(public_key: &str) -> Option<String> {
        if public_key.len() != 64 || hex::decode(public_key).ok()?.len() != 32 {
            return None;
        }
        Some(format!("ed25519:{}", &public_key[..16]))
    }

    pub fn sign_hex(&self, message: &[u8]) -> String {
        hex::encode(self.signing_key.sign(message).to_bytes())
    }

    pub fn verify_hex(public_key: &str, message: &[u8], signature: &str) -> bool {
        let Ok(public_key) = hex::decode(public_key) else {
            return false;
        };
        let Ok(signature) = hex::decode(signature) else {
            return false;
        };
        let Ok(public_key) = <[u8; 32]>::try_from(public_key.as_slice()) else {
            return false;
        };
        let Ok(signature) = Signature::from_slice(&signature) else {
            return false;
        };
        let Ok(verifying_key) = VerifyingKey::from_bytes(&public_key) else {
            return false;
        };
        verifying_key.verify(message, &signature).is_ok()
    }

    /// Sign the canonical envelope bytes, leaving the envelope metadata and
    /// payload visible to receivers while excluding the signature itself.
    pub fn sign_envelope<T: Serialize>(
        &self,
        envelope: &mut MessageEnvelope<T>,
    ) -> Result<(), serde_json::Error> {
        envelope.signature = Some(self.sign_hex(&envelope.signing_bytes()?));
        Ok(())
    }

    pub fn signed_envelope<T: Serialize>(
        &self,
        mut envelope: MessageEnvelope<T>,
    ) -> Result<MessageEnvelope<T>, serde_json::Error> {
        self.sign_envelope(&mut envelope)?;
        Ok(envelope)
    }

    /// Verify an envelope against the public key belonging to its sender.
    pub fn verify_envelope<T: Serialize>(public_key: &str, envelope: &MessageEnvelope<T>) -> bool {
        let Some(signature) = envelope.signature.as_deref() else {
            return false;
        };
        let Ok(message) = envelope.signing_bytes() else {
            return false;
        };
        Self::verify_hex(public_key, &message, signature)
    }
}

pub fn sign_envelope<T: Serialize>(
    identity: &Identity,
    envelope: &mut MessageEnvelope<T>,
) -> Result<(), serde_json::Error> {
    identity.sign_envelope(envelope)
}

pub fn verify_envelope<T: Serialize>(public_key: &str, envelope: &MessageEnvelope<T>) -> bool {
    Identity::verify_envelope(public_key, envelope)
}

pub fn pairing_message(challenge: &str, monitor_id: &str, host_id: &str) -> Vec<u8> {
    format!("codex-monitor/pair-v1:{monitor_id}:{host_id}:{challenge}").into_bytes()
}

pub fn default_identity_path(role: &str) -> PathBuf {
    if let Ok(path) = env::var("CODEX_MONITOR_IDENTITY_PATH") {
        return PathBuf::from(path);
    }

    if cfg!(windows) {
        let root = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return root.join("CodexMonitor").join(format!("{role}.identity"));
    }

    let root = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    root.join("codex-monitor").join(format!("{role}.identity"))
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_and_verifies_pairing_messages() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let identity = Identity { signing_key: key };
        let message = pairing_message("challenge", "monitor", "agent");
        let signature = identity.sign_hex(&message);

        assert!(Identity::verify_hex(
            &identity.public_key_hex(),
            &message,
            &signature
        ));
        assert!(!Identity::verify_hex(
            &identity.public_key_hex(),
            b"different",
            &signature
        ));
    }

    #[test]
    fn signs_and_verifies_envelopes_without_signature_field() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let identity = Identity { signing_key: key };
        let mut envelope = MessageEnvelope::new(
            "state_snapshot",
            "network-1",
            identity.host_id(),
            3,
            10,
            Some("request-1".to_string()),
            serde_json::json!({"sourceNodeId": "node-a"}),
        );

        identity
            .sign_envelope(&mut envelope)
            .expect("sign envelope");
        assert!(envelope.validate().is_ok());
        assert!(Identity::verify_envelope(
            &identity.public_key_hex(),
            &envelope
        ));

        envelope.sequence = 4;
        assert!(!Identity::verify_envelope(
            &identity.public_key_hex(),
            &envelope
        ));
    }

    #[test]
    fn rejects_non_hex_public_keys_before_slicing() {
        assert!(Identity::host_id_from_public_key(&"é".repeat(32)).is_none());
    }
}
