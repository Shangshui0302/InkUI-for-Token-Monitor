use serde::Serialize;
use tauri::{Runtime, plugin::TauriPlugin};

mod p1_device;
mod p1_network;
#[cfg(not(target_os = "android"))]
mod p2_source;
mod platform_lifecycle;
mod secret_store;

fn platform_events<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::<R>::new("nekodex-platform-events")
        .on_event(|app, event| {
            #[cfg(target_os = "android")]
            if matches!(event, tauri::RunEvent::Resumed) {
                use tauri::Manager;
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<p1_device::P1State>();
                    if let Err(error) = p1_device::resume_revalidate(&app, &state).await {
                        eprintln!("unable to revalidate device group after resume: {error}");
                    }
                });
            }
            #[cfg(not(target_os = "android"))]
            let _ = (app, event);
        })
        .build()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppEnvironment {
    platform: &'static str,
    version: &'static str,
    local_assets: bool,
    ipc_available: bool,
}

#[tauri::command]
fn app_environment() -> AppEnvironment {
    AppEnvironment {
        platform: std::env::consts::OS,
        version: env!("CARGO_PKG_VERSION"),
        local_assets: true,
        ipc_available: true,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(not(desktop))]
            let _ = app;
            #[cfg(desktop)]
            {
                use tauri::{
                    Manager, WindowEvent,
                    menu::{Menu, MenuItem},
                    tray::{TrayIconBuilder, TrayIconEvent},
                };

                let open = MenuItem::with_id(app, "open", "打开 Nekodex", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "退出 Nekodex", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&open, &quit])?;
                let icon = app
                    .default_window_icon()
                    .cloned()
                    .ok_or("缺少默认应用图标")?;

                TrayIconBuilder::with_id("nekodex-tray")
                    .icon(icon)
                    .tooltip("Nekodex")
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id().as_ref() {
                        "open" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "quit" => {
                            let app = app.clone();
                            tauri::async_runtime::spawn(async move {
                                let state = app.state::<p1_device::P1State>();
                                p1_device::shutdown_application(&app, &state).await;
                                app.exit(0);
                            });
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button,
                            button_state,
                            ..
                        } = event
                        {
                            if button == tauri::tray::MouseButton::Left
                                && button_state == tauri::tray::MouseButtonState::Up
                            {
                                if let Some(window) = tray.app_handle().get_webview_window("main") {
                                    let _ = window.show();
                                    let _ = window.set_focus();
                                }
                            }
                        }
                    })
                    .build(app)?;

                if let Some(window) = app.get_webview_window("main") {
                    let window_for_event = window.clone();
                    window.on_window_event(move |event| {
                        if let WindowEvent::CloseRequested { api, .. } = event {
                            api.prevent_close();
                            let _ = window_for_event.hide();
                        }
                    });
                }
            }

            Ok(())
        })
        .plugin(tauri_plugin_nekodex_secure_store::init())
        .plugin(tauri_plugin_nekodex_platform_lifecycle::init())
        .plugin(platform_events())
        .manage(p1_device::P1State::default())
        .invoke_handler(tauri::generate_handler![
            app_environment,
            p1_device::p1_initialize,
            p1_device::p1_create_group,
            p1_device::p1_discover_nearby,
            p1_device::p1_state,
            p1_device::p1_create_invitation,
            p1_device::p1_pending_join,
            p1_device::p1_decide_join,
            p1_device::p1_join_group,
            p1_device::p1_network_state,
            p1_device::p1_resume_revalidate,
            p1_device::p1_members,
            p1_device::p1_rename_device,
            p1_device::p1_exit_group,
            p1_device::p1_revoke_member,
            p1_device::p1_connect_peer,
            p1_device::p2_source_state,
            p1_device::p2_restart_source,
            p1_device::p2_start_shared_source,
            p1_device::p2_local_projects,
            p1_device::p2_add_local_project,
            p1_device::p2_update_local_project,
            p1_device::p2_delete_local_project,
            p1_device::p2_request,
            p1_device::p2_events,
            p1_device::p2_event_revision,
            p1_device::p2_cached_request
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Nekodex shell");
}
