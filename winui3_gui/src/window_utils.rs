use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, IsWindowVisible, SetForegroundWindow, SetWindowLongPtrW,
    ShowWindow, CallWindowProcW, SW_HIDE, SW_SHOW, GWLP_WNDPROC, WM_CLOSE,
};

static CACHED_HWND: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(true);
static ORIGINAL_WNDPROC: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

unsafe extern "system" fn enum_callback(hwnd: HWND, _lparam: LPARAM) -> BOOL {
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

    let orig = ORIGINAL_WNDPROC.load(Ordering::SeqCst) as isize;
    if orig == 0 {
        return 0;
    }
    CallWindowProcW(Some(std::mem::transmute(orig)), hwnd, msg, wparam, lparam)
}

pub fn install_close_to_tray() {
    if let Some(hwnd) = find_hwnd() {
        let current = ORIGINAL_WNDPROC.load(Ordering::SeqCst);
        if !current.is_null() {
            return;
        }
        unsafe {
            let prev =
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass_wndproc as *const () as isize)
                    as *mut c_void;
            ORIGINAL_WNDPROC.store(prev, Ordering::SeqCst);
        }
    }
}

pub fn set_close_to_tray(enabled: bool) {
    CLOSE_TO_TRAY.store(enabled, Ordering::SeqCst);
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
