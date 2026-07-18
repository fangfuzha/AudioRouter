use std::sync::{Arc, Mutex};

use app_core::controller::AppController;
use audio_core::router::Router;
use config::ConfigManager;
use windows_reactor::*;

mod app;
mod pane_bg_override;
mod tray;
mod window_utils;

fn app_config_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
        .join("AudioRouter")
}

fn main() -> windows_reactor::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("icu_segmenter", log::LevelFilter::Off)
        .format_timestamp_micros()
        .format_module_path(true)
        .init();

    let app_local_data_dir = app_config_dir();
    let config_manager = ConfigManager::load(Some(app_local_data_dir)).expect("load config");
    let router = Router::new();
    let controller = Arc::new(Mutex::new(AppController::new(config_manager, router)));

    {
        let mut c = controller.lock().unwrap();
        c.init();
    }

    {
        let c = controller.lock().unwrap();
        let i18n = c.i18n.clone();
        drop(c);
        if let Err(e) = tray::init_tray(i18n) {
            log::warn!("Failed to initialize system tray: {e}");
        }
    }

    // 从配置读取初始 backdrop，在窗口创建时直接应用。
    // 必须通过 App::backdrop 在窗口创建阶段设置，而非在组件 use_effect 中
    // 事后调用 set_backdrop——后者依赖的 ROOT_WINDOW 在 UI 首次挂载后才设置，
    // use_effect 执行时机可能早于该设置，导致 backdrop 被静默丢弃。
    let initial_backdrop = {
        let c = controller.lock().unwrap();
        c.backdrop()
    };
    let reactor_backdrop = match initial_backdrop {
        config::config::Backdrop::Mica => Backdrop::Mica,
        config::config::Backdrop::MicaAlt => Backdrop::MicaAlt,
        config::config::Backdrop::Acrylic => Backdrop::Acrylic,
    };

    // 窗口图标：使用项目根目录 assets/icon.ico
    let icon_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("assets")
        .join("icon.ico");

    log::info!("Starting AudioRouter WinUI3 GUI...");
    let mut app = App::new()
        .title("AudioRouter")
        .inner_size(980.0, 720.0)
        .inner_constraints(InnerConstraints {
            min_width: Some(640.0),
            min_height: Some(480.0),
            ..Default::default()
        })
        .backdrop(reactor_backdrop);

    if icon_path.is_file() {
        app = app.icon(icon_path.to_string_lossy().to_string());
    } else {
        log::warn!("Window icon not found: {}", icon_path.display());
    }

    app.run(move || app::RootComponent::new(Arc::clone(&controller)))
}
