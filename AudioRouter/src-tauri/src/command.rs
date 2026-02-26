use crate::{AppState, DeviceLog, RoutingParams};
use audio_core::com_service::device::get_all_output_devices;
use audio_core::router::RouterConfig;
use config::config::{General, Output};
use config::Config;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

#[tauri::command]
#[specta::specta]
pub async fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    let cfg = state.config_manager.handle().read().clone();
    Ok(cfg)
}

#[tauri::command]
#[specta::specta]
pub async fn update_general_config(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    general: General,
) -> Result<(), String> {
    state
        .config_manager
        .update(|cfg| {
            cfg.general = general.clone();
        })
        .map_err(|e| e.to_string())?;

    // Handle autostart plugin
    let autostart_manager = app.autolaunch();
    if general.start_with_windows {
        let _ = autostart_manager.enable();
    } else {
        let _ = autostart_manager.disable();
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_devices() -> Result<Vec<DeviceLog>, String> {
    let devices = get_all_output_devices().map_err(|e| e.to_string())?;
    Ok(devices
        .into_iter()
        .map(|d| DeviceLog {
            id: d.id,
            name: d.friendly_name,
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn start_routing(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    params: RoutingParams,
) -> Result<(), String> {
    // Start routing
    let cfg = RouterConfig {
        source_device_id: params.source_id,
        target_config: params.targets,
    };

    fn noop_cb(_: &[f32], _: u32, _: u16) {}

    let res = state
        .router
        .start_with_callback(cfg, Arc::new(noop_cb))
        .map_err(|e| e.to_string());

    if res.is_ok() {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.emit("routing-started", true);
        }
    }

    res
}

#[tauri::command]
#[specta::specta]
pub async fn stop_routing(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let res = state.router.stop().map_err(|e| e.to_string());
    if res.is_ok() {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.emit("routing-stopped", true);
        }
    }
    res
}

#[tauri::command]
#[specta::specta]
pub fn update_tray_menu(app: AppHandle, labels: HashMap<String, String>) -> Result<(), String> {
    // Build menu items using provided localized labels
    let quit_label = labels.get("quit").map(|s| s.as_str()).unwrap_or("Quit");
    let show_label = labels.get("show").map(|s| s.as_str()).unwrap_or("Show");

    let quit_i = MenuItem::with_id(&app, "quit", quit_label, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let show_i = MenuItem::with_id(&app, "show", show_label, true, None::<&str>)
        .map_err(|e| e.to_string())?;

    let tray_menu = Menu::with_items(&app, &[&show_i, &quit_i]).map_err(|e| e.to_string())?;

    // Update the existing tray icon's menu if it exists
    let state = app.state::<crate::AppState>();
    let mut guard = state.tray.lock().unwrap();
    let tray_icon = &mut *guard;
    tray_icon
        .set_menu(Some(tray_menu))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn save_routing_config(
    state: State<'_, AppState>,
    source_device_id: String,
    outputs: Vec<Output>,
) -> Result<(), String> {
    state
        .config_manager
        .update(|cfg| {
            cfg.source_device_id = source_device_id;
            cfg.outputs = outputs;
        })
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn get_language(state: State<'_, AppState>) -> Result<String, String> {
    let cfg = state.config_manager.handle().read().clone();
    Ok(cfg.general.language)
}

#[tauri::command]
#[specta::specta]
pub async fn get_ui_data(state: State<'_, AppState>) -> Result<crate::UiData, String> {
    let cfg = state.config_manager.handle().read().clone();
    let devices = get_all_output_devices().map_err(|e| e.to_string())?;

    // Create a map of device_id to output config for quick lookup
    let output_map: HashMap<String, &Output> = cfg
        .outputs
        .iter()
        .map(|o| (o.device_id.clone(), o))
        .collect();

    let target_devices = devices
        .into_iter()
        .map(|d| {
            let output = output_map.get(&d.id);
            crate::UiDataTargetDevice {
                id: d.id,
                name: d.friendly_name,
                mix_mode: output
                    .map(|o| o.channel_mode)
                    .unwrap_or(config::ChannelMixMode::Stereo),
                enabled: output.map(|o| o.enabled).unwrap_or(false),
            }
        })
        .collect();

    let source_device = cfg.source_device_id;
    let is_running = state.router.is_running();

    Ok(crate::UiData {
        source_device: Some(source_device),
        target_devices,
        is_running,
    })
}
