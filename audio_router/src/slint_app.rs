//! Slint 前端入口。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use audio_core::device_watcher::DeviceWatcher;
use audio_core::router::{ChannelMode, Router};
use config::ConfigManager;
use slint::{
    CloseRequestResponse, ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel,
};

use crate::controller::AppController;
use crate::tray::{AppToTrayCommand, TrayToAppCommand};
use crate::update::UpdateStatus;

slint::include_modules!();

const CHANNEL_MODES: &[ChannelMode] = &[
    ChannelMode::Stereo,
    ChannelMode::LeftMono,
    ChannelMode::RightMono,
    ChannelMode::Mono,
    ChannelMode::Swap,
    ChannelMode::LeftOnly,
    ChannelMode::RightOnly,
];

pub fn run_slint_app(
    config_manager: ConfigManager,
    router: Router,
    tray_rx: mpsc::Receiver<TrayToAppCommand>,
    tray_tx: mpsc::Sender<AppToTrayCommand>,
    initial_window_visible: bool,
) -> anyhow::Result<()> {
    let ui = MainWindow::new()?;
    let controller = Rc::new(RefCell::new(AppController::new(config_manager, router)));

    ui.window().on_close_requested({
        let weak_ui = ui.as_weak();
        move || {
            if let Some(ui) = weak_ui.upgrade() {
                let _ = ui.hide();
            }
            CloseRequestResponse::KeepWindowShown
        }
    });

    spawn_tray_command_bridge(ui.as_weak(), tray_rx)?;
    let device_refresh_pending = spawn_device_watcher()?;
    let _device_refresh_timer = start_device_refresh_timer(
        ui.as_weak(),
        Rc::clone(&controller),
        Arc::clone(&device_refresh_pending),
    );
    spawn_update_check(ui.as_weak())?;
    register_callbacks(&ui, Rc::clone(&controller), tray_tx);

    {
        let mut controller = controller.borrow_mut();
        controller.init();
        update_main_window(&ui, &controller);
    }

    if initial_window_visible {
        ui.show()?;
    }

    slint::run_event_loop()?;
    Ok(())
}

fn spawn_tray_command_bridge(
    weak_ui: slint::Weak<MainWindow>,
    tray_rx: mpsc::Receiver<TrayToAppCommand>,
) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("slint-tray-commands".into())
        .spawn(move || {
            while let Ok(cmd) = tray_rx.recv() {
                let should_quit = matches!(cmd, TrayToAppCommand::Quit);
                let weak_ui = weak_ui.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak_ui.upgrade() {
                        match cmd {
                            TrayToAppCommand::ShowWindow => {
                                let _ = ui.show();
                            }
                            TrayToAppCommand::Quit => {
                                let _ = ui.hide();
                                let _ = slint::quit_event_loop();
                            }
                        }
                    }
                });
                if should_quit {
                    break;
                }
            }
        })?;
    Ok(())
}

fn spawn_device_watcher() -> anyhow::Result<Arc<AtomicBool>> {
    let refresh_pending = Arc::new(AtomicBool::new(false));
    let watcher_flag = Arc::clone(&refresh_pending);
    std::thread::Builder::new()
        .name("device-watcher".into())
        .spawn(move || match DeviceWatcher::start() {
            Ok((_watcher, rx)) => {
                while rx.recv().is_ok() {
                    watcher_flag.store(true, Ordering::Release);
                }
            }
            Err(e) => log::error!("Start device watcher failed: {e}"),
        })?;
    Ok(refresh_pending)
}

fn start_device_refresh_timer(
    weak_ui: slint::Weak<MainWindow>,
    controller: Rc<RefCell<AppController>>,
    refresh_pending: Arc<AtomicBool>,
) -> Timer {
    let timer = Timer::default();
    let mut last_poll = Instant::now();
    timer.start(TimerMode::Repeated, Duration::from_millis(700), move || {
        let should_poll = last_poll.elapsed() >= Duration::from_secs(10);
        if !refresh_pending.swap(false, Ordering::AcqRel) && !should_poll {
            return;
        }
        last_poll = Instant::now();
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = controller.borrow_mut();
            controller.refresh_devices();
            update_main_window(&ui, &controller);
        }
    });
    timer
}

fn spawn_update_check(weak_ui: slint::Weak<MainWindow>) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let status = crate::update::check_for_update();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.set_update_text(update_status_text(&status).into());
                }
            });
        })?;
    Ok(())
}

fn update_status_text(status: &UpdateStatus) -> String {
    match status {
        UpdateStatus::Available { latest_version, .. } => format!("发现新版本 {latest_version}"),
        UpdateStatus::Checking => "正在检查更新...".to_string(),
        UpdateStatus::Error(e) => format!("更新检查失败：{e}"),
        UpdateStatus::Idle | UpdateStatus::UpToDate => String::new(),
    }
}
fn register_callbacks(
    ui: &MainWindow,
    controller: Rc<RefCell<AppController>>,
    tray_tx: mpsc::Sender<AppToTrayCommand>,
) {
    let weak_ui = ui.as_weak();
    let routing_controller = Rc::clone(&controller);
    ui.on_toggle_routing(move || {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = routing_controller.borrow_mut();
            if controller.is_running {
                controller.stop_routing();
            } else {
                controller.start_routing();
            }
            update_main_window(&ui, &controller);
        }
    });

    let weak_ui = ui.as_weak();
    let source_controller = Rc::clone(&controller);
    ui.on_select_source(move |device_id| {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = source_controller.borrow_mut();
            controller.select_source_device(device_id.to_string());
            update_main_window(&ui, &controller);
        }
    });

    let weak_ui = ui.as_weak();
    let enabled_controller = Rc::clone(&controller);
    ui.on_set_output_enabled(move |device_id, enabled| {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = enabled_controller.borrow_mut();
            controller.set_output_enabled(device_id.as_str(), enabled);
            update_main_window(&ui, &controller);
        }
    });

    let weak_ui = ui.as_weak();
    let channel_controller = Rc::clone(&controller);
    ui.on_set_output_channel_mode(move |device_id, channel_index| {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = channel_controller.borrow_mut();
            if let Some(channel_mode) = channel_mode_from_index(channel_index) {
                controller.set_output_channel_mode(device_id.as_str(), channel_mode);
                update_main_window(&ui, &controller);
            }
        }
    });

    let weak_ui = ui.as_weak();
    let settings_controller = Rc::clone(&controller);
    ui.on_open_settings(move || {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = settings_controller.borrow_mut();
            controller.begin_settings_edit();
            update_main_window(&ui, &controller);
            ui.set_show_settings(true);
        }
    });

    let weak_ui = ui.as_weak();
    ui.on_cancel_settings(move || {
        if let Some(ui) = weak_ui.upgrade() {
            ui.set_show_settings(false);
        }
    });

    let weak_ui = ui.as_weak();
    let save_controller = Rc::clone(&controller);
    ui.on_save_settings(
        move |start_with_windows, minimized, auto_route, language_index| {
            if let Some(ui) = weak_ui.upgrade() {
                let mut controller = save_controller.borrow_mut();
                controller.draft_general.start_with_windows = start_with_windows;
                controller.draft_general.minimized = minimized;
                controller.draft_general.auto_route = auto_route;
                controller.draft_general.language =
                    language_code_from_index(language_index).to_string();
                if let Some(new_language) = controller.save_general_config() {
                    let _ = tray_tx.send(AppToTrayCommand::UpdateLanguage(new_language));
                }
                ui.set_show_settings(false);
                update_main_window(&ui, &controller);
            }
        },
    );
}

fn update_main_window(ui: &MainWindow, controller: &AppController) {
    ui.set_settings_state(settings_state(controller));
    ui.set_device_count(controller.devices.len() as i32);
    ui.set_is_running(controller.is_running);
    ui.set_status_text(controller.status_text.as_str().into());
    ui.set_source_devices(ModelRc::new(VecModel::from(source_rows(controller))));
    ui.set_output_devices(ModelRc::new(VecModel::from(output_rows(controller))));
    ui.set_channel_mode_options(ModelRc::new(VecModel::from(channel_mode_options(
        controller,
    ))));
}

fn source_rows(controller: &AppController) -> Vec<SourceDevice> {
    controller
        .devices
        .iter()
        .map(|device| SourceDevice {
            id: SharedString::from(device.id.as_str()),
            name: SharedString::from(device.friendly_name.as_str()),
            selected: controller.selected_source.as_deref() == Some(device.id.as_str()),
        })
        .collect()
}

fn output_rows(controller: &AppController) -> Vec<DeviceRow> {
    let cfg = controller.config_manager.handle().read().clone();
    controller
        .filtered_target_devices()
        .into_iter()
        .map(|device| {
            let output = cfg.outputs.iter().find(|o| o.device_id == device.id);
            let enabled = output.map(|o| o.enabled).unwrap_or(false);
            let channel_mode =
                ChannelMode::from_config(output.and_then(|o| o.channel_mode.as_deref()));
            DeviceRow {
                id: SharedString::from(device.id.as_str()),
                name: SharedString::from(device.friendly_name.as_str()),
                enabled,
                channel_mode: channel_mode_index(channel_mode),
            }
        })
        .collect()
}

fn channel_mode_index(channel_mode: ChannelMode) -> i32 {
    CHANNEL_MODES
        .iter()
        .position(|mode| *mode == channel_mode)
        .unwrap_or(0) as i32
}

fn channel_mode_options(controller: &AppController) -> Vec<SharedString> {
    CHANNEL_MODES
        .iter()
        .map(|mode| SharedString::from(channel_mode_label(controller, *mode).as_str()))
        .collect()
}

fn channel_mode_label(controller: &AppController, channel_mode: ChannelMode) -> String {
    let key = match channel_mode {
        ChannelMode::Stereo => "channelModes.Stereo",
        ChannelMode::LeftMono => "channelModes.LeftMono",
        ChannelMode::RightMono => "channelModes.RightMono",
        ChannelMode::Mono => "channelModes.Mono",
        ChannelMode::Swap => "channelModes.Swap",
        ChannelMode::LeftOnly => "channelModes.LeftOnly",
        ChannelMode::RightOnly => "channelModes.RightOnly",
    };
    controller.i18n.t(key).to_string()
}

fn channel_mode_from_index(index: i32) -> Option<ChannelMode> {
    CHANNEL_MODES.get(index as usize).copied()
}

fn settings_state(controller: &AppController) -> SettingsState {
    SettingsState {
        start_with_windows: controller.draft_general.start_with_windows,
        minimized: controller.draft_general.minimized,
        auto_route: controller.draft_general.auto_route,
        language_index: language_index(&controller.draft_general.language),
    }
}

fn language_index(language: &str) -> i32 {
    match language {
        "zh" => 1,
        _ => 0,
    }
}

fn language_code_from_index(index: i32) -> &'static str {
    match index {
        1 => "zh",
        _ => "en",
    }
}
