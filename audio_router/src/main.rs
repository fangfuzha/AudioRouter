//! AudioRouter egui 桌面应用入口

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod controller;
mod fonts;
mod i18n;
mod slint_app;
mod tray;
mod update;
mod views;
mod widgets;

use std::sync::mpsc;

use audio_core::router::Router;
use config::ConfigManager;
use eframe::egui;

use app::AudioRouterApp;
use tray::{AppToTrayCommand, TrayToAppCommand};

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_micros()
        .init();

    let cmd_slint = std::env::args().any(|a| a == "--slint");
    let cmd_minimized = std::env::args().any(|a| a == "--minimized");

    let app_local_data_dir = app_config_dir();
    let config_manager =
        ConfigManager::load(Some(app_local_data_dir)).expect("Failed to load config");

    let cfg = config_manager.handle().read().clone();

    if cmd_slint {
        if let Err(e) = slint_app::run_slint_app(config_manager, Router::new()) {
            log::error!("Slint app failed: {e}");
        }
        return Ok(());
    }

    let initial_window_visible = !cmd_minimized && !cfg.general.minimized;

    // 创建通信通道
    let (tray_cmd_tx, tray_cmd_rx) = mpsc::channel::<AppToTrayCommand>();
    let (app_cmd_tx, app_cmd_rx) = mpsc::channel::<TrayToAppCommand>();

    // 启动托盘线程
    let tx_for_tray = app_cmd_tx.clone();
    let tray_locale = cfg.general.language.clone();
    std::thread::Builder::new()
        .name("tray-icon".into())
        .spawn(move || tray::run_tray(tray_cmd_rx, tx_for_tray, tray_locale))
        .expect("Failed to spawn tray thread");

    let router = Router::new();

    let mut native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 720.0])
            .with_min_inner_size([700.0, 500.0])
            .with_resizable(true)
            .with_decorations(true)
            .with_visible(initial_window_visible)
            .with_title("AudioRouter"),
        ..Default::default()
    };

    if let Some(icon) = load_window_icon() {
        native_options.viewport = native_options.viewport.with_icon(icon);
    }

    let tray_tx = tray_cmd_tx.clone();

    eframe::run_native(
        "AudioRouter",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(AudioRouterApp::new(
                cc,
                config_manager,
                router,
                app_cmd_rx,
                tray_tx,
                initial_window_visible,
            )))
        }),
    )?;

    let _ = tray_cmd_tx.send(AppToTrayCommand::Quit);
    Ok(())
}

fn app_config_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(std::path::PathBuf::from))
        .unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        })
        .join("AudioRouter")
}

fn load_window_icon() -> Option<egui::IconData> {
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("icon.png")));

    if let Some(ref path) = path
        && path.exists()
        && let Ok(img) = image::open(path)
    {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return Some(egui::IconData {
            width: w,
            height: h,
            rgba: rgba.into_raw(),
        });
    }

    // fallback 绿色图标
    let size = 32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let cx = x as f32 - size as f32 / 2.0;
            let cy = y as f32 - size as f32 / 2.0;
            if (cx * cx + cy * cy).sqrt() < size as f32 * 0.425 {
                rgba.extend_from_slice(&[43, 217, 127, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Some(egui::IconData {
        width: size,
        height: size,
        rgba,
    })
}
