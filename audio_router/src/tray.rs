//! 系统托盘模块

use std::sync::mpsc;
use std::time::Duration;
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};

#[derive(Debug, Clone, Copy)]
pub struct TrayAnchorRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct TrayClickPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone)]
pub enum TrayToAppCommand {
    ShowWindow,
    ShowTrayPopup {
        anchor_rect: TrayAnchorRect,
        click_pos: TrayClickPoint,
    },
    Quit,
}

#[derive(Debug, Clone)]
pub enum AppToTrayCommand {
    Quit,
    UpdateLanguage(String),
}

/// 创建并运行系统托盘（在独立线程中运行）
pub fn run_tray(rx: mpsc::Receiver<AppToTrayCommand>, tx: mpsc::Sender<TrayToAppCommand>) {
    let icon = match load_tray_icon() {
        Some(icon) => icon,
        None => {
            log::error!("Failed to load tray icon");
            return;
        }
    };

    let tray_icon = TrayIconBuilder::new()
        .with_tooltip("AudioRouter")
        .with_icon(icon)
        .build();

    match tray_icon {
        Ok(_tray_icon) => {
            // 点击事件
            let click_tx = tx.clone();
            TrayIconEvent::set_event_handler(Some(move |event| match event {
                TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } => {
                    log::info!("Tray icon left click released: show window");
                    let _ = click_tx.send(TrayToAppCommand::ShowWindow);
                }
                TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Right,
                    button_state: tray_icon::MouseButtonState::Up,
                    position,
                    rect,
                    ..
                } => {
                    log::info!("Tray icon right click released: show tray popup");
                    let _ = click_tx.send(TrayToAppCommand::ShowTrayPopup {
                        anchor_rect: TrayAnchorRect {
                            x: rect.position.x as i32,
                            y: rect.position.y as i32,
                            width: rect.size.width as i32,
                            height: rect.size.height as i32,
                        },
                        click_pos: TrayClickPoint {
                            x: position.x as i32,
                            y: position.y as i32,
                        },
                    });
                }
                _ => {}
            }));

            keep_tray_alive(_tray_icon, rx);
        }
        Err(e) => log::error!("Failed to create tray icon: {e}"),
    }
}

fn keep_tray_alive(_tray_icon: TrayIcon, rx: mpsc::Receiver<AppToTrayCommand>) {
    loop {
        pump_tray_event_loop();

        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(AppToTrayCommand::Quit) => break,
            Ok(AppToTrayCommand::UpdateLanguage(_lang)) => {
                // tray-icon 菜单文本无法稳定原地更新，下一次启动会应用语言。
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(target_os = "windows")]
fn pump_tray_event_loop() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn pump_tray_event_loop() {}

fn load_tray_icon() -> Option<tray_icon::Icon> {
    let icon_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("icon.png")));

    if let Some(ref path) = icon_path
        && path.exists()
    {
        match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                return tray_icon::Icon::from_rgba(rgba.into_raw(), w, h).ok();
            }
            Err(e) => log::warn!("Failed to load tray icon: {e}"),
        }
    }

    load_icon_from_png_bytes(include_bytes!("../assets/icon.png"))
}

fn load_icon_from_png_bytes(bytes: &[u8]) -> Option<tray_icon::Icon> {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            tray_icon::Icon::from_rgba(rgba.into_raw(), w, h).ok()
        }
        Err(e) => {
            log::error!("Failed to load embedded tray icon: {e}");
            None
        }
    }
}
