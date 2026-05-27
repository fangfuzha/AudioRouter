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
            0,
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
        let path_str = autostart_command(std::env::current_exe()?);

        let wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes =
            unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };

        let rc = unsafe {
            RegSetValueExW(
                handle,
                windows::core::w!("AudioRouter"),
                0,
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

fn autostart_command(exe_path: std::path::PathBuf) -> String {
    format!("\"{}\" --minimized", exe_path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_autostart_path() {
        let cmd = autostart_command(std::path::PathBuf::from(
            r"C:\Program Files\AudioRouter\audio_router.exe",
        ));
        assert_eq!(
            cmd,
            r#""C:\Program Files\AudioRouter\audio_router.exe" --minimized"#
        );
    }
}
