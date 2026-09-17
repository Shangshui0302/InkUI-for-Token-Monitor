//! Internal Android lifecycle bridge for the Rust network core.

use std::marker::PhantomData;

#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;
use tauri::{Manager, Runtime, plugin::TauriPlugin};

#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
struct EmptyRequest {}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
struct EmptyResponse {}

/// Native lifecycle controls. This handle is intentionally available only to Rust.
pub struct PlatformLifecycle<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: PluginHandle<R>,
    marker: PhantomData<fn() -> R>,
}

impl<R: Runtime> Clone for PlatformLifecycle<R> {
    fn clone(&self) -> Self {
        Self {
            #[cfg(target_os = "android")]
            handle: self.handle.clone(),
            marker: PhantomData,
        }
    }
}

impl<R: Runtime> PlatformLifecycle<R> {
    /// Start the same-process foreground service that protects the running core.
    pub fn start_core_service(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            return self
                .handle
                .run_mobile_plugin::<Option<EmptyResponse>>("startCoreService", EmptyRequest {})
                .map(|_| ())
                .map_err(|error| error.to_string());
        }

        #[cfg(not(target_os = "android"))]
        Err("the core foreground service is only available on Android".to_owned())
    }

    /// Stop the foreground service without terminating the Tauri process.
    pub fn stop_core_service(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            return self
                .handle
                .run_mobile_plugin::<Option<EmptyResponse>>("stopCoreService", EmptyRequest {})
                .map(|_| ())
                .map_err(|error| error.to_string());
        }

        #[cfg(not(target_os = "android"))]
        Err("the core foreground service is only available on Android".to_owned())
    }

    /// Hold Android's multicast lock for a foreground, one-shot discovery scan.
    pub fn acquire_discovery_multicast_lock(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            return self
                .handle
                .run_mobile_plugin::<Option<EmptyResponse>>(
                    "acquireDiscoveryMulticastLock",
                    EmptyRequest {},
                )
                .map(|_| ())
                .map_err(|error| error.to_string());
        }

        #[cfg(not(target_os = "android"))]
        Err("the discovery multicast lock is only available on Android".to_owned())
    }

    /// Release the one-shot discovery multicast lock.
    pub fn release_discovery_multicast_lock(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            return self
                .handle
                .run_mobile_plugin::<Option<EmptyResponse>>(
                    "releaseDiscoveryMulticastLock",
                    EmptyRequest {},
                )
                .map(|_| ())
                .map_err(|error| error.to_string());
        }

        #[cfg(not(target_os = "android"))]
        Err("the discovery multicast lock is only available on Android".to_owned())
    }
}

/// Register the Android bridge and keep all lifecycle operations native-only.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("nekodex-platform-lifecycle")
        .invoke_handler(|invoke| {
            invoke
                .resolver
                .reject("Platform lifecycle controls are internal to the native backend");
            true
        })
        .setup(|app, _api| {
            #[cfg(target_os = "android")]
            let lifecycle = PlatformLifecycle {
                handle: _api.register_android_plugin(
                    "dev.nekodex.platformlifecycle",
                    "PlatformLifecyclePlugin",
                )?,
                marker: PhantomData,
            };

            #[cfg(not(target_os = "android"))]
            let lifecycle: PlatformLifecycle<R> = PlatformLifecycle {
                marker: PhantomData,
            };

            app.manage(lifecycle);
            Ok(())
        })
        .build()
}
