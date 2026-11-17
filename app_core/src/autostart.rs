//! 开机自启管理（Windows 注册表方式）

use anyhow::{Result, anyhow};

/// 设置或取消开机自启
pub fn set_autostart(enabled: bool) -> Result<()> {
    set_autostart_inner(enabled)
}

fn set_autostart_inner(enabled: bool) -> Result<()> {
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Registry::*;

    let hkey = HKEY_CURRENT_USER;
    let mut handle = HKEY::default();

    let rc = unsafe {
        RegOpenKeyExW(
            hkey,
            windows::core::w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run"),
            Some(0),
            KEY_SET_VALUE,
            &mut handle,
        )
    };
    if rc != ERROR_SUCCESS {
        let rc = unsafe {
            RegCreateKeyW(
                hkey,
                windows::core::w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run"),
                &mut handle,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(anyhow!(
                "Failed to open/create registry key: error {}",
                rc.0
            ));
        }
    }

    if enabled {
        let (exe, minimized) = autostart_target();
        let path_str = autostart_command(&exe, minimized);

        let wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes =
            unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };

        let rc = unsafe {
            RegSetValueExW(
                handle,
                windows::core::w!("AudioRouter"),
                Some(0),
                REG_SZ,
                Some(bytes),
            )
        };
        if rc != ERROR_SUCCESS {
            let _ = unsafe { RegCloseKey(handle) };
            return Err(anyhow!("RegSetValueExW failed: error {}", rc.0));
        }
    } else {
        unsafe {
            let _ = RegDeleteValueW(handle, windows::core::w!("AudioRouter"));
        }
    }

    let _ = unsafe { RegCloseKey(handle) };
    Ok(())
}

/// 开机自启要拉起的程序。
///
/// 架构拆成两个进程之后，开机只需要常驻服务（路由 + 托盘 + 热键），
/// 控制面板是随用随开的。因此优先指向同目录下的 `audiorouter_service.exe`；
/// 找不到它时（例如开发环境里只编了 GUI）才退回当前进程。
///
/// 第二个返回值表示是否要附加 `--minimized`（只有退回 GUI 时才需要）。
fn autostart_target() -> (std::path::PathBuf, bool) {
    if let Some(service) = crate::ipc::sibling_exe(crate::ipc::SERVICE_EXE_NAME) {
        return (service, false);
    }
    (std::env::current_exe().unwrap_or_default(), true)
}

/// 生成开机自启的注册表命令行。
fn autostart_command(exe_path: &std::path::Path, minimized: bool) -> String {
    if minimized {
        format!("\"{}\" --minimized", exe_path.display())
    } else {
        format!("\"{}\"", exe_path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_autostart_path() {
        let cmd = autostart_command(
            std::path::Path::new(r"C:\Program Files\AudioRouter\winui3_gui.exe"),
            true,
        );
        assert_eq!(
            cmd,
            r#""C:\Program Files\AudioRouter\winui3_gui.exe" --minimized"#
        );
    }

    #[test]
    fn service_target_has_no_minimized_flag() {
        let cmd = autostart_command(
            std::path::Path::new(r"C:\Program Files\AudioRouter\audiorouter_service.exe"),
            false,
        );
        assert_eq!(
            cmd,
            r#""C:\Program Files\AudioRouter\audiorouter_service.exe""#
        );
    }
}
