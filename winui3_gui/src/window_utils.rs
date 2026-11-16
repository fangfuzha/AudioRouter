use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use app_core::window_placement::WindowRect;
use parking_lot::Mutex;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, RegisterHotKey, UnregisterHotKey, VK_CONTROL, VK_ESCAPE, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CallWindowProcW, EnumWindows, FlashWindowEx, GetForegroundWindow,
    GetSystemMetrics, GetWindowPlacement, GetWindowTextW, IsWindowVisible, PostMessageW,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, SetWindowsHookExW, ShowWindow,
    UnhookWindowsHookEx, FLASHWINFO, FLASHW_ALL, GWLP_WNDPROC, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_HIDE, SW_MAXIMIZE, SW_SHOW, WH_KEYBOARD_LL, WINDOWPLACEMENT, WM_CLOSE,
    WM_EXITSIZEMOVE, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// `SC_KEYMENU`：Alt（或 F10）触发的系统菜单命令。
const SC_KEYMENU: usize = 0xF100;

/// 全局热键 id（同一窗口只能注册有限个，用固定值即可）。
const HOTKEY_ID: i32 = 0xA17E;

static CACHED_HWND: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(true);
/// 主窗口的原始窗口过程（与键盘焦点窗口的分开存，见 `KEYBOARD_WNDPROC`）。
static MAIN_WNDPROC: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// 窗口位置是否已经恢复（或本来就没有需要恢复的值）。
static PLACEMENT_RESTORED: AtomicBool = AtomicBool::new(false);
/// 启动时应保持隐藏（「启动时最小化」或 `--minimized`）。
static START_HIDDEN: AtomicBool = AtomicBool::new(false);
/// 是否已经执行过「启动时隐藏」。
static START_HIDDEN_APPLIED: AtomicBool = AtomicBool::new(false);
/// 当前已注册的全局热键（0 表示没有注册）。
static REGISTERED_HOTKEY_VK: AtomicU32 = AtomicU32::new(0);
static REGISTERED_HOTKEY_MODS: AtomicU32 = AtomicU32::new(0);

/// 快捷键捕获的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureResult {
    /// 捕获成功：规范化的快捷键字符串，例如 Ctrl+Alt+M。
    Captured(String),
    /// 按下的组合里没有修饰键，Windows 不接受这种全局快捷键。
    NeedsModifier,
}

/// 是否处于「等待用户按下组合键」的捕获状态。
static HOTKEY_CAPTURE_ARMED: AtomicBool = AtomicBool::new(false);
/// 捕获结果：由窗口过程写入，UI 线程取走。
static CAPTURE_RESULT: Mutex<Option<CaptureResult>> = Mutex::new(None);
/// 捕获到快捷键后的回调（用来请求一次 UI 重绘）。
static CAPTURE_LISTENER: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);
/// 录制期间安装的低级键盘钩子句柄（0 表示没装）。
static CAPTURE_HOOK: AtomicUsize = AtomicUsize::new(0);
/// 录制期间自己累计的修饰键状态。
static CAPTURE_MODIFIERS: AtomicU32 = AtomicU32::new(0);
/// 键盘焦点所在窗口（WinUI 3 用一个隐藏窗口接收键盘输入）。
static KEYBOARD_WNDPROC: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static KEYBOARD_HWND: AtomicUsize = AtomicUsize::new(0);
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

    // 全局热键：打开主窗口（已经在前台时只聚焦，不隐藏）。
    if msg == WM_HOTKEY && wparam as i32 == HOTKEY_ID {
        show_and_focus_window();
        return 0;
    }

    // 快捷键捕获：录制期间拦截所有按键，翻译成 Ctrl+Alt+M 这样的字符串。
    if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
        if let Some(result) = handle_capture_key(wparam as u32) {
            return result;
        }
    }

    // 录制期间不要让 Alt 打开系统菜单：菜单一旦弹出，紧接着按下的主键
    // 会被菜单吃掉，组合键就再也收不到了。
    if msg == WM_SYSCOMMAND && (wparam & 0xFFF0) == SC_KEYMENU && hotkey_capture_active() {
        return 0;
    }

    // 移动/缩放拖拽结束后保存窗口位置，供下次启动恢复。
    if msg == WM_EXITSIZEMOVE {
        save_window_placement(hwnd);
    }

    // 主窗口和键盘焦点窗口各自有自己的原始窗口过程，不能共用一个槽位：
    // 后子类化的窗口会把先前的那个覆盖掉，消息就会被派发到错误的窗口过程。
    let orig = if hwnd as usize == KEYBOARD_HWND.load(Ordering::SeqCst) {
        KEYBOARD_WNDPROC.load(Ordering::SeqCst) as isize
    } else {
        MAIN_WNDPROC.load(Ordering::SeqCst) as isize
    };
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
/// 这里允许重复调用：装好之后立刻返回（只读几个原子变量），
/// 未装好时继续找窗口，直到找到为止。
pub fn install_window_hooks() {
    if !MAIN_WNDPROC.load(Ordering::SeqCst).is_null() {
        // 主窗口已经钩好，但键盘焦点可能落在另一个窗口上，
        // 仍然要继续检查。
        subclass_keyboard_focus_window();
        return;
    }

    let Some(hwnd) = find_hwnd() else {
        return;
    };

    unsafe {
        let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, subclass_wndproc as *const () as isize)
            as *mut c_void;
        MAIN_WNDPROC.store(prev, Ordering::SeqCst);
    }
    log::info!("Window hooks installed (close-to-tray + placement tracking)");
    subclass_keyboard_focus_window();
}

/// 把「当前键盘焦点所在窗口」也子类化。
///
/// WinUI 3 不会把键盘消息发到主窗口：XAML 内容挂在自己的隐藏窗口上，
/// 键盘焦点落在那个窗口，WM_KEYDOWN 也发给它。主窗口上的子类钩子
/// 因此收不到按键，设置页的快捷键录制必须钩住这个窗口。
///
/// GetFocus 只能在拥有键盘焦点的线程上调用，本函数运行在 UI 线程。
fn subclass_keyboard_focus_window() {
    let focus = unsafe { GetFocus() };
    if focus.is_null() {
        return;
    }
    let Some(hwnd) = find_hwnd() else {
        return;
    };
    if focus == hwnd {
        // 焦点还在主窗口上（主窗口已经钩过）。
        return;
    }
    let focus_addr = focus as usize;
    if KEYBOARD_HWND.load(Ordering::SeqCst) == focus_addr {
        return;
    }
    unsafe {
        let prev = SetWindowLongPtrW(focus, GWLP_WNDPROC, subclass_wndproc as *const () as isize)
            as *mut c_void;
        KEYBOARD_WNDPROC.store(prev, Ordering::SeqCst);
        KEYBOARD_HWND.store(focus_addr, Ordering::SeqCst);
    }
    log::info!("Keyboard focus window {focus_addr} subclassed for hotkey capture");
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

// === 快捷键捕获 ===

/// 开始捕获下一次按键组合（设置页的「录制」按钮）。
///
/// 捕获期间会装一个低级键盘钩子（WH_KEYBOARD_LL），在按键送到窗口之前
/// 就把它拦住，直到用户按下一个合法的组合键或者按 Esc 取消。
pub fn arm_hotkey_capture() {
    CAPTURE_RESULT.lock().take();
    CAPTURE_MODIFIERS.store(0, Ordering::SeqCst);
    HOTKEY_CAPTURE_ARMED.store(true, Ordering::SeqCst);
    install_capture_hook();
    // 兜底：窗口过程里也挂一道，钩子装不上时还能用。
    subclass_keyboard_focus_window();
}

/// 取消捕获（离开设置页时也要调用，避免把后续输入吞掉）。
pub fn cancel_hotkey_capture() {
    HOTKEY_CAPTURE_ARMED.store(false, Ordering::SeqCst);
    CAPTURE_MODIFIERS.store(0, Ordering::SeqCst);
    CAPTURE_RESULT.lock().take();
    uninstall_capture_hook();
}

/// 装上低级键盘钩子（录制期间用来拦截按键）。
fn install_capture_hook() {
    if CAPTURE_HOOK.load(Ordering::SeqCst) != 0 {
        return;
    }
    unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), std::ptr::null_mut(), 0);
        if hook.is_null() {
            log::warn!("SetWindowsHookExW(WH_KEYBOARD_LL) failed");
        } else {
            CAPTURE_HOOK.store(hook as usize, Ordering::SeqCst);
            log::info!("Low-level keyboard hook installed for hotkey capture");
        }
    }
}

/// 卸掉低级键盘钩子。
fn uninstall_capture_hook() {
    let hook = CAPTURE_HOOK.swap(0, Ordering::SeqCst);
    if hook != 0 {
        unsafe {
            UnhookWindowsHookEx(hook as HHOOK);
        }
    }
}

/// 低级键盘钩子：在按键被送到窗口之前拦住它。
///
/// 为什么不用窗口过程：WinUI 3 的键盘焦点在一个隐藏窗口上，而且第一次
/// 按 Alt 时系统会进入「菜单激活」状态，紧接着按下的主键会被系统菜单
/// 吃掉，组合键根本收不到。低级钩子发生在消息分发之前，每一个按键都
/// 拿得到，修饰键状态也由自己累计，不依赖 `GetKeyState`。
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let msg = wparam as u32;
    if code == HC_ACTION as i32
        && (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN || msg == WM_KEYUP || msg == WM_SYSKEYUP)
    {
        let ks = &*(lparam as *const KBDLLHOOKSTRUCT);
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        // 只有应用在前台时才拦截，避免影响其它程序。
        if is_app_foreground() {
            return handle_capture_event(ks.vkCode, is_down);
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// 处理一次录制期间的按键事件，返回 1 表示把这个按键吞掉。
fn handle_capture_event(vk: u32, is_down: bool) -> LRESULT {
    if !HOTKEY_CAPTURE_ARMED.load(Ordering::SeqCst) {
        return 0;
    }

    // 修饰键：自己累计状态。吞掉它，主键就不会再触发系统菜单。
    if let Some(bit) = app_core::hotkey::modifier_bit(vk) {
        let mut modifiers = CAPTURE_MODIFIERS.load(Ordering::SeqCst);
        if is_down {
            modifiers |= bit;
        } else {
            modifiers &= !bit;
        }
        CAPTURE_MODIFIERS.store(modifiers, Ordering::SeqCst);
        return 1;
    }

    // 主键抬起：忽略。
    if !is_down {
        return 1;
    }

    // Esc 取消录制。
    if vk as u16 == VK_ESCAPE {
        cancel_hotkey_capture();
        notify_capture_listener();
        return 1;
    }

    let modifiers = CAPTURE_MODIFIERS.load(Ordering::SeqCst);
    if modifiers == 0 {
        // 没有修饰键的组合注册不成全局快捷键（Windows 也不接受）。
        *CAPTURE_RESULT.lock() = Some(CaptureResult::NeedsModifier);
        notify_capture_listener();
        return 1;
    }

    // 不认识的按键（媒体键之类）：忽略，继续等待。
    if let Some(hotkey) = app_core::hotkey::describe(vk, modifiers) {
        *CAPTURE_RESULT.lock() = Some(CaptureResult::Captured(hotkey));
        HOTKEY_CAPTURE_ARMED.store(false, Ordering::SeqCst);
        CAPTURE_MODIFIERS.store(0, Ordering::SeqCst);
        uninstall_capture_hook();
        notify_capture_listener();
    }
    1
}

/// 应用窗口当前是否在前台。
fn is_app_foreground() -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        !foreground.is_null() && find_hwnd().is_some_and(|hwnd| hwnd == foreground)
    }
}

/// 当前是否正在等待用户按下组合键。
pub fn hotkey_capture_active() -> bool {
    HOTKEY_CAPTURE_ARMED.load(Ordering::SeqCst)
}

/// 取走最近一次捕获的结果（取走后清空）。
pub fn take_capture_result() -> Option<CaptureResult> {
    CAPTURE_RESULT.lock().take()
}

/// 注册捕获回调：捕获状态变化时调用，用来请求一次 UI 重绘。
pub fn set_capture_listener(listener: Arc<dyn Fn() + Send + Sync>) {
    *CAPTURE_LISTENER.lock() = Some(listener);
}

/// 窗口过程里的按键处理：返回 `Some(0)` 表示这个按键已被吞掉。
fn handle_capture_key(vk: u32) -> Option<LRESULT> {
    if !HOTKEY_CAPTURE_ARMED.load(Ordering::SeqCst) {
        return None;
    }

    // Esc 取消录制。
    if vk as u16 == VK_ESCAPE {
        cancel_hotkey_capture();
        notify_capture_listener();
        return Some(0);
    }

    // 只按了修饰键：继续等用户按下主键。
    //
    // 这里不能把修饰键本身吞掉：吞掉 Alt 之后系统会进入「菜单激活」状态，
    // 紧接着按下的主键（比如 Alt+M）会被系统菜单吃掉，组合键永远收不到。
    // 让修饰键照常走 DefWindowProc，主键仍然由这里拦截。
    if app_core::hotkey::is_modifier_key(vk) {
        return None;
    }

    let modifiers = current_modifiers();
    if modifiers == 0 {
        // 没有修饰键的组合注册不成全局快捷键（Windows 也不接受）。
        *CAPTURE_RESULT.lock() = Some(CaptureResult::NeedsModifier);
        notify_capture_listener();
        return Some(0);
    }

    // 不认识的按键（媒体键之类）返回 None：忽略，继续等待。
    if let Some(hotkey) = app_core::hotkey::describe(vk, modifiers) {
        *CAPTURE_RESULT.lock() = Some(CaptureResult::Captured(hotkey));
        HOTKEY_CAPTURE_ARMED.store(false, Ordering::SeqCst);
        notify_capture_listener();
    }
    Some(0)
}

/// 当前按下的修饰键掩码。
fn current_modifiers() -> u32 {
    let mut modifiers = 0;
    if key_state(VK_CONTROL) {
        modifiers |= app_core::hotkey::MOD_CONTROL;
    }
    if key_state(VK_MENU) {
        modifiers |= app_core::hotkey::MOD_ALT;
    }
    if key_state(VK_SHIFT) {
        modifiers |= app_core::hotkey::MOD_SHIFT;
    }
    if key_state(VK_LWIN) || key_state(VK_RWIN) {
        modifiers |= app_core::hotkey::MOD_WIN;
    }
    modifiers
}

/// 某个键当前是否处于「按下」状态。
///
/// GetKeyState 返回 i16，最高位（0x8000）表示按键当前处于按下状态。
fn key_state(vk: u16) -> bool {
    unsafe { (GetKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

/// 通知 UI：捕获状态有变化，需要重绘。
fn notify_capture_listener() {
    let listener = CAPTURE_LISTENER.lock().clone();
    if let Some(listener) = listener {
        listener();
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

/// 窗口隐藏（最小化到托盘）时闪烁任务栏图标，提醒用户有事发生。
///
/// 隐藏状态下界面上的提示条根本看不到，闪烁是唯一可靠的视觉提示。
/// 窗口可见时不闪烁，避免干扰正在使用应用的用户。
pub fn flash_window_if_hidden() {
    let Some(hwnd) = find_hwnd() else {
        return;
    };
    unsafe {
        if IsWindowVisible(hwnd) != 0 {
            return;
        }
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            dwFlags: FLASHW_ALL,
            // 闪烁若干次后自动停止，避免一直闪下去。
            uCount: 6,
            dwTimeout: 0,
        };
        FlashWindowEx(&info);
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
