//! Slint 前端入口。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use audio_core::device_watcher::DeviceWatcher;
use audio_core::router::{ChannelMode, Router};
use config::ConfigManager;
use slint::winit_030::WinitWindowAccessor;
use slint::{
    CloseRequestResponse, ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel,
};

use crate::controller::AppController;
use crate::tray::{AppToTrayCommand, TrayAnchorRect, TrayToAppCommand};
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

#[derive(Clone)]
struct TrayPopupHandles {
    main_window: slint::Weak<MainWindow>,
    popup_window: slint::Weak<TrayPopupWindow>,
}

pub fn run_slint_app(
    config_manager: ConfigManager,
    router: Router,
    tray_rx: mpsc::Receiver<TrayToAppCommand>,
    tray_tx: mpsc::Sender<AppToTrayCommand>,
    initial_window_visible: bool,
) -> anyhow::Result<()> {
    let ui = MainWindow::new()?;
    let tray_popup = TrayPopupWindow::new()?;
    let controller = Rc::new(RefCell::new(AppController::new(config_manager, router)));

    // 注意：弹窗的 Win32 窗口属性（圆角区域、WS_EX_TOOLWINDOW）不能在此处设置，
    // 因为 Slint 使用延迟窗口创建，此时 HWND 尚不存在。
    // 这些设置已移至 spawn_tray_command_bridge 的 ShowTrayPopup 分支中，
    // 在 popup.show() 之后执行（此时 HWND 已创建）。

    ui.window().on_close_requested({
        let weak_ui = ui.as_weak();
        move || {
            if let Some(ui) = weak_ui.upgrade() {
                let _ = ui.hide();
            }
            CloseRequestResponse::HideWindow
        }
    });

    spawn_tray_command_bridge(
        TrayPopupHandles {
            main_window: ui.as_weak(),
            popup_window: tray_popup.as_weak(),
        },
        tray_rx,
    )?;
    let device_refresh_pending = spawn_device_watcher()?;
    let _device_refresh_timer = start_device_refresh_timer(
        ui.as_weak(),
        Rc::clone(&controller),
        Arc::clone(&device_refresh_pending),
    );
    spawn_update_check(ui.as_weak(), controller.borrow().i18n.locale().to_string())?;

    // 托盘弹窗回调
    {
        let weak_popup = tray_popup.as_weak();
        let weak_ui = ui.as_weak();
        tray_popup.on_show_main_window(move || {
            if let Some(popup) = weak_popup.upgrade() {
                let _ = popup.hide();
            }
            if let Some(ui) = weak_ui.upgrade() {
                let _ = show_and_focus_window(&ui);
            }
        });
    }
    {
        let weak_popup = tray_popup.as_weak();
        let popup_controller = Rc::clone(&controller);
        let weak_ui = ui.as_weak();
        tray_popup.on_toggle_routing(move || {
            if let Some(popup) = weak_popup.upgrade() {
                let mut controller = popup_controller.borrow_mut();
                if controller.is_running {
                    controller.stop_routing();
                } else {
                    controller.start_routing();
                }
                // 同步状态到主窗口和弹窗
                if let Some(ui) = weak_ui.upgrade() {
                    update_main_window(&ui, &controller);
                }
                popup.set_routing_running(controller.is_running);
                let _ = popup.hide();
            }
        });
    }
    {
        let weak_popup = tray_popup.as_weak();
        tray_popup.on_quit_app(move || {
            if let Some(popup) = weak_popup.upgrade() {
                let _ = popup.hide();
            }
            let _ = slint::quit_event_loop();
        });
    }
    // Esc 关闭弹窗并归还焦点到主窗口
    {
        let weak_popup = tray_popup.as_weak();
        let weak_ui = ui.as_weak();
        tray_popup.on_request_close(move || {
            if let Some(popup) = weak_popup.upgrade() {
                let _ = popup.hide();
            }
            // 归还焦点到主窗口（如果可见）
            if let Some(ui) = weak_ui.upgrade() {
                if ui.window().is_visible() {
                    let _ = show_and_focus_window(&ui);
                }
            }
        });
    }
    // 失焦关闭：用 Timer 轮询弹窗焦点状态（200ms 间隔）
    let _popup_focus_timer = {
        let popup_timer = Timer::default();
        let weak_popup = tray_popup.as_weak();
        let focus_state = Rc::new(RefCell::new(false));
        popup_timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
            if let Some(popup) = weak_popup.upgrade() {
                if popup.window().is_visible() {
                    let has_focus = popup
                        .window()
                        .with_winit_window(|w| w.has_focus())
                        .unwrap_or(false);
                    let was_focused = *focus_state.borrow();
                    // 从有焦点变为无焦点 → 关闭弹窗
                    if was_focused && !has_focus {
                        let _ = popup.hide();
                    }
                    *focus_state.borrow_mut() = has_focus;
                } else {
                    *focus_state.borrow_mut() = false;
                }
            }
        });
        popup_timer
    };

    register_callbacks(&ui, Rc::clone(&controller), tray_tx);

    {
        let mut controller = controller.borrow_mut();
        controller.init();
        update_main_window(&ui, &controller);
    }

    if initial_window_visible {
        show_and_focus_window(&ui)?;
    }

    slint::run_event_loop_until_quit()?;
    Ok(())
}

/// 统一处理窗口拉起逻辑，供启动阶段与托盘事件复用。
fn show_and_focus_window(ui: &MainWindow) -> Result<(), slint::PlatformError> {
    log::debug!("Show and focus window requested");
    ui.show()?;

    let window = ui.window();
    let was_minimized = window.is_minimized();
    log::debug!("Window minimized before restore: {}", was_minimized);
    if was_minimized {
        window.set_minimized(false);
    }

    let used_winit = window.with_winit_window(|winit_window| {
        winit_window.focus_window();
        winit_window.request_user_attention(None);
    });
    log::debug!("Winit activation available: {}", used_winit.is_some());

    Ok(())
}

fn spawn_tray_command_bridge(
    handles: TrayPopupHandles,
    tray_rx: mpsc::Receiver<TrayToAppCommand>,
) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("slint-tray-commands".into())
        .spawn(move || {
            while let Ok(cmd) = tray_rx.recv() {
                let handles = handles.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    match cmd {
                        TrayToAppCommand::ShowWindow => {
                            if let Some(ui) = handles.main_window.upgrade() {
                                if let Err(e) = show_and_focus_window(&ui) {
                                    log::error!("Failed to show main window from tray: {e}");
                                }
                            }
                        }
                        TrayToAppCommand::ShowTrayPopup { anchor_rect } => {
                            if let Some(popup) = handles.popup_window.upgrade() {
                                // 同步路由运行状态到弹窗
                                let is_running = handles
                                    .main_window
                                    .upgrade()
                                    .map(|ui| ui.get_is_running())
                                    .unwrap_or(false);
                                popup.set_routing_running(is_running);

                                // 获取 scale_factor 并计算物理像素弹窗尺寸
                                let scale = popup
                                    .window()
                                    .with_winit_window(|w| w.scale_factor())
                                    .unwrap_or(1.0);
                                let popup_phys_w = (180.0 * scale) as i32;
                                let popup_phys_h = (160.0 * scale) as i32;

                                // 通过 available_monitors 查找包含锚点的显示器尺寸
                                let screen_size = popup
                                    .window()
                                    .with_winit_window(|winit_window| {
                                        let ax = anchor_rect.x + anchor_rect.width / 2;
                                        let ay = anchor_rect.y + anchor_rect.height / 2;
                                        // 查找包含锚点中心的显示器
                                        let best = winit_window
                                            .available_monitors()
                                            .find(|m| {
                                                let pos = m.position();
                                                let size = m.size();
                                                let mx = pos.x as i32;
                                                let my = pos.y as i32;
                                                ax >= mx
                                                    && ax < mx + size.width as i32
                                                    && ay >= my
                                                    && ay < my + size.height as i32
                                            })
                                            .or_else(|| winit_window.current_monitor());
                                        best.map(|m| {
                                            let s = m.size();
                                            (s.width as i32, s.height as i32)
                                        })
                                    })
                                    .flatten()
                                    .unwrap_or((
                                        anchor_rect.x + anchor_rect.width + 220,
                                        anchor_rect.y + anchor_rect.height + 200,
                                    ));

                                let gap = 8;
                                let (px, py) = compute_tray_popup_position(
                                    anchor_rect,
                                    (popup_phys_w, popup_phys_h),
                                    screen_size,
                                    gap,
                                );

                                // 已显示时仅更新位置，避免闪烁
                                if popup.window().is_visible() {
                                    popup
                                        .window()
                                        .set_position(slint::PhysicalPosition::new(px, py));
                                } else if let Err(e) = popup.show() {
                                    log::error!("Show tray popup failed: {e}");
                                } else {
                                    popup
                                        .window()
                                        .set_position(slint::PhysicalPosition::new(px, py));

                                    // 此时 HWND 已创建，设置窗口属性
                                    popup.window().with_winit_window(|w| {
                                        use slint::winit_030::winit::raw_window_handle::{
                                            HasWindowHandle, RawWindowHandle,
                                        };
                                        use windows::Win32::Foundation::HWND;
                                        use windows::Win32::Graphics::Gdi::{
                                            CreateRoundRectRgn, DeleteObject, SetWindowRgn,
                                        };
                                        use windows::Win32::UI::WindowsAndMessaging::{
                                            GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos,
                                            GWL_EXSTYLE, SWP_FRAMECHANGED, SWP_NOMOVE,
                                            SWP_NOZORDER, SWP_NOSIZE, WS_EX_TOOLWINDOW,
                                        };

                                        if let Ok(handle) = w.window_handle() {
                                            if let RawWindowHandle::Win32(win32) =
                                                handle.as_raw()
                                            {
                                                let hwnd =
                                                    HWND(win32.hwnd.get() as *mut _);

                                                // 设置 WS_EX_TOOLWINDOW + SWP_FRAMECHANGED
                                                unsafe {
                                                    let ex_style = GetWindowLongPtrW(
                                                        hwnd, GWL_EXSTYLE,
                                                    );
                                                    SetWindowLongPtrW(
                                                        hwnd,
                                                        GWL_EXSTYLE,
                                                        ex_style
                                                            | WS_EX_TOOLWINDOW.0 as isize,
                                                    );
                                                    let _ = SetWindowPos(
                                                        hwnd,
                                                        HWND::default(),
                                                        0,
                                                        0,
                                                        0,
                                                        0,
                                                        SWP_NOMOVE
                                                            | SWP_NOSIZE
                                                            | SWP_NOZORDER
                                                            | SWP_FRAMECHANGED,
                                                    );
                                                }

                                                // 设置圆角窗口区域
                                                let scale = w.scale_factor();
                                                let pw = (180.0 * scale) as i32;
                                                let ph = (160.0 * scale) as i32;
                                                let radius = (8.0 * scale) as i32;
                                                let region = unsafe {
                                                    CreateRoundRectRgn(
                                                        0,
                                                        0,
                                                        pw,
                                                        ph,
                                                        radius * 2,
                                                        radius * 2,
                                                    )
                                                };
                                                unsafe {
                                                    if SetWindowRgn(hwnd, region, true) == 0 {
                                                        let _ = DeleteObject(region);
                                                        log::warn!("SetWindowRgn failed");
                                                    }
                                                }
                                            }
                                        }
                                    });

                                    // 强制焦点以启用失焦关闭
                                    popup.window().with_winit_window(|w| {
                                        w.focus_window();
                                    });
                                }
                                log::debug!("Tray popup shown at ({px}, {py})");
                            } else {
                                log::warn!(
                                    "Show tray popup requested after popup window was released"
                                );
                            }
                        }
                    }
                });
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

fn spawn_update_check(weak_ui: slint::Weak<MainWindow>, locale: String) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let status = crate::update::check_for_update();
            let i18n = crate::i18n::I18n::new(&locale);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.set_update_text(update_status_text(&status, &i18n).into());
                    if let UpdateStatus::Available { html_url, .. } = &status {
                        ui.set_update_url(html_url.as_str().into());
                    }
                }
            });
        })?;
    Ok(())
}

fn update_status_text(status: &UpdateStatus, i18n: &crate::i18n::I18n) -> String {
    match status {
        UpdateStatus::Available { latest_version, .. } => i18n
            .t("UpdateAvailableVersion")
            .replace("{v}", latest_version),
        UpdateStatus::Error(e) => i18n.t("UpdateCheckFailed").replace("{e}", &e.to_string()),
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

    // 点击更新提示 → 打开浏览器跳转下载页
    let weak_ui = ui.as_weak();
    ui.on_open_update_url(move || {
        if let Some(ui) = weak_ui.upgrade() {
            let url = ui.get_update_url();
            if !url.is_empty() {
                let _ = open::that(url.as_str());
            }
        }
    });
}

fn update_main_window(ui: &MainWindow, controller: &AppController) {
    let i18n = &controller.i18n;
    ui.set_settings_state(settings_state(controller));
    ui.set_device_count(controller.devices.len() as i32);
    ui.set_is_running(controller.is_running);
    ui.set_status_text(controller.status_text.as_str().into());
    ui.set_source_devices(ModelRc::new(VecModel::from(source_rows(controller))));
    ui.set_output_devices(ModelRc::new(VecModel::from(output_rows(controller))));
    ui.set_channel_mode_options(ModelRc::new(VecModel::from(channel_mode_options(
        controller,
    ))));

    // i18n 翻译文本
    ui.set_i18n_status_running(i18n.t("Running").into());
    ui.set_i18n_status_ready(i18n.t("StatusReady").into());
    ui.set_i18n_device_count_text(
        i18n.t("FoundDevices")
            .replace("{count}", &controller.devices.len().to_string())
            .into(),
    );
    ui.set_i18n_settings(i18n.t("Settings").into());
    ui.set_i18n_settings_title(i18n.t("SettingsTitle").into());
    ui.set_i18n_source_device(i18n.t("SourceDevice").into());
    ui.set_i18n_output_devices(i18n.t("OutputDevices").into());
    ui.set_i18n_start(i18n.t("Start").into());
    ui.set_i18n_stop(i18n.t("Stop").into());
    ui.set_i18n_start_with_windows(i18n.t("StartWithWindows").into());
    ui.set_i18n_start_minimized(i18n.t("StartMinimized").into());
    ui.set_i18n_auto_route(i18n.t("AutoRoute").into());
    ui.set_i18n_language(i18n.t("Language").into());
    ui.set_i18n_cancel(i18n.t("Cancel").into());
    ui.set_i18n_save(i18n.t("Save").into());
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

/// 计算托盘弹窗在屏幕上的显示位置。
///
/// 默认在托盘图标上方偏移 `gap` 像素显示；当上方空间不足时改为显示在图标下方；
/// 当右侧超出屏幕时向左回退；当左侧超出屏幕时夹到 0。
fn compute_tray_popup_position(
    anchor_rect: TrayAnchorRect,
    popup_size: (i32, i32),
    screen_size: (i32, i32),
    gap: i32,
) -> (i32, i32) {
    let (popup_w, popup_h) = popup_size;
    let (screen_w, screen_h) = screen_size;

    // X: 默认对齐锚点左边缘
    let mut x = anchor_rect.x;
    // 右侧超出 → 向左回退
    if x + popup_w > screen_w {
        x = screen_w - popup_w;
    }
    // 左侧超出 → 夹到 0
    if x < 0 {
        x = 0;
    }

    // Y: 默认在锚点上方
    let mut y = anchor_rect.y - popup_h - gap;
    // 上方空间不足 → 改为显示在锚点下方
    if y < 0 {
        y = anchor_rect.y + anchor_rect.height + gap;
    }
    // 下方也溢出 → 贴顶
    if y + popup_h > screen_h {
        y = 0;
    }

    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_popup_above_tray_icon_when_space_is_available() {
        let anchor = TrayAnchorRect {
            x: 1500,
            y: 900,
            width: 40,
            height: 40,
        };
        let pos = compute_tray_popup_position(anchor, (220, 160), (1920, 1080), 8);
        // 1500 < 1920-220, ok; y = 900-160-8 = 732
        assert_eq!(pos, (1500, 732));
    }

    #[test]
    fn clamps_x_when_popup_exceeds_right_edge() {
        let anchor = TrayAnchorRect {
            x: 1800,
            y: 900,
            width: 40,
            height: 40,
        };
        let pos = compute_tray_popup_position(anchor, (220, 160), (1920, 1080), 8);
        // 1800+220 > 1920 → x = 1920-220 = 1700
        assert_eq!(pos, (1700, 732));
    }

    #[test]
    fn falls_below_anchor_when_not_enough_space_above() {
        let anchor = TrayAnchorRect {
            x: 100,
            y: 50,
            width: 40,
            height: 40,
        };
        let pos = compute_tray_popup_position(anchor, (220, 160), (1920, 1080), 8);
        // y = 50-160-8 = -118 < 0 → y = 50+40+8 = 98
        assert_eq!(pos, (100, 98));
    }

    #[test]
    fn clamps_x_to_zero_when_screen_too_narrow() {
        let anchor = TrayAnchorRect {
            x: 10,
            y: 900,
            width: 40,
            height: 40,
        };
        let pos = compute_tray_popup_position(anchor, (300, 160), (200, 1080), 8);
        // x=10, 10+300 > 200 → x = 200-300 = -100 < 0 → x = 0
        assert_eq!(pos, (0, 732));
    }
}
