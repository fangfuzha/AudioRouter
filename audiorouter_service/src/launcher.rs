//! 启动 / 激活控制面板进程。
//!
//! 服务进程是常驻的，控制面板是随用随开的。托盘点击和全局热键都走这里：
//! 控制面板已经开着就激活它的窗口，否则拉起一个新的进程。

use std::process::{Command, Stdio};

use anyhow::{Result, anyhow};
use app_core::ipc::{GUI_EXE_NAME, sibling_exe};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOW, SetForegroundWindow,
    ShowWindowAsync,
};

/// 控制面板窗口标题（与 GUI 侧保持一致）。
const GUI_WINDOW_TITLE: &str = "AudioRouter";

/// 显示控制面板：已经在跑就激活它的窗口，否则拉起一个新进程。
pub fn show_control_panel() {
    if activate_existing() {
        return;
    }
    if let Err(e) = spawn_panel() {
        log::error!("启动控制面板失败: {e}");
    }
}

/// 激活已存在的控制面板窗口。
fn activate_existing() -> bool {
    let wide: Vec<u16> = GUI_WINDOW_TITLE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let hwnd = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
    if hwnd.is_null() {
        return false;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindowAsync(hwnd, SW_RESTORE);
        } else if IsWindowVisible(hwnd) == 0 {
            ShowWindowAsync(hwnd, SW_SHOW);
        }
        // 前台可能被系统锁住，激活失败也无所谓：窗口已经被显示出来了。
        let _ = SetForegroundWindow(hwnd);
    }
    true
}

/// 拉起控制面板进程。
fn spawn_panel() -> Result<()> {
    let exe = sibling_exe(GUI_EXE_NAME)
        .ok_or_else(|| anyhow!("找不到控制面板可执行文件（{GUI_EXE_NAME}）"))?;
    // `--show`：用户是从托盘图标点过来的，必须把窗口显示出来，
    // 不能沿用配置里的「启动时最小化」。
    Command::new(exe)
        .arg("--show")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
