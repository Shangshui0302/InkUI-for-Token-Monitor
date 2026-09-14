use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use p256::ecdsa::{
    Signature as P256Signature, SigningKey as P256SigningKey, VerifyingKey as P256VerifyingKey,
};
use p256::pkcs8::{DecodePublicKey, EncodePublicKey};
use rand::rngs::OsRng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::MessageEnvelope;

/// A locally stored Ed25519 identity. The private key never belongs in a wire message.
pub struct Identity {
    signing_key: SigningKey,
}

/// Secondary P-256 key used only for pairing with Android's SHA256withECDSA
/// implementation. The established Ed25519 identity and node id are unchanged.
pub struct PairingIdentity {
    signing_key: P256SigningKey,
}

const P256_PREFIX: &str = "p256:";

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

impl PairingIdentity {
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let signing_key = P256SigningKey::from_slice(&bytes).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "P-256 pairing key encoding")
                })?;
                return Ok(Self { signing_key });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let identity = Self {
            signing_key: P256SigningKey::random(&mut OsRng),
        };
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, identity.signing_key.to_bytes())?;
        fs::rename(&temporary, path)?;
        set_private_permissions(path)?;
        Ok(identity)
    }

    /// Canonical `p256:<lowercase hex SPKI DER>` form shared with Android.
    pub fn public_key(&self) -> String {
        encode_p256_public_key(&self.signing_key.verifying_key())
    }

    /// Produce an ASN.1 DER ECDSA signature, the form emitted by Java's
    /// `SHA256withECDSA` Signature implementation.
    pub fn sign_hex(&self, message: &[u8]) -> String {
        let signature: P256Signature = self.signing_key.sign(message);
        hex::encode(signature.to_der().as_bytes())
    }
}

fn encode_p256_public_key(key: &P256VerifyingKey) -> String {
    let der = key
        .to_public_key_der()
        .expect("P-256 public key has a valid SPKI encoding");
    format!("{P256_PREFIX}{}", hex::encode(der.as_bytes()))
}

fn decode_p256_public_key(value: &str) -> Option<(P256VerifyingKey, Vec<u8>)> {
    let encoded = value.strip_prefix(P256_PREFIX)?;
    if encoded.is_empty() || encoded.len() % 2 != 0 || encoded.len() > 1024 {
        return None;
    }
    let der = hex::decode(encoded).ok()?;
    let key = P256VerifyingKey::from_public_key_der(&der).ok()?;
    // Accept only the canonical SPKI form, rejecting alternate encodings and
    // trailing bytes so host-id hashing has one unambiguous input.
    if key.to_public_key_der().ok()?.as_bytes() != der {
        return None;
    }
    Some((key, der))
}

pub fn validate_pairing_public_key(public_key: &str) -> bool {
    decode_p256_public_key(public_key).is_some()
}

pub fn p256_host_id_from_public_key(public_key: &str) -> Option<String> {
    let (_, der) = decode_p256_public_key(public_key)?;
    let digest = Sha256::digest(der);
    Some(format!("{P256_PREFIX}{}", hex::encode(&digest[..8])))
}

/// Strictly validate both key formats and ensure the announced node id is
/// derived from at least one key. This permits legacy Ed25519 ids as well as
/// Android primary P-256 ids without changing desktop Ed25519 node ids.
pub fn validate_node_identity(
    host_id: &str,
    public_key: &str,
    pairing_public_key: Option<&str>,
) -> Result<(), &'static str> {
    let primary_host_id = if public_key.starts_with(P256_PREFIX) {
        p256_host_id_from_public_key(public_key).ok_or("invalid public key")?
    } else {
        Identity::host_id_from_public_key(public_key).ok_or("invalid public key")?
    };
    if pairing_public_key.is_some_and(|key| !validate_pairing_public_key(key)) {
        return Err("invalid pairing public key");
    }
    if host_id == primary_host_id {
        Ok(())
    } else {
        Err("host id does not match public key")
    }
}

/// Verify the actual handshake key. Unprefixed keys are the original raw
/// Ed25519 public-key hex format; new Android-compatible keys are P-256 SPKI.
pub fn verify_pairing_signature(public_key: &str, message: &[u8], signature: &str) -> bool {
    if !public_key.starts_with(P256_PREFIX) {
        return Identity::verify_hex(public_key, message, signature);
    }
    let Some((verifying_key, _)) = decode_p256_public_key(public_key) else {
        return false;
    };
    let Ok(signature_bytes) = hex::decode(signature) else {
        return false;
    };
    let Ok(signature) = P256Signature::from_der(&signature_bytes) else {
        return false;
    };
    verifying_key.verify(message, &signature).is_ok()
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

/// Canonical, role-ordered transcript for code-authenticated pairing.
/// Length-prefixing prevents delimiter ambiguity across language implementations.
pub fn pairing_transcript(
    initiator_id: &str,
    initiator_public_key: &str,
    initiator_pairing_key: &str,
    acceptor_id: &str,
    acceptor_public_key: &str,
    acceptor_pairing_key: &str,
    network_id: &str,
    challenge: &str,
) -> Vec<u8> {
    let mut bytes = b"codex-monitor/pair-transcript-v2".to_vec();
    for field in [
        initiator_id,
        initiator_public_key,
        initiator_pairing_key,
        acceptor_id,
        acceptor_public_key,
        acceptor_pairing_key,
        network_id,
        challenge,
    ] {
        bytes.extend_from_slice(&(field.len() as u32).to_be_bytes());
        bytes.extend_from_slice(field.as_bytes());
    }
    bytes
}

/// Canonical domain-separated bytes for a successful pairing acceptance.
/// This deliberately differs from the challenge-signing transcript.
pub fn pairing_acceptance_message(transcript: &[u8]) -> Vec<u8> {
    let mut bytes = b"codex-monitor/pair-acceptance-v2".to_vec();
    bytes.extend_from_slice(&(transcript.len() as u32).to_be_bytes());
    bytes.extend_from_slice(transcript);
    bytes
}

/// HMAC-SHA256 without an additional runtime dependency (RFC 2104).
pub fn pairing_code_proof(code: &str, role: &str, transcript: &[u8]) -> String {
    let mut message = b"codex-monitor/pair-code-proof-v2:".to_vec();
    message.extend_from_slice(role.as_bytes());
    message.push(0);
    message.extend_from_slice(transcript);
    let mut key = [0u8; 64];
    key[..code.len()].copy_from_slice(code.as_bytes());
    let mut inner = vec![0x36; 64];
    let mut outer = vec![0x5c; 64];
    for index in 0..64 {
        inner[index] ^= key[index];
        outer[index] ^= key[index];
    }
    let mut inner_hash = Sha256::new();
    inner_hash.update(inner);
    inner_hash.update(message);
    let mut outer_hash = Sha256::new();
    outer_hash.update(outer);
    outer_hash.update(inner_hash.finalize());
    hex::encode(outer_hash.finalize())
}

pub fn constant_time_hex_eq(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (hex::decode(left), hex::decode(right)) else {
        return false;
    };
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
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

pub fn default_pairing_identity_path(role: &str) -> PathBuf {
    default_identity_path(role).with_file_name(format!("{role}.pairing.identity"))
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

    #[test]
    fn p256_spki_and_der_signature_round_trip() {
        let identity = PairingIdentity {
            signing_key: P256SigningKey::from_slice(&[7u8; 32]).expect("P-256 test key"),
        };
        let public_key = identity.public_key();
        let message = pairing_message("challenge", "monitor", "agent");
        let signature = identity.sign_hex(&message);

        assert!(validate_pairing_public_key(&public_key));
        assert!(verify_pairing_signature(&public_key, &message, &signature));
        assert!(!verify_pairing_signature(
            &public_key,
            b"different",
            &signature
        ));
        assert!(hex::decode(&signature).unwrap().starts_with(&[0x30]));
        assert_eq!(p256_host_id_from_public_key(&public_key).unwrap().len(), 21);
    }

    #[test]
    fn validates_ed25519_and_p256_primary_node_ids() {
        let ed = Identity {
            signing_key: SigningKey::from_bytes(&[7u8; 32]),
        };
        let p256 = PairingIdentity {
            signing_key: P256SigningKey::from_slice(&[8u8; 32]).expect("P-256 test key"),
        };
        assert!(validate_node_identity(
            &ed.host_id(),
            &ed.public_key_hex(),
            Some(&p256.public_key()),
        )
        .is_ok());
        let p256_primary_id = p256_host_id_from_public_key(&p256.public_key()).unwrap();
        assert!(validate_node_identity(&p256_primary_id, &p256.public_key(), None).is_ok());
        assert!(validate_node_identity(
            &p256_primary_id,
            &p256.public_key(),
            Some(&ed.public_key_hex())
        )
        .is_err());
        assert!(
            validate_node_identity("p256:deadbeefdeadbeef", &ed.public_key_hex(), None).is_err()
        );
        assert!(!validate_pairing_public_key("p256:3000"));
    }

    #[test]
    fn legacy_ed25519_pairing_signatures_remain_supported() {
        let identity = Identity {
            signing_key: SigningKey::from_bytes(&[7u8; 32]),
        };
        let message = b"legacy pairing message";
        let signature = identity.sign_hex(message);
        assert!(verify_pairing_signature(
            &identity.public_key_hex(),
            message,
            &signature
        ));
    }
}
