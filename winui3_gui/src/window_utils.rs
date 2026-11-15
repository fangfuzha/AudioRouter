use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use app_core::window_placement::WindowRect;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, EnumWindows, GetSystemMetrics, GetWindowPlacement, GetWindowTextW,
    IsWindowVisible, PostMessageW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, GWLP_WNDPROC, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_MAXIMIZE, SW_SHOW,
    WINDOWPLACEMENT, WM_CLOSE, WM_EXITSIZEMOVE, WM_HOTKEY,
};

/// 全局热键 id（同一窗口只能注册有限个，用固定值即可）。
const HOTKEY_ID: i32 = 0xA17E;

static CACHED_HWND: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(true);
static ORIGINAL_WNDPROC: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// 窗口位置是否已经恢复（或本来就没有需要恢复的值）。
static PLACEMENT_RESTORED: AtomicBool = AtomicBool::new(false);
/// 启动时应保持隐藏（「启动时最小化」或 `--minimized`）。
static START_HIDDEN: AtomicBool = AtomicBool::new(false);
/// 是否已经执行过「启动时隐藏」。
static START_HIDDEN_APPLIED: AtomicBool = AtomicBool::new(false);
/// 当前已注册的全局热键（0 表示没有注册）。
static REGISTERED_HOTKEY_VK: AtomicU32 = AtomicU32::new(0);
static REGISTERED_HOTKEY_MODS: AtomicU32 = AtomicU32::new(0);
unsafe extern "system" fn enum_callback(hwnd: HWND, _lparam: LPARAM) -> i32 {
    let mut buf = [0u16; 256];
    let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), 256);
    if len > 0 {
        let text = String::from_utf16_lossy(&buf[..len as usize]);
        if text == "AudioRouter" {
            CACHED_HWND.store(hwnd, Ordering::SeqCst);
            return 0;
        }
    }
    1
}

fn find_hwnd() -> Option<HWND> {
    let cached = CACHED_HWND.load(Ordering::SeqCst);
    if !cached.is_null() {
        unsafe {
            if IsWindowVisible(cached) != 0 {
                return Some(cached);
            }
        }
    }

    CACHED_HWND.store(std::ptr::null_mut(), Ordering::SeqCst);
    unsafe {
        EnumWindows(Some(enum_callback), 0);
    }
    let found = CACHED_HWND.load(Ordering::SeqCst);
    if !found.is_null() {
        Some(found)
    } else {
        None
    }
}

unsafe extern "system" fn subclass_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLOSE && CLOSE_TO_TRAY.load(Ordering::SeqCst) {
        ShowWindow(hwnd, SW_HIDE);
        return 0;
    }

    // 全局热键：切换窗口显隐。
    if msg == WM_HOTKEY && wparam as i32 == HOTKEY_ID {
        toggle_window();
        return 0;
    }

    // 移动/缩放拖拽结束后保存窗口位置，供下次启动恢复。
    if msg == WM_EXITSIZEMOVE {
        save_window_placement(hwnd);
    }

    let orig = ORIGINAL_WNDPROC.load(Ordering::SeqCst) as isize;
    if orig == 0 {
        return 0;
    }
    CallWindowProcW(
        Some(unsafe {
            std::mem::transmute::<
                isize,
                unsafe extern "system" fn(*mut std::ffi::c_void, u32, usize, isize) -> isize,
            >(orig)
        }),
        hwnd,
        msg,
        wparam,
        lparam,
    )
}

/// 安装窗口子类钩子：处理「关闭到托盘」和窗口位置记忆。
///
/// WinUI 的窗口标题在首帧渲染之后才设置好，因此首帧往往还找不到窗口。
/// 这里允许重复调用：装好之后立刻返回（只读两个原子变量），
/// 未装好时继续找窗口，直到找到为止。
pub fn install_window_hooks() {
    if !ORIGINAL_WNDPROC.load(Ordering::SeqCst).is_null() {
        return;
    }

    let Some(hwnd) = find_hwnd() else {
        return;
    };

    unsafe {
        let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass_wndproc as *const () as isize)
            as *mut c_void;
        ORIGINAL_WNDPROC.store(prev, Ordering::SeqCst);
    }
    log::info!("Window hooks installed (close-to-tray + placement tracking)");
}

pub fn set_close_to_tray(enabled: bool) {
    CLOSE_TO_TRAY.store(enabled, Ordering::SeqCst);
}

pub fn set_start_hidden(enabled: bool) {
    if enabled {
        START_HIDDEN.store(true, Ordering::SeqCst);
    }
}

pub fn apply_start_hidden() {
    if !START_HIDDEN.load(Ordering::SeqCst) || START_HIDDEN_APPLIED.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(hwnd) = find_hwnd() {
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

pub fn configure_global_hotkey(value: &str) -> Result<(), String> {
    let hwnd = find_hwnd().ok_or_else(|| "window handle not found".to_string())?;
    let parsed = if value.trim().is_empty() {
        None
    } else {
        Some(app_core::hotkey::parse(value).map_err(|e| e.to_string())?)
    };
    let old_vk = REGISTERED_HOTKEY_VK.swap(0, Ordering::SeqCst);
    let old_mods = REGISTERED_HOTKEY_MODS.swap(0, Ordering::SeqCst);
    if old_vk != 0 {
        unsafe {
            UnregisterHotKey(hwnd, HOTKEY_ID);
        }
    }
    let Some(hotkey) = parsed else {
        return Ok(());
    };
    let ok = unsafe { RegisterHotKey(hwnd, HOTKEY_ID, hotkey.modifiers, hotkey.vk) };
    if ok == 0 {
        if old_vk != 0 {
            let _ = unsafe { RegisterHotKey(hwnd, HOTKEY_ID, old_mods, old_vk) };
            REGISTERED_HOTKEY_VK.store(old_vk, Ordering::SeqCst);
            REGISTERED_HOTKEY_MODS.store(old_mods, Ordering::SeqCst);
        }
        return Err("hotkey is already in use".to_string());
    }
    REGISTERED_HOTKEY_MODS.store(hotkey.modifiers, Ordering::SeqCst);
    REGISTERED_HOTKEY_VK.store(hotkey.vk, Ordering::SeqCst);
    Ok(())
}

pub fn unregister_global_hotkey() {
    if REGISTERED_HOTKEY_VK.swap(0, Ordering::SeqCst) != 0 {
        if let Some(hwnd) = find_hwnd() {
            unsafe {
                UnregisterHotKey(hwnd, HOTKEY_ID);
            }
        }
        REGISTERED_HOTKEY_MODS.store(0, Ordering::SeqCst);
    }
}

/// 保存窗口当前的“正常显示”矩形到注册表。
///
/// 窗口最大化时保存的是还原后的尺寸，避免下次启动直接以最大化尺寸打开。
fn save_window_placement(hwnd: HWND) {
    let Some(rect) = current_normal_rect(hwnd) else {
        return;
    };
    if let Err(e) = app_core::window_placement::save_window_placement(rect) {
        log::warn!("Failed to save window placement: {e}");
    }
}

/// 读取窗口当前的“正常显示”矩形。
fn current_normal_rect(hwnd: HWND) -> Option<WindowRect> {
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
        return None;
    }

    let rc = placement.rcNormalPosition;
    let rect = WindowRect {
        left: rc.left,
        top: rc.top,
        width: rc.right - rc.left,
        height: rc.bottom - rc.top,
        maximized: placement.showCmd == SW_MAXIMIZE as u32,
    };
    rect.is_valid().then_some(rect)
}

/// 恢复上次保存的窗口位置与大小。
///
/// 窗口由框架按默认尺寸创建，这里在窗口就绪后把它移动到上次的位置。
/// 如果保存的位置已经落在虚拟屏幕之外（显示器被拔掉、分辨率变更），
/// 则放弃恢复，避免窗口“消失”在屏幕外。
///
/// 返回 `true` 表示恢复流程已结束（成功恢复或本来就没有保存值），
/// 返回 `false` 表示窗口还没就绪，调用方可以稍后重试。
pub fn restore_window_placement() -> bool {
    if PLACEMENT_RESTORED.load(Ordering::SeqCst) {
        return true;
    }

    let Some(rect) = app_core::window_placement::load_window_placement() else {
        // 没有保存过位置，无需恢复。
        PLACEMENT_RESTORED.store(true, Ordering::SeqCst);
        return true;
    };
    let Some(hwnd) = find_hwnd() else {
        // 窗口还没创建好，下次再试。
        return false;
    };
    let Some((x, y)) = clamp_to_virtual_screen(rect) else {
        log::warn!("Saved window placement {rect:?} is outside the virtual screen, ignoring");
        PLACEMENT_RESTORED.store(true, Ordering::SeqCst);
        return true;
    };

    let ok = unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            rect.width,
            rect.height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    if ok == 0 {
        log::warn!("SetWindowPos failed while restoring window placement");
        PLACEMENT_RESTORED.store(true, Ordering::SeqCst);
        if rect.maximized {
            unsafe {
                ShowWindow(hwnd, SW_MAXIMIZE);
            }
        }
        return true;
    }
    PLACEMENT_RESTORED.store(true, Ordering::SeqCst);
    log::info!(
        "Restored window placement: {x},{y} {}x{}",
        rect.width,
        rect.height
    );
    true
}

/// 把保存的矩形夹到虚拟屏幕范围内。
///
/// 返回 `None` 表示保存的位置已经完全不可见（应当放弃恢复）。
fn clamp_to_virtual_screen(rect: WindowRect) -> Option<(i32, i32)> {
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if vw <= 0 || vh <= 0 {
        return Some((rect.left, rect.top));
    }

    let intersects = rect.left < vx + vw
        && rect.left + rect.width > vx
        && rect.top < vy + vh
        && rect.top + rect.height > vy;
    if !intersects {
        return None;
    }

    let max_x = (vx + vw - rect.width).max(vx);
    let max_y = (vy + vh - rect.height).max(vy);
    Some((rect.left.clamp(vx, max_x), rect.top.clamp(vy, max_y)))
}

#[allow(dead_code)]
pub fn show_window() {
    if let Some(hwnd) = find_hwnd() {
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
        }
    }
}

#[allow(dead_code)]
pub fn hide_window() {
    if let Some(hwnd) = find_hwnd() {
        unsafe {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

pub fn show_and_focus_window() {
    if let Some(hwnd) = find_hwnd() {
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// 关闭窗口，语义与点击标题栏关闭按钮完全一致。
///
/// 这里投递 `WM_CLOSE` 而不是直接 `ShowWindow(SW_HIDE)` / `DestroyWindow`，
/// 是为了复用子类化钩子里已有的处理：开启「关闭到托盘」时隐藏窗口，
/// 关闭时真正退出应用。
pub fn close_window() {
    let Some(hwnd) = find_hwnd() else {
        log::warn!("Ctrl+W: window handle not found, cannot close");
        return;
    };
    unsafe {
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }
}

pub fn toggle_window() {
    if let Some(hwnd) = find_hwnd() {
        unsafe {
            if IsWindowVisible(hwnd) != 0 {
                ShowWindow(hwnd, SW_HIDE);
            } else {
                ShowWindow(hwnd, SW_SHOW);
                // 显示后聚焦窗口，使键盘输入和焦点正确。
                // SetForegroundWindow 在某些情况下会因系统前台锁定失败，
                // 但在 UI 线程定时器中调用通常可生效。
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }
}
