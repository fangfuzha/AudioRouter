use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use app_core::window_placement::WindowRect;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, EnumWindows, GetSystemMetrics, GetWindowPlacement, GetWindowTextW,
    IsWindowVisible, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWLP_WNDPROC, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW, WINDOWPLACEMENT, WM_CLOSE, WM_EXITSIZEMOVE,
};

static CACHED_HWND: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(true);
static ORIGINAL_WNDPROC: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

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
pub fn install_window_hooks() {
    if let Some(hwnd) = find_hwnd() {
        let current = ORIGINAL_WNDPROC.load(Ordering::SeqCst);
        if !current.is_null() {
            return;
        }
        unsafe {
            let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass_wndproc as *const () as isize)
                as *mut c_void;
            ORIGINAL_WNDPROC.store(prev, Ordering::SeqCst);
        }
    }
}

pub fn set_close_to_tray(enabled: bool) {
    CLOSE_TO_TRAY.store(enabled, Ordering::SeqCst);
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
    };
    rect.is_valid().then_some(rect)
}

/// 恢复上次保存的窗口位置与大小。
///
/// 窗口由框架按默认尺寸创建，这里在首次渲染后把它移动到上次的位置。
/// 如果保存的位置已经落在虚拟屏幕之外（显示器被拔掉、分辨率变更），
/// 则放弃恢复，避免窗口“消失”在屏幕外。
pub fn restore_window_placement() {
    let Some(rect) = app_core::window_placement::load_window_placement() else {
        return;
    };
    let Some(hwnd) = find_hwnd() else {
        log::warn!("Window handle not found, skip restoring window placement");
        return;
    };
    let Some((x, y)) = clamp_to_virtual_screen(rect) else {
        log::warn!("Saved window placement {rect:?} is outside the virtual screen, ignoring");
        return;
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
        return;
    }
    log::info!(
        "Restored window placement: {x},{y} {}x{}",
        rect.width,
        rect.height
    );
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
