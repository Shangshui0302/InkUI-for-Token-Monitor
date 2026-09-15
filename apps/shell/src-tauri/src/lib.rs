use serde::Serialize;

#[cfg(not(target_os = "android"))]
mod p0_appserver;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct P0Environment {
    platform: &'static str,
    local_assets: bool,
    ipc_available: bool,
}

#[tauri::command]
fn p0_environment() -> P0Environment {
    P0Environment {
        platform: std::env::consts::OS,
        local_assets: true,
        ipc_available: true,
    }
}

#[tauri::command]
async fn p0_appserver_lifecycle() -> Result<serde_json::Value, String> {
    #[cfg(not(target_os = "android"))]
    {
        p0_appserver::run_lifecycle().await
    }

    #[cfg(target_os = "android")]
    Err("Poke3 是控制端，不在本机运行 Codex App Server".to_owned())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            p0_environment,
            p0_appserver_lifecycle
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Nekodex shell");
}
