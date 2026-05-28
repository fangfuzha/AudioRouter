//! AudioRouter Slint 桌面应用入口

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod controller;
mod i18n;
mod slint_app;
mod tray;
mod update;

use std::sync::mpsc;

use audio_core::router::Router;
use config::ConfigManager;
use tray::{AppToTrayCommand, TrayToAppCommand};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("icu_segmenter", log::LevelFilter::Off)
        .format_timestamp_micros()
        .format_module_path(true)
        .init();

    let cmd_minimized = std::env::args().any(|a| a == "--minimized");

    let app_local_data_dir = app_config_dir();
    let config_manager = ConfigManager::load(Some(app_local_data_dir))?;
    let cfg = config_manager.handle().read().clone();
    let initial_window_visible = !cmd_minimized && !cfg.general.minimized;

    let (tray_cmd_tx, tray_cmd_rx) = mpsc::channel::<AppToTrayCommand>();
    let (app_cmd_tx, app_cmd_rx) = mpsc::channel::<TrayToAppCommand>();

    std::thread::Builder::new()
        .name("tray-icon".into())
        .spawn(move || tray::run_tray(tray_cmd_rx, app_cmd_tx))?;

    let router = Router::new();
    slint_app::run_slint_app(
        config_manager,
        router,
        app_cmd_rx,
        tray_cmd_tx.clone(),
        initial_window_visible,
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
