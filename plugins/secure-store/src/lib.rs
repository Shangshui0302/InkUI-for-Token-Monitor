//! Internal native secret bridge. No frontend invocation is permitted.

#[cfg(target_os = "android")]
use nekodex_device_core::identity::{SecretStore, SecretStoreError};
#[cfg(target_os = "android")]
use tauri::{Manager, plugin::PluginHandle};
use tauri::{Runtime, plugin::TauriPlugin};

#[cfg(target_os = "android")]
pub struct AndroidSecretStore<R: Runtime>(PluginHandle<R>);

#[cfg(target_os = "android")]
impl<R: Runtime> Clone for AndroidSecretStore<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
struct Request<'a> {
    key: &'a str,
    value: Option<&'a [u8]>,
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct Response {
    value: Option<Vec<u8>>,
}

#[cfg(target_os = "android")]
impl<R: Runtime> SecretStore for AndroidSecretStore<R> {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        self.0
            .run_mobile_plugin::<Response>("readIdentity", Request { key, value: None })
            .map(|response| response.value)
            .map_err(|_| SecretStoreError::Backend("Android secret read failed".into()))
    }

    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), SecretStoreError> {
        self.0
            .run_mobile_plugin::<Response>(
                "writeIdentity",
                Request {
                    key,
                    value: Some(value),
                },
            )
            .map(|_| ())
            .map_err(|_| SecretStoreError::Backend("Android secret write failed".into()))
    }
}

/// Register before shell setup. Call the stored adapter only from a blocking worker.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("nekodex-secure-store")
        // Explicitly consume ALL webview invocations: never fall through to the
        // mobile command router, even if a capability is accidentally broadened.
        .invoke_handler(|invoke| {
            invoke
                .resolver
                .reject("Secret storage is internal to the native backend");
            true
        })
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    _api.register_android_plugin("dev.nekodex.securestore", "SecretStorePlugin")?;
                _app.manage(AndroidSecretStore(handle));
            }
            Ok(())
        })
        .build()
}
