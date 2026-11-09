use std::cell::RefCell;

use app_core::i18n::I18n;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

/// 托盘运行时状态，保存在 thread_local 中以便运行时更新菜单文本。
struct TrayState {
    show_item: MenuItem,
    quit_item: MenuItem,
    tray_icon: TrayIcon,
}

thread_local! {
    static TRAY_STATE: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

/// 托盘交互产生的命令。
#[derive(Clone, Copy, Debug)]
pub enum TrayCommand {
    ToggleWindow,
    ShowWindow,
    Quit,
}

/// 已注册菜单项的 id。
///
/// 菜单事件在后台线程接收，而菜单项本身保存在创建线程的 thread_local 中，
/// 因此把 id 复制出来交给后台线程做事件翻译。
#[derive(Clone)]
pub struct TrayMenuIds {
    show: MenuId,
    quit: MenuId,
}

/// 菜单项 id 类型（muda::MenuId）。
use tray_icon::menu::MenuId;

/// 初始化系统托盘图标，并在 thread_local 中保持存活。
///
/// `i18n` 用于翻译菜单项文本；左键点击不会弹出菜单（只在右键点击时弹出），
/// 左键点击的事件由 `command_from_tray_event` 翻译为 `ShowWindow`。
pub fn init_tray(i18n: I18n) -> anyhow::Result<()> {
    let icon = load_icon()?;

    let show_text = i18n.t("TrayShowHide").to_string();
    let quit_text = i18n.t("TrayQuit").to_string();
    let tooltip_text = i18n.t("AppTitle").to_string();

    let tray_menu = Menu::new();
    let show_item = MenuItem::new(&show_text, true, None);
    let quit_item = MenuItem::new(&quit_text, true, None);
    let separator = PredefinedMenuItem::separator();

    tray_menu.append(&show_item)?;
    tray_menu.append(&separator)?;
    tray_menu.append(&quit_item)?;

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_menu_on_left_click(false)
        .with_tooltip(&tooltip_text)
        .with_icon(icon)
        .build()?;

    TRAY_STATE.with(|s| {
        *s.borrow_mut() = Some(TrayState {
            show_item,
            quit_item,
            tray_icon,
        });
    });

    Ok(())
}

/// 运行时更新托盘菜单文本和 tooltip，用于语言切换后同步。
pub fn update_tray_language(i18n: &I18n) {
    TRAY_STATE.with(|s| {
        if let Some(state) = s.borrow().as_ref() {
            state.show_item.set_text(i18n.t("TrayShowHide"));
            state.quit_item.set_text(i18n.t("TrayQuit"));
            let _ = state.tray_icon.set_tooltip(Some(i18n.t("AppTitle")));
        }
    });
}

/// 读取已注册的菜单项 id。
///
/// 必须在创建托盘图标的那个线程上调用（菜单项保存在 thread_local 中）。
pub fn menu_item_ids() -> Option<TrayMenuIds> {
    TRAY_STATE.with(|s| {
        let borrow = s.borrow();
        let state = borrow.as_ref()?;
        Some(TrayMenuIds {
            show: state.show_item.id().clone(),
            quit: state.quit_item.id().clone(),
        })
    })
}

/// 托盘图标事件接收端（可跨线程使用）。
pub fn tray_event_receiver() -> tray_icon::TrayIconEventReceiver {
    TrayIconEvent::receiver().clone()
}

/// 托盘菜单事件接收端（可跨线程使用）。
pub fn menu_event_receiver() -> tray_icon::menu::MenuEventReceiver {
    MenuEvent::receiver().clone()
}

/// 把托盘图标事件翻译成命令。
///
/// 左键单击（抬起）= 显示并聚焦窗口；其余事件忽略。
pub fn command_from_tray_event(event: &TrayIconEvent) -> Option<TrayCommand> {
    if let TrayIconEvent::Click {
        button,
        button_state,
        ..
    } = event
    {
        if *button == tray_icon::MouseButton::Left
            && *button_state == tray_icon::MouseButtonState::Up
        {
            return Some(TrayCommand::ShowWindow);
        }
    }
    None
}

/// 把托盘菜单事件翻译成命令。
pub fn command_from_menu_event(event: &MenuEvent, ids: &TrayMenuIds) -> Option<TrayCommand> {
    if event.id == ids.show {
        Some(TrayCommand::ToggleWindow)
    } else if event.id == ids.quit {
        Some(TrayCommand::Quit)
    } else {
        None
    }
}

fn load_icon() -> anyhow::Result<Icon> {
    let icon_path = crate::resolve_asset_path("assets/icon.png");

    if icon_path.exists() {
        let image = image::open(&icon_path)?.into_rgba8();
        let (width, height) = image.dimensions();
        return Ok(Icon::from_rgba(image.into_raw(), width, height)?);
    }

    // 最后回退：生成一个纯色的默认图标
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            rgba[idx] = 0x00; // R
            rgba[idx + 1] = 0x78; // G
            rgba[idx + 2] = 0xD4; // B
            rgba[idx + 3] = 0xFF; // A
        }
    }
    Ok(Icon::from_rgba(rgba, size, size)?)
}
