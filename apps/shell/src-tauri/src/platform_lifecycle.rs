//! Platform process-lifecycle integration. Desktop builds intentionally do nothing.

use tauri::{AppHandle, Runtime};

#[cfg(target_os = "android")]
use tauri::Manager;

pub fn start_core_service<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        return app
            .try_state::<tauri_plugin_nekodex_platform_lifecycle::PlatformLifecycle<R>>()
            .ok_or_else(|| "Android 平台生命周期桥未初始化".to_owned())?
            .start_core_service();
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}

pub fn stop_core_service<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        return app
            .try_state::<tauri_plugin_nekodex_platform_lifecycle::PlatformLifecycle<R>>()
            .ok_or_else(|| "Android 平台生命周期桥未初始化".to_owned())?
            .stop_core_service();
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}

pub fn acquire_discovery_multicast_lock<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        return app
            .try_state::<tauri_plugin_nekodex_platform_lifecycle::PlatformLifecycle<R>>()
            .ok_or_else(|| "Android 平台生命周期桥未初始化".to_owned())?
            .acquire_discovery_multicast_lock();
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}

pub fn release_discovery_multicast_lock<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        return app
            .try_state::<tauri_plugin_nekodex_platform_lifecycle::PlatformLifecycle<R>>()
            .ok_or_else(|| "Android 平台生命周期桥未初始化".to_owned())?
            .release_discovery_multicast_lock();
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}
