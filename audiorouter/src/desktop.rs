//! 桌面集成：托盘图标 + 全局热键（主线程）。
//!
//! 主线程是整个应用的「主」：托盘图标、全局热键、Win32 消息循环都在这里。
//! 控制面板（WinUI 3）跑在另一条 STA 线程上，音频路由跑在服务线程上，
//! 三者互不阻塞——托盘永远不会因为路由启动要等 5 秒而卡住。
//!
//! 注意：托盘菜单项（muda）只能在创建它的那个线程上操作，所以托盘状态的
//! 更新统一走「服务线程写共享数据 → PostMessage 唤醒窗口过程 → 窗口过程
//! 所在线程真正更新托盘」这条路径。

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use anyhow::Result;
use app_core::bus::{Command, TrayCommand};
use app_core::hotkey;
use app_core::i18n::I18n;
use app_core::paths::resolve_asset_path;
use parking_lot::Mutex;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, RegisterClassW,
    TranslateMessage, WM_APP, WM_HOTKEY, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

/// 自定义消息：托盘状态需要更新。
const WM_TRAY_UPDATE: u32 = WM_APP + 1;
/// 自定义消息：把控制面板叫起来（第二个实例启动时由它投递）。
pub const WM_TRAY_SHOW_PANEL: u32 = WM_APP + 2;
/// 承载托盘与热键的隐藏窗口类名。单实例保证全系统只有一个，
/// 第二个实例靠它找到常驻实例并把控制面板叫起来。
pub const TRAY_WINDOW_CLASS: &str = "AudioRouterServiceWindow";
/// 全局热键 id。
const HOTKEY_ID: i32 = 0xA17E;

/// 主循环 → 服务线程的命令通道。
static COMMAND_TX: OnceLock<Sender<Command>> = OnceLock::new();
/// 隐藏窗口句柄（存成 usize 便于放进原子变量）。
static TRAY_HWND: AtomicUsize = AtomicUsize::new(0);
/// 待应用的托盘状态。
static PENDING_TRAY: Mutex<Option<(I18n, bool, usize)>> = Mutex::new(None);
/// 待应用的全局热键。
static PENDING_HOTKEY: Mutex<Option<String>> = Mutex::new(None);

/// 托盘运行时状态。muda 的菜单项只能在创建线程上操作，因此放在
/// thread_local 里，由窗口过程所在的线程访问。
struct TrayState {
    status_item: MenuItem,
    start_item: MenuItem,
    stop_item: MenuItem,
    tray_icon: TrayIcon,
    running: bool,
    output_count: usize,
}

thread_local! {
    static TRAY_STATE: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

/// 已注册菜单项的 id。菜单事件在后台线程接收，菜单项本身在创建线程上，
/// 因此把 id 复制出来交给后台线程做事件翻译。
#[derive(Clone)]
pub struct TrayMenuIds {
    show: MenuId,
    start: MenuId,
    stop: MenuId,
    quit: MenuId,
}

/// 生成托盘状态行文本。
fn status_text(i18n: &I18n, running: bool, output_count: usize) -> String {
    if running {
        i18n.t("TrayStatusRunning")
            .replace("{count}", &output_count.to_string())
    } else {
        i18n.t("TrayStatusStopped").to_string()
    }
}

/// 托盘 tooltip：应用名 + 当前状态。
fn tooltip_text(i18n: &I18n, running: bool, output_count: usize) -> String {
    format!(
        "{} · {}",
        i18n.t("AppTitle"),
        status_text(i18n, running, output_count)
    )
}

/// 初始化系统托盘图标。
fn init_tray(i18n: &I18n) -> Result<TrayMenuIds> {
    let icon = load_icon()?;

    let tray_menu = Menu::new();
    let status_item = MenuItem::new(status_text(i18n, false, 0), false, None);
    let show_item = MenuItem::new(i18n.t("TrayShowHide"), true, None);
    let start_item = MenuItem::new(i18n.t("TrayStart"), true, None);
    let stop_item = MenuItem::new(i18n.t("TrayStop"), true, None);
    let quit_item = MenuItem::new(i18n.t("TrayQuit"), true, None);
    let separator = PredefinedMenuItem::separator();

    tray_menu.append(&status_item)?;
    tray_menu.append(&show_item)?;
    tray_menu.append(&start_item)?;
    tray_menu.append(&stop_item)?;
    tray_menu.append(&separator)?;
    tray_menu.append(&quit_item)?;

    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_menu_on_left_click(false)
        .with_tooltip(tooltip_text(i18n, false, 0))
        .with_icon(icon)
        .build()?;

    let ids = TrayMenuIds {
        show: show_item.id().clone(),
        start: start_item.id().clone(),
        stop: stop_item.id().clone(),
        quit: quit_item.id().clone(),
    };

    TRAY_STATE.with(|s| {
        *s.borrow_mut() = Some(TrayState {
            status_item,
            start_item,
            stop_item,
            tray_icon,
            running: false,
            output_count: 0,
        });
    });

    Ok(ids)
}

/// 同步托盘里的运行状态。
fn apply_tray_state(i18n: &I18n, running: bool, output_count: usize) {
    TRAY_STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            state.running = running;
            state.output_count = output_count;
            state.start_item.set_enabled(!running);
            state.stop_item.set_enabled(running);
            let status = status_text(i18n, state.running, state.output_count);
            state.status_item.set_text(&status);
            let _ = state.tray_icon.set_tooltip(Some(&tooltip_text(
                i18n,
                state.running,
                state.output_count,
            )));
        }
    });
}

/// 服务线程调用：请求更新托盘状态。
pub fn request_tray_update(i18n: &I18n, running: bool, output_count: usize) {
    *PENDING_TRAY.lock() = Some((i18n.clone(), running, output_count));
    wake();
}

/// 服务线程调用：请求重新注册全局热键。
pub fn request_set_hotkey(value: &str) {
    *PENDING_HOTKEY.lock() = Some(value.to_string());
    wake();
}

/// 唤醒桌面线程（让它处理待办）。
fn wake() {
    let hwnd = TRAY_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            PostMessageW(hwnd as HWND, WM_TRAY_UPDATE, 0, 0);
        }
    }
}

/// 窗口过程：处理热键与托盘更新。
unsafe extern "system" fn service_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            if let Some(tx) = COMMAND_TX.get() {
                let _ = tx.send(Command::HotkeyPressed);
            }
            0
        }
        WM_TRAY_UPDATE => {
            if let Some((i18n, running, count)) = PENDING_TRAY.lock().take() {
                apply_tray_state(&i18n, running, count);
            }
            if let Some(value) = PENDING_HOTKEY.lock().take() {
                let err = register_hotkey(hwnd, &value);
                if let Some(tx) = COMMAND_TX.get() {
                    let _ = tx.send(Command::HotkeyResult(err));
                }
            }
            0
        }
        // 第二个实例启动时投递：让常驻实例把控制面板显示出来。
        WM_TRAY_SHOW_PANEL => {
            crate::window_utils::show_window();
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 注册全局热键（会先注销旧的）。返回失败原因，None 表示成功。
fn register_hotkey(hwnd: HWND, value: &str) -> Option<String> {
    unsafe {
        let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
    }
    if value.trim().is_empty() {
        return None;
    }
    match hotkey::parse(value) {
        Ok(parsed) => {
            let ok = unsafe { RegisterHotKey(hwnd, HOTKEY_ID, parsed.modifiers, parsed.vk) };
            if ok == 0 {
                log::warn!("全局热键注册失败（可能已被其他程序占用）: {value}");
                Some(format!("全局热键已被占用: {value}"))
            } else {
                log::info!("全局热键已注册: {value}");
                None
            }
        }
        Err(e) => {
            log::warn!("全局热键解析失败: {e}");
            Some(e.to_string())
        }
    }
}

/// 创建承载热键的隐藏窗口。
fn create_hidden_window() -> Result<HWND> {
    unsafe {
        let class_name: Vec<u16> = TRAY_WINDOW_CLASS
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(service_wndproc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        // 重复注册会失败（类已存在），这里忽略返回值。
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            return Err(anyhow::anyhow!("创建隐藏窗口失败"));
        }
        Ok(hwnd)
    }
}

/// 把托盘图标事件翻译成命令。
///
/// 左键单击图标 = **显示**窗口（已经显示时只聚焦，不隐藏）；菜单里的
/// 「显示 / 隐藏」才是切换。
fn command_from_tray_event(event: &TrayIconEvent) -> Option<TrayCommand> {
    if let TrayIconEvent::Click {
        button,
        button_state,
        ..
    } = event
    {
        if *button == tray_icon::MouseButton::Left
            && *button_state == tray_icon::MouseButtonState::Up
        {
            return Some(TrayCommand::ShowPanel);
        }
    }
    None
}

/// 把托盘菜单事件翻译成命令。
fn command_from_menu_event(event: &MenuEvent, ids: &TrayMenuIds) -> Option<TrayCommand> {
    if event.id == ids.show {
        Some(TrayCommand::TogglePanel)
    } else if event.id == ids.start {
        Some(TrayCommand::StartRouting)
    } else if event.id == ids.stop {
        Some(TrayCommand::StopRouting)
    } else if event.id == ids.quit {
        Some(TrayCommand::Quit)
    } else {
        None
    }
}

/// 主线程入口：创建窗口、托盘与热键，然后进入消息循环。
///
/// `cmd_tx` 是通往服务线程的命令通道：托盘菜单与全局热键都经它转交，
/// 这样路由状态的修改全部发生在服务线程上。
pub fn run(cmd_tx: Sender<Command>, hotkey_value: &str, i18n: &I18n) -> Result<()> {
    let _ = COMMAND_TX.set(cmd_tx.clone());

    let hwnd = create_hidden_window()?;
    TRAY_HWND.store(hwnd as usize, Ordering::SeqCst);

    // 注册结果回传给服务线程：被别的程序占用时要让控制面板显示出来。
    let hotkey_error = register_hotkey(hwnd, hotkey_value);
    let _ = cmd_tx.send(Command::HotkeyResult(hotkey_error));

    let ids = init_tray(i18n)?;

    // 托盘事件在后台线程接收，翻译成命令交给服务线程。
    {
        let tx = cmd_tx.clone();
        std::thread::spawn(move || {
            let rx = TrayIconEvent::receiver();
            while let Ok(event) = rx.recv() {
                if let Some(cmd) = command_from_tray_event(&event) {
                    let _ = tx.send(Command::Tray(cmd));
                }
            }
        });
    }
    {
        let tx = cmd_tx.clone();
        std::thread::spawn(move || {
            let rx = MenuEvent::receiver();
            while let Ok(event) = rx.recv() {
                if let Some(cmd) = command_from_menu_event(&event, &ids) {
                    let _ = tx.send(Command::Tray(cmd));
                }
            }
        });
    }

    log::info!("托盘主线程启动");

    // 消息循环：托盘图标与热键窗口的消息都在这里派发。
    loop {
        let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG =
            unsafe { std::mem::zeroed() };
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// 载入托盘图标。
fn load_icon() -> Result<Icon> {
    let icon_path = resolve_asset_path("assets/icon.png");

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
