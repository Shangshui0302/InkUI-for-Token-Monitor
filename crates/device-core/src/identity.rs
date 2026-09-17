//! Persistent device identity and platform secret-store boundary.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use ed25519_dalek::SigningKey;
use nekodex_domain::signing::member_id_for_key;
use snow::params::NoiseParams;
use thiserror::Error;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::transport::PeerCredentials;
use crate::types::{DeviceProfile, TRANSPORT_PROTOCOL_VERSION};
use nekodex_protocol::BusinessCapabilities;

const IDENTITY_BUNDLE_KEY: &str = "nekodex/device/identity/v1";
const IDENTITY_BUNDLE_VERSION: u8 = 1;
const IDENTITY_BUNDLE_LEN: usize = 1 + 32 + 32;

/// The small interface required from an operating-system secret store.
///
/// Implementations should provide atomic, durable writes where possible. The
/// values passed to `put` are secret material and must not be logged.
pub trait SecretStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError>;
    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), SecretStoreError>;
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum SecretStoreError {
    #[error("secret store error: {0}")]
    Backend(String),
}

/// An in-memory store, primarily useful for tests and embedding applications.
#[derive(Clone, Default)]
pub struct MemorySecretStore {
    values: HashMap<String, Vec<u8>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl fmt::Debug for MemorySecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemorySecretStore")
            .field("keys", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        Ok(self.values.get(key).cloned())
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        self.values.insert(key.to_owned(), value.to_vec());
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("secret store error: {0}")]
    Store(#[from] SecretStoreError),
    #[error("identity secret has invalid length: {0}")]
    InvalidSecret(&'static str),
    #[error("unsupported identity bundle version: {0}")]
    UnsupportedVersion(u8),
    #[error("unable to initialize Noise key generation: {0}")]
    Noise(String),
}

/// A device's long-lived signing and Noise identities.
#[derive(Clone)]
pub struct DeviceIdentity {
    signing_key: SigningKey,
    noise_private_key: [u8; 32],
    noise_public_key: [u8; 32],
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("device_id", &self.device_id())
            .field("signing_public_key", &self.signing_key.verifying_key())
            .field("noise_public_key", &self.noise_public_key)
            .finish()
    }
}

impl DeviceIdentity {
    /// Load an existing identity, or generate and persist one on first use.
    pub fn load_or_generate(store: &mut dyn SecretStore) -> Result<Self, IdentityError> {
        let (signing, noise_private) = match store.get(IDENTITY_BUNDLE_KEY)? {
            Some(bundle) => decode_bundle(&bundle)?,
            None => {
                let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
                let pair = snow::Builder::new(noise_params()?)
                    .generate_keypair()
                    .map_err(|e| IdentityError::Noise(e.to_string()))?;
                let noise_private = fixed_key(&pair.private, "Noise private key")?;
                let signing = signing_key.to_bytes();
                store.put(
                    IDENTITY_BUNDLE_KEY,
                    &encode_bundle(&signing, &noise_private),
                )?;
                (signing, noise_private)
            }
        };
        let signing_key = SigningKey::from_bytes(&signing);
        let noise_public = noise_public_from_private(&noise_private);
        Ok(Self {
            signing_key,
            noise_private_key: noise_private,
            noise_public_key: noise_public,
        })
    }

    pub fn device_id(&self) -> String {
        member_id_for_key(&self.signing_key.verifying_key())
    }

    pub fn signing_public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn noise_public_key(&self) -> [u8; 32] {
        self.noise_public_key
    }

    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    pub fn peer_credentials(&self, group_id: impl Into<String>) -> PeerCredentials {
        PeerCredentials {
            device_id: self.device_id(),
            group_id: group_id.into(),
            transport_version: TRANSPORT_PROTOCOL_VERSION,
            business_capabilities: BusinessCapabilities::current(),
            signing_key: self.signing_key.clone(),
            noise_private_key: self.noise_private_key,
        }
    }

    pub fn profile(&self, name: impl Into<String>, codex_capable: bool) -> DeviceProfile {
        DeviceProfile {
            device_id: self.device_id(),
            name: name.into(),
            signing_public_key: self.signing_public_key(),
            noise_public_key: self.noise_public_key,
            codex_capable,
            protocol_version: TRANSPORT_PROTOCOL_VERSION,
        }
    }
}

fn noise_params() -> Result<NoiseParams, IdentityError> {
    NoiseParams::from_str("Noise_XX_25519_ChaChaPoly_BLAKE2s")
        .map_err(|e| IdentityError::Noise(e.to_string()))
}

fn fixed_key(value: &[u8], name: &'static str) -> Result<[u8; 32], IdentityError> {
    value
        .try_into()
        .map_err(|_| IdentityError::InvalidSecret(name))
}

fn encode_bundle(signing: &[u8; 32], noise: &[u8; 32]) -> [u8; IDENTITY_BUNDLE_LEN] {
    let mut bundle = [0; IDENTITY_BUNDLE_LEN];
    bundle[0] = IDENTITY_BUNDLE_VERSION;
    bundle[1..33].copy_from_slice(signing);
    bundle[33..].copy_from_slice(noise);
    bundle
}

fn decode_bundle(bundle: &[u8]) -> Result<([u8; 32], [u8; 32]), IdentityError> {
    if bundle.len() != IDENTITY_BUNDLE_LEN {
        return Err(IdentityError::InvalidSecret("identity bundle"));
    }
    if bundle[0] != IDENTITY_BUNDLE_VERSION {
        return Err(IdentityError::UnsupportedVersion(bundle[0]));
    }
    Ok((
        fixed_key(&bundle[1..33], "signing seed")?,
        fixed_key(&bundle[33..], "Noise private key")?,
    ))
}

fn noise_public_from_private(private: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*private)).to_bytes()
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::VerifyingKey;
    use nekodex_domain::signing::member_id_for_key;

    use super::*;

    #[test]
    fn first_generation_and_reload_are_stable() {
        let mut store = MemorySecretStore::new();
        let first = DeviceIdentity::load_or_generate(&mut store).unwrap();
        let second = DeviceIdentity::load_or_generate(&mut store).unwrap();
        assert_eq!(first.device_id(), second.device_id());
        assert_eq!(first.signing_public_key(), second.signing_public_key());
        assert_eq!(first.noise_public_key(), second.noise_public_key());
        assert_eq!(
            first.profile("laptop", true),
            second.profile("laptop", true)
        );
    }

    #[test]
    fn malformed_secret_is_rejected() {
        let mut store = MemorySecretStore::new();
        DeviceIdentity::load_or_generate(&mut store).unwrap();
        store.put(IDENTITY_BUNDLE_KEY, &[0u8; 31]).unwrap();
        assert!(matches!(
            DeviceIdentity::load_or_generate(&mut store),
            Err(IdentityError::InvalidSecret("identity bundle"))
        ));
    }

    #[test]
    fn unsupported_bundle_version_is_rejected() {
        let mut store = MemorySecretStore::new();
        DeviceIdentity::load_or_generate(&mut store).unwrap();
        let mut bundle = store.values[IDENTITY_BUNDLE_KEY].clone();
        bundle[0] = 2;
        store.put(IDENTITY_BUNDLE_KEY, &bundle).unwrap();
        assert!(matches!(
            DeviceIdentity::load_or_generate(&mut store),
            Err(IdentityError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn noise_public_is_derived_and_not_loaded_from_store() {
        let mut store = MemorySecretStore::new();
        let first = DeviceIdentity::load_or_generate(&mut store).unwrap();
        let second = DeviceIdentity::load_or_generate(&mut store).unwrap();
        assert_eq!(first.noise_public_key(), second.noise_public_key());
    }

    #[test]
    fn profile_id_is_self_certifying() {
        let mut store = MemorySecretStore::new();
        let identity = DeviceIdentity::load_or_generate(&mut store).unwrap();
        let profile = identity.profile("phone", false);
        let key = VerifyingKey::from_bytes(&profile.signing_public_key).unwrap();
        assert_eq!(profile.device_id, member_id_for_key(&key));
    }
}
