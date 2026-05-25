//! 开机自启管理（Windows 注册表方式）

use anyhow::{Result, anyhow};

/// 设置或取消开机自启
pub fn set_autostart(enabled: bool) {
    if let Err(e) = set_autostart_inner(enabled) {
        log::error!("Failed to set autostart: {e}");
    }
}

fn set_autostart_inner(enabled: bool) -> Result<()> {
    use windows::Win32::System::Registry::*;
    use windows::Win32::Foundation::*;

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
            return Err(anyhow!("Failed to open/create registry key: error {}", rc.0));
        }
    }

    if enabled {
        let exe_path = std::env::current_exe()?;
        let mut path_str = exe_path.to_string_lossy().to_string();
        path_str.push_str(" --minimized");

        let wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = unsafe {
            std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2)
        };

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
        unsafe { let _ = RegDeleteValueW(handle, windows::core::w!("AudioRouter")); }
    }

    let _ = unsafe { RegCloseKey(handle) };
    Ok(())
}
