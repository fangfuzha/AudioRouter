//! 窗口位置记忆：把窗口矩形持久化到 Windows 注册表。
//!
//! 存储位置：`HKCU\Software\AudioRouter\WindowPlacement`（REG_SZ）。
//! 取值格式：`left,top,width,height`（屏幕物理像素）。
//!
//! 只保存“正常显示”状态下的矩形：窗口最大化时保存的是还原后的尺寸，
//! 避免下次启动直接以最大化尺寸打开。
//!
//! 该模块不依赖 GUI 框架，WinUI 侧负责读取窗口矩形并调用保存/加载。

use anyhow::{Result, anyhow};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, REG_VALUE_TYPE, RegCloseKey,
    RegCreateKeyW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

/// 注册表子键路径。
const SUB_KEY: &str = "SOFTWARE\\AudioRouter";
/// 注册表值名。
const VALUE_NAME: &str = "WindowPlacement";

/// 窗口矩形（屏幕物理像素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl WindowRect {
    /// 是否是一个尺寸合理的矩形。
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// 读取上次保存的窗口矩形。
///
/// 读取失败或数据损坏时返回 `None`，调用方应回退到默认窗口位置。
pub fn load_window_placement() -> Option<WindowRect> {
    let handle = open_key(KEY_READ).ok()?;

    let value_name = wide(VALUE_NAME);
    let mut buf = [0u8; 64];
    let mut len: u32 = buf.len() as u32;
    let mut value_type: REG_VALUE_TYPE = REG_VALUE_TYPE(0);

    let status = unsafe {
        RegQueryValueExW(
            handle,
            windows::core::PCWSTR(value_name.as_ptr()),
            None,
            Some(&mut value_type as *mut REG_VALUE_TYPE),
            Some(buf.as_mut_ptr()),
            Some(&mut len),
        )
    };
    unsafe {
        let _ = RegCloseKey(handle);
    }

    if status != ERROR_SUCCESS || value_type != REG_SZ {
        return None;
    }

    // REG_SZ 以 UTF-16LE 存储，len 是字节数。
    let text = String::from_utf16_lossy(
        &buf[..len as usize]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect::<Vec<u16>>(),
    );
    let text = text.trim_end_matches('\0').trim();

    parse_rect(text)
}

/// 保存窗口矩形。
///
/// 写入失败只返回错误，不影响 GUI 主流程。
pub fn save_window_placement(rect: WindowRect) -> Result<()> {
    if !rect.is_valid() {
        return Err(anyhow!("invalid window rect: {rect:?}"));
    }

    let handle = open_key(KEY_READ | KEY_WRITE)?;

    let value = format!("{},{},{},{}", rect.left, rect.top, rect.width, rect.height);
    let wide_value = wide(&value);
    let bytes = unsafe {
        std::slice::from_raw_parts(wide_value.as_ptr() as *const u8, wide_value.len() * 2)
    };

    let value_name = wide(VALUE_NAME);
    let status = unsafe {
        RegSetValueExW(
            handle,
            windows::core::PCWSTR(value_name.as_ptr()),
            Some(0),
            REG_SZ,
            Some(bytes),
        )
    };
    unsafe {
        let _ = RegCloseKey(handle);
    }

    if status != ERROR_SUCCESS {
        return Err(anyhow!("RegSetValueExW failed: error {}", status.0));
    }
    Ok(())
}

/// 打开（必要时创建）注册表键。
fn open_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<HKEY> {
    let sub_key = wide(SUB_KEY);
    let sub_key_ptr = windows::core::PCWSTR(sub_key.as_ptr());
    let mut handle = HKEY::default();

    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub_key_ptr, None, access, &mut handle) };
    if status == ERROR_SUCCESS {
        return Ok(handle);
    }

    // 键不存在时创建。RegCreateKeyW 只申请 KEY_WRITE，创建后按需要的
    // 权限重新打开，避免依赖 Win32_Security 特性的 RegCreateKeyExW。
    let status = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, sub_key_ptr, &mut handle) };
    if status != ERROR_SUCCESS {
        return Err(anyhow!("RegCreateKeyW failed: error {}", status.0));
    }
    unsafe {
        let _ = RegCloseKey(handle);
    }

    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub_key_ptr, None, access, &mut handle) };
    if status != ERROR_SUCCESS {
        return Err(anyhow!("RegOpenKeyExW failed: error {}", status.0));
    }
    Ok(handle)
}

/// 把字符串转成以 NUL 结尾的 UTF-16 缓冲区。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 解析 "left,top,width,height" 格式。
fn parse_rect(text: &str) -> Option<WindowRect> {
    let mut parts = text.split(',');
    let mut next = || parts.next()?.trim().parse::<i32>().ok();

    let rect = WindowRect {
        left: next()?,
        top: next()?,
        width: next()?,
        height: next()?,
    };
    if !rect.is_valid() {
        return None;
    }
    Some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_rect() {
        let rect = parse_rect("100,200,800,600").expect("parse");
        assert_eq!(rect.left, 100);
        assert_eq!(rect.top, 200);
        assert_eq!(rect.width, 800);
        assert_eq!(rect.height, 600);
    }

    #[test]
    fn rejects_broken_rect() {
        assert!(parse_rect("").is_none());
        assert!(parse_rect("1,2,3").is_none());
        assert!(parse_rect("a,b,c,d").is_none());
        assert!(parse_rect("0,0,0,0").is_none());
        assert!(parse_rect("0,0,-10,10").is_none());
    }

    #[test]
    fn round_trips_through_registry() {
        let rect = WindowRect {
            left: 120,
            top: 240,
            width: 1024,
            height: 768,
        };
        save_window_placement(rect).expect("save");
        let loaded = load_window_placement().expect("load");
        assert_eq!(loaded, rect);
        // 清理测试数据，避免污染用户环境
        if let Ok(handle) = open_key(KEY_READ | KEY_WRITE) {
            let name = wide(VALUE_NAME);
            unsafe {
                let _ = windows::Win32::System::Registry::RegDeleteValueW(
                    handle,
                    windows::core::PCWSTR(name.as_ptr()),
                );
                let _ = RegCloseKey(handle);
            }
        }
    }
}
