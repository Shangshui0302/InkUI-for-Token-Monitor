//! Persistent platform secrets. Call from a blocking worker, never the UI thread.

use nekodex_device_core::identity::{SecretStore, SecretStoreError};

#[cfg(target_os = "linux")]
pub struct LinuxSecretStore;

#[cfg(target_os = "linux")]
impl LinuxSecretStore {
    fn entry(key: &str) -> Result<keyring::Entry, SecretStoreError> {
        if key != "nekodex/device/identity/v1" {
            return Err(SecretStoreError::Backend("Unknown secret key".into()));
        }
        keyring::Entry::new("dev.nekodex.device", key)
            .map_err(|_| SecretStoreError::Backend("Secret Service entry unavailable".into()))
    }
}

#[cfg(target_os = "linux")]
impl SecretStore for LinuxSecretStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        match Self::entry(key)?.get_secret() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(SecretStoreError::Backend(
                "Secret Service read failed".into(),
            )),
        }
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        if value.len() != 65 || value[0] != 1 {
            return Err(SecretStoreError::Backend("Invalid identity bundle".into()));
        }
        Self::entry(key)?
            .set_secret(value)
            .map_err(|_| SecretStoreError::Backend("Secret Service write failed".into()))
    }
}

#[cfg(target_os = "linux")]
pub fn open<R: tauri::Runtime>(
    _: &tauri::AppHandle<R>,
) -> Result<Box<dyn SecretStore>, SecretStoreError> {
    Ok(Box::new(LinuxSecretStore))
}

#[cfg(target_os = "windows")]
pub struct WindowsSecretStore;

#[cfg(target_os = "windows")]
impl WindowsSecretStore {
    fn entry(key: &str) -> Result<keyring::Entry, SecretStoreError> {
        if key != "nekodex/device/identity/v1" {
            return Err(SecretStoreError::Backend("Unknown secret key".into()));
        }
        keyring::Entry::new("dev.nekodex.device", key)
            .map_err(|_| SecretStoreError::Backend("Windows Credential Manager unavailable".into()))
    }
}

#[cfg(target_os = "windows")]
impl SecretStore for WindowsSecretStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        match Self::entry(key)?.get_secret() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(SecretStoreError::Backend(
                "Windows Credential Manager read failed".into(),
            )),
        }
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        if value.len() != 65 || value[0] != 1 {
            return Err(SecretStoreError::Backend("Invalid identity bundle".into()));
        }
        Self::entry(key)?.set_secret(value).map_err(|_| {
            SecretStoreError::Backend("Windows Credential Manager write failed".into())
        })
    }
}

#[cfg(target_os = "windows")]
pub fn open<R: tauri::Runtime>(
    _: &tauri::AppHandle<R>,
) -> Result<Box<dyn SecretStore>, SecretStoreError> {
    Ok(Box::new(WindowsSecretStore))
}

#[cfg(target_os = "android")]
pub fn open<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Box<dyn SecretStore>, SecretStoreError> {
    use tauri::Manager;
    app.try_state::<tauri_plugin_nekodex_secure_store::AndroidSecretStore<R>>()
        .map(|state| Box::new((*state).clone()) as Box<dyn SecretStore>)
        .ok_or_else(|| SecretStoreError::Backend("Android secret store is not initialized".into()))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "windows")))]
pub fn open<R: tauri::Runtime>(
    _: &tauri::AppHandle<R>,
) -> Result<Box<dyn SecretStore>, SecretStoreError> {
    Err(SecretStoreError::Backend(
        "Unsupported secret-store platform".into(),
    ))
}
