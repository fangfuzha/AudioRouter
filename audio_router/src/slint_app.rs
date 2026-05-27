//! Slint 前端入口。当前作为迁移中的独立 UI 壳，后续会替换 egui 入口。

use std::cell::RefCell;
use std::rc::Rc;

use audio_core::router::{ChannelMode, Router};
use config::ConfigManager;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::controller::AppController;

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
    tray_tx: Option<std::sync::mpsc::Sender<crate::tray::AppToTrayCommand>>,
) -> anyhow::Result<()> {
    let ui = MainWindow::new()?;
    let controller = Rc::new(RefCell::new(AppController::new(config_manager, router)));

    {
        let mut controller = controller.borrow_mut();
        controller.init();
        update_main_window(&ui, &controller);
    }

    let weak_ui = ui.as_weak();
    let refresh_controller = Rc::clone(&controller);
    ui.on_refresh_devices(move || {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = refresh_controller.borrow_mut();
            controller.refresh_devices();
            update_main_window(&ui, &controller);
        }
    });

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
    ui.on_next_channel_mode(move |device_id| {
        if let Some(ui) = weak_ui.upgrade() {
            let mut controller = channel_controller.borrow_mut();
            if let Some(next_mode) = next_channel_mode(&controller, device_id.as_str()) {
                controller.set_output_channel_mode(device_id.as_str(), next_mode);
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
                if let Some(new_language) = controller.save_general_config()
                    && let Some(tray_tx) = &tray_tx
                {
                    let _ =
                        tray_tx.send(crate::tray::AppToTrayCommand::UpdateLanguage(new_language));
                }
                ui.set_show_settings(false);
                update_main_window(&ui, &controller);
            }
        },
    );

    ui.run()?;
    Ok(())
}

fn update_main_window(ui: &MainWindow, controller: &AppController) {
    ui.set_settings_state(settings_state(controller));
    ui.set_device_count(controller.devices.len() as i32);
    ui.set_is_running(controller.is_running);
    ui.set_status_text(controller.status_text.as_str().into());
    ui.set_source_devices(ModelRc::new(VecModel::from(source_rows(controller))));
    ui.set_output_devices(ModelRc::new(VecModel::from(output_rows(controller))));
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
                channel_label: SharedString::from(
                    channel_mode_label(controller, channel_mode).as_str(),
                ),
            }
        })
        .collect()
}

fn next_channel_mode(controller: &AppController, device_id: &str) -> Option<ChannelMode> {
    let cfg = controller.config_manager.handle().read().clone();
    let current = cfg
        .outputs
        .iter()
        .find(|o| o.device_id == device_id)
        .map(|o| ChannelMode::from_config(o.channel_mode.as_deref()))
        .unwrap_or_default();
    let index = CHANNEL_MODES.iter().position(|mode| *mode == current)?;
    Some(CHANNEL_MODES[(index + 1) % CHANNEL_MODES.len()])
}

fn channel_mode_index(channel_mode: ChannelMode) -> i32 {
    CHANNEL_MODES
        .iter()
        .position(|mode| *mode == channel_mode)
        .unwrap_or(0) as i32
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
