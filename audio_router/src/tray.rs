//! 系统托盘模块

use std::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{TrayIconBuilder, TrayIconEvent};

/// 托盘 → 主应用 的命令
#[derive(Debug, Clone)]
pub enum TrayCommand {
    ShowWindow,
    Quit,
    UpdateLanguage(String),
}

/// 创建并运行系统托盘（在独立线程中运行）
pub fn run_tray(rx: mpsc::Receiver<TrayCommand>, tx: mpsc::Sender<TrayCommand>) {
    let icon = match load_tray_icon() {
        Some(icon) => icon,
        None => {
            log::error!("Failed to load tray icon");
            return;
        }
    };

    let tray_menu = Menu::new();
    let show_item = MenuItem::new("Show", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    // 提取 ID 为独立的所有权值（避免 MenuItem 的非 Send 问题）
    let sid: MenuId = show_item.id().clone();
    let qid: MenuId = quit_item.id().clone();

    let _ = tray_menu.append_items(&[&show_item, &quit_item]);

    // 构建托盘图标，然后丢弃 MenuItem（menu 已经内部持有）
    drop(show_item);
    drop(quit_item);

    if TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("AudioRouter")
        .with_icon(icon)
        .build()
        .is_ok()
    {
        // 菜单事件
        let menu_tx = tx.clone();
        MenuEvent::set_event_handler(Some(move |event: tray_icon::menu::MenuEvent| {
            if *event.id() == sid {
                let _ = menu_tx.send(TrayCommand::ShowWindow);
            } else if *event.id() == qid {
                let _ = menu_tx.send(TrayCommand::Quit);
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
                let _ = click_tx.send(TrayCommand::ShowWindow);
            }
        }));

        // 保持线程存活，接收命令
        while let Ok(cmd) = rx.recv() {
            if matches!(cmd, TrayCommand::Quit) {
                break;
            }
        }
    }
}

fn load_tray_icon() -> Option<tray_icon::Icon> {
    let icon_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("icon.png")));

    if let Some(ref path) = icon_path {
        if path.exists() {
            match image::open(path) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    return tray_icon::Icon::from_rgba(rgba.into_raw(), w, h).ok();
                }
                Err(e) => log::warn!("Failed to load tray icon: {e}"),
            }
        }
    }

    // 生成绿色圆点
    let size = 32u32;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let cx = x as f32 - size as f32 / 2.0;
            let cy = y as f32 - size as f32 / 2.0;
            if (cx * cx + cy * cy).sqrt() < size as f32 * 0.425 {
                pixels.extend_from_slice(&[43, 217, 127, 255]);
            } else {
                pixels.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    tray_icon::Icon::from_rgba(pixels, size, size).ok()
}
