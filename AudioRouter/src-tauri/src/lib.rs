use audio_core::router::{Router, RouterConfig};
use config::ChannelMixMode;
use config::ConfigManager;
use specta_typescript::Typescript;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::{
    Emitter, Manager,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::MacosLauncher;
use tauri_specta::{Builder, collect_commands};

// moving tauri commands to a separate module for clarity
mod command;
mod types;
use types::*;

struct AppState {
    router: Router,
    config_manager: ConfigManager,
    tray: Mutex<tauri::tray::TrayIcon>,
}

/// Show, unminimize and focus a Webview window if it exists.
fn show_and_focus_window(app: &tauri::AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Handle auto routing based on configuration
fn handle_auto_routing(router: &Router, config: &config::Config, app: &tauri::App) {
    if config.general.auto_route && !config.source_device_id.is_empty() {
        let target_config: Vec<(String, ChannelMixMode)> = config
            .outputs
            .iter()
            .filter(|o| o.enabled)
            .map(|o| (o.device_id.clone(), o.channel_mode.clone()))
            .collect();

        if !target_config.is_empty() {
            let cfg = RouterConfig {
                source_device_id: Some(config.source_device_id.clone()),
                target_config,
            };

            fn noop_cb(_: &[f32], _: u32, _: u16) {}
            let res = router.start_with_callback(cfg, Arc::new(noop_cb));
            if res.is_ok() {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("routing-started", true);
                }
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Identify command line parameter --minimized (passed by autostart launcher)
    let cmd_minimized = std::env::args().any(|a| a == "--minimized");

    let builder = Builder::<tauri::Wry>::new().commands(collect_commands![
        command::get_devices,
        command::start_routing,
        command::stop_routing,
        command::get_config,
        command::update_general_config,
        command::save_routing_config,
        command::get_language,
        command::get_ui_data,
        command::update_tray_menu
    ]);

    #[cfg(debug_assertions)]
    builder
        .export(Typescript::default(), "../src/generated/bindings.ts")
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        // Auto start plugin, add --minimized arg when Startup with system
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(move |app| {
            let app_local_data_dir = app.path().app_local_data_dir()?;
            let router = Router::new();
            let config_manager = ConfigManager::load(Some(app_local_data_dir))?;
            let config = config_manager.handle().read().clone();

            // Window starts invisible by default (to avoid flash). Show it only when we should not start minimized.
            if !cmd_minimized && !config.general.minimized {
                show_and_focus_window(app.handle(), "main");
            }

            handle_auto_routing(&router, &config, app);

            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let tray_icon = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        show_and_focus_window(app, "main");
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        show_and_focus_window(&app, "main");
                    }
                })
                .build(app)?;

            app.manage(AppState {
                router,
                config_manager,
                tray: Mutex::new(tray_icon),
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(builder.invoke_handler())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
