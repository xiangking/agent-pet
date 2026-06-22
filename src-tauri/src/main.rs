// Agent Pet Desktop - Main Entry Point
// Compatible with Codex pet protocol: 1536x1872, 8x9 grid, 192x208 cells

mod codex_monitor;
mod focus;
mod message;
mod pet;
mod settings;
mod state_machine;
mod tray;
mod websocket;

use std::sync::Arc;
use tauri::async_runtime::Mutex;
use tauri::{Manager, WindowEvent};
use tokio_util::sync::CancellationToken;

use crate::codex_monitor::CodexMonitor;
use crate::message::PetNotice;
use crate::state_machine::{
    default_live_source_focus_targets, default_live_source_paths, PetStateMachine,
};
use crate::websocket::WebSocketServer;

pub struct AppState {
    pub state_machine: Arc<Mutex<PetStateMachine>>,
    pub cancellation_token: CancellationToken,
}

#[tauri::command]
async fn get_pet_list(app: tauri::AppHandle) -> Result<Vec<pet::PetInfo>, String> {
    pet::list_pets(app).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_user_pet_dir() -> Result<String, String> {
    pet::user_pets_dir()
        .map(|path| path.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn load_pet(
    state: tauri::State<'_, AppState>,
    pet_id: String,
) -> Result<pet::PetConfig, String> {
    let mut sm = state.state_machine.lock().await;
    sm.load_pet(&pet_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_current_state(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let sm = state.state_machine.lock().await;
    Ok(sm.current_state().to_string())
}

#[tauri::command]
async fn trigger_state(
    state: tauri::State<'_, AppState>,
    message_type: String,
) -> Result<String, String> {
    let mut sm = state.state_machine.lock().await;
    let new_state = sm.handle_message(&message_type);
    Ok(new_state.to_string())
}

#[tauri::command]
async fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("settings")
        .ok_or_else(|| "Settings window not found".to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

#[tauri::command]
async fn trigger_notice(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let sm = state.state_machine.lock().await;
    sm.show_notice(&PetNotice {
        id: "manual-test-notice".to_string(),
        group_key: "manual-test-notice".to_string(),
        level: "info".to_string(),
        title: "Manual test".to_string(),
        body: "Live usage and real notices appear only when a monitored source or WebSocket sends them."
            .to_string(),
        source: "manual".to_string(),
        source_label: Some("Agent Pet".to_string()),
        ttl_seconds: 60,
        timestamp: None,
    });
    Ok(())
}

#[tauri::command]
async fn get_message_map(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let sm = state.state_machine.lock().await;
    Ok(serde_json::to_value(sm.message_map()).map_err(|e| e.to_string())?)
}

#[tauri::command]
async fn update_message_map(
    state: tauri::State<'_, AppState>,
    map: serde_json::Value,
) -> Result<(), String> {
    let mut sm = state.state_machine.lock().await;
    let new_map: std::collections::HashMap<String, String> =
        serde_json::from_value(map).map_err(|e| e.to_string())?;
    sm.set_message_map(new_map);
    Ok(())
}

#[tauri::command]
async fn get_websocket_status(
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let sm = state.state_machine.lock().await;
    Ok(serde_json::json!({
        "enabled": sm.websocket_enabled(),
        "port": sm.websocket_port(),
    }))
}

#[tauri::command]
async fn toggle_websocket(state: tauri::State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut sm = state.state_machine.lock().await;
    sm.set_websocket_enabled(enabled);
    Ok(())
}

#[tauri::command]
async fn get_codex_monitor_status(
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let sm = state.state_machine.lock().await;
    Ok(serde_json::json!({
        "enabled": sm.codex_monitor_enabled(),
    }))
}

#[tauri::command]
async fn toggle_codex_monitor(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let mut sm = state.state_machine.lock().await;
    sm.set_live_source_enabled("codex", enabled)
}

#[tauri::command]
async fn get_live_sources_status(
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let sm = state.state_machine.lock().await;
    let paths = sm.live_source_paths();
    let defaults = default_live_source_paths();
    let focus_targets = sm.live_source_focus_targets();
    let default_focus_targets = default_live_source_focus_targets();

    Ok(serde_json::json!({
        "codex": {
            "enabled": sm.codex_monitor_enabled(),
            "path": paths.get("codex"),
            "defaultPath": defaults.get("codex"),
            "focusTarget": focus_targets.get("codex"),
            "defaultFocusTarget": default_focus_targets.get("codex"),
        },
        "claude": {
            "enabled": sm.claude_monitor_enabled(),
            "path": paths.get("claude"),
            "defaultPath": defaults.get("claude"),
            "focusTarget": focus_targets.get("claude"),
            "defaultFocusTarget": default_focus_targets.get("claude"),
        },
        "opencode": {
            "enabled": sm.opencode_monitor_enabled(),
            "path": paths.get("opencode"),
            "defaultPath": defaults.get("opencode"),
            "focusTarget": focus_targets.get("opencode"),
            "defaultFocusTarget": default_focus_targets.get("opencode"),
        },
        "openclaw": {
            "enabled": sm.openclaw_monitor_enabled(),
            "path": paths.get("openclaw"),
            "defaultPath": defaults.get("openclaw"),
            "focusTarget": focus_targets.get("openclaw"),
            "defaultFocusTarget": default_focus_targets.get("openclaw"),
        },
        "hermes": {
            "enabled": sm.hermes_monitor_enabled(),
            "path": paths.get("hermes"),
            "defaultPath": defaults.get("hermes"),
            "focusTarget": focus_targets.get("hermes"),
            "defaultFocusTarget": default_focus_targets.get("hermes"),
        },
        "antigravity": {
            "enabled": sm.antigravity_monitor_enabled(),
            "path": paths.get("antigravity"),
            "defaultPath": defaults.get("antigravity"),
            "focusTarget": focus_targets.get("antigravity"),
            "defaultFocusTarget": default_focus_targets.get("antigravity"),
        },
        "prefixEnabled": sm.live_source_prefix_enabled(),
    }))
}

#[tauri::command]
async fn toggle_live_source(
    state: tauri::State<'_, AppState>,
    source: String,
    enabled: bool,
) -> Result<(), String> {
    let mut sm = state.state_machine.lock().await;
    sm.set_live_source_enabled(&source, enabled)
}

#[tauri::command]
async fn set_live_source_path(
    state: tauri::State<'_, AppState>,
    source: String,
    path: String,
) -> Result<(), String> {
    let mut sm = state.state_machine.lock().await;
    sm.set_live_source_path(&source, path)
}

#[tauri::command]
async fn set_live_source_focus_target(
    state: tauri::State<'_, AppState>,
    source: String,
    target: String,
) -> Result<(), String> {
    let mut sm = state.state_machine.lock().await;
    sm.set_live_source_focus_target(&source, target)
}

#[tauri::command]
async fn focus_live_source(
    state: tauri::State<'_, AppState>,
    source: String,
) -> Result<(), String> {
    let target = {
        let sm = state.state_machine.lock().await;
        sm.live_source_focus_target(&source)?
    };

    match focus::focus_target(&target) {
        Ok(()) => Ok(()),
        Err(e) => {
            log::warn!(
                "Failed to focus live source '{}' with target '{}': {}",
                source,
                target,
                e
            );
            Err(e)
        }
    }
}

#[tauri::command]
async fn set_live_source_prefix_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let mut sm = state.state_machine.lock().await;
    sm.set_live_source_prefix_enabled(enabled)
}

#[tauri::command]
async fn get_live_source_prefix_enabled(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let sm = state.state_machine.lock().await;
    Ok(sm.live_source_prefix_enabled())
}

#[tauri::command]
async fn get_usage_metrics(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let sm = state.state_machine.lock().await;
    Ok(sm.usage_metrics_payload())
}

#[tauri::command]
async fn get_pet_scale(state: tauri::State<'_, AppState>) -> Result<f64, String> {
    let sm = state.state_machine.lock().await;
    Ok(sm.pet_scale())
}

#[tauri::command]
async fn set_pet_scale(state: tauri::State<'_, AppState>, scale: f64) -> Result<f64, String> {
    let mut sm = state.state_machine.lock().await;
    Ok(sm.set_pet_scale(scale))
}

#[tauri::command]
async fn get_language(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let sm = state.state_machine.lock().await;
    Ok(sm.language().to_string())
}

#[tauri::command]
async fn set_language(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    language: String,
) -> Result<(), String> {
    let lang = language.clone();
    let mut sm = state.state_machine.lock().await;
    sm.set_language(language)?;
    drop(sm);
    tray::rebuild_tray(&app, &lang).map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    env_logger::init();

    tauri::Builder::default()
        .setup(|app| {
            let app_handle = app.handle().clone();

            // Initialize state machine
            let state_machine = Arc::new(Mutex::new(PetStateMachine::new(app_handle.clone())));
            let cancellation_token = CancellationToken::new();

            // Store app state
            app.manage(AppState {
                state_machine: state_machine.clone(),
                cancellation_token: cancellation_token.clone(),
            });

            // Setup system tray
            {
                let sm_block = tauri::async_runtime::block_on(state_machine.lock());
                let lang = sm_block.language().to_string();
                drop(sm_block);
                tray::setup_tray(app, &lang)?;
            }

            // Show pet window after setup (allow clicks for interaction)
            if let Some(window) = app.get_webview_window("pet") {
                let _ = window.show();
            }

            // Spawn WebSocket server
            let sm_clone = state_machine.clone();
            let ws_cancel = cancellation_token.clone();
            tauri::async_runtime::spawn(async move {
                let server = WebSocketServer::new(8765, sm_clone, ws_cancel);
                if let Err(e) = server.run().await {
                    log::error!("WebSocket server error: {}", e);
                }
            });

            // Mirror live Codex session activity into pet animations.
            let codex_sm = state_machine.clone();
            let monitor_cancel = cancellation_token.clone();
            tauri::async_runtime::spawn(async move {
                let monitor = CodexMonitor::new(codex_sm, monitor_cancel);
                monitor.run().await;
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::Destroyed) {
                let app = window.app_handle();
                if app.webview_windows().is_empty() {
                    if let Some(state) = app.try_state::<AppState>() {
                        state.cancellation_token.cancel();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_pet_list,
            get_user_pet_dir,
            load_pet,
            get_current_state,
            trigger_state,
            open_settings_window,
            trigger_notice,
            get_message_map,
            update_message_map,
            get_websocket_status,
            toggle_websocket,
            get_codex_monitor_status,
            toggle_codex_monitor,
            get_live_sources_status,
            toggle_live_source,
            set_live_source_path,
            set_live_source_focus_target,
            focus_live_source,
            set_live_source_prefix_enabled,
            get_live_source_prefix_enabled,
            get_usage_metrics,
            get_pet_scale,
            set_pet_scale,
            get_language,
            set_language,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
