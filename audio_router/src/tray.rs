//! 系统托盘模块

use std::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone)]
pub enum TrayToAppCommand {
    ShowWindow,
    Quit,
}

#[derive(Debug, Clone)]
pub enum AppToTrayCommand {
    Quit,
    UpdateLanguage(String),
}

/// 创建并运行系统托盘（在独立线程中运行）
pub fn run_tray(
    rx: mpsc::Receiver<AppToTrayCommand>,
    tx: mpsc::Sender<TrayToAppCommand>,
    locale: String,
) {
    let icon = match load_tray_icon() {
        Some(icon) => icon,
        None => {
            log::error!("Failed to load tray icon");
            return;
        }
    };

    let tray_menu = Menu::new();
    let show_item = MenuItem::new(tray_text(&locale, "Show"), true, None);
    let quit_item = MenuItem::new(tray_text(&locale, "Exit"), true, None);

    // 提取 ID 为独立的所有权值（避免 MenuItem 的非 Send 问题）
    let sid: MenuId = show_item.id().clone();
    let qid: MenuId = quit_item.id().clone();

    let _ = tray_menu.append_items(&[&show_item, &quit_item]);

    // 构建托盘图标，然后丢弃 MenuItem（menu 已经内部持有）
    drop(show_item);
    drop(quit_item);

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("AudioRouter")
        .with_icon(icon)
        .build();

    match tray_icon {
        Ok(_tray_icon) => {
            // 菜单事件
            let menu_tx = tx.clone();
            MenuEvent::set_event_handler(Some(move |event: tray_icon::menu::MenuEvent| {
                if *event.id() == sid {
                    let _ = menu_tx.send(TrayToAppCommand::ShowWindow);
                } else if *event.id() == qid {
                    let _ = menu_tx.send(TrayToAppCommand::Quit);
                }
            }));

            // 左键点击事件
            let click_tx = tx.clone();
            TrayIconEvent::set_event_handler(Some(move |event| {
                if let TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } = event
                {
                    let _ = click_tx.send(TrayToAppCommand::ShowWindow);
                }
            }));

            keep_tray_alive(_tray_icon, rx);
        }
        Err(e) => log::error!("Failed to create tray icon: {e}"),
    }
}

fn keep_tray_alive(_tray_icon: TrayIcon, rx: mpsc::Receiver<AppToTrayCommand>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            AppToTrayCommand::Quit => break,
            AppToTrayCommand::UpdateLanguage(_lang) => {
                // tray-icon 菜单文本无法稳定原地更新，下一次启动会应用语言。
            }
        }
    }
}

fn tray_text(locale: &str, key: &str) -> String {
    match (locale, key) {
        ("zh", "Show") => "显示".to_string(),
        ("zh", "Exit") => "退出".to_string(),
        (_, "Show") => "Show".to_string(),
        (_, "Exit") => "Quit".to_string(),
        _ => key.to_string(),
    }
}

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
