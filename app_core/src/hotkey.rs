//! 全局热键字符串的解析与格式化（纯逻辑，不注册热键）。
//!
//! 配置里保存的是用户可读的字符串，例如 `Ctrl+Alt+M`；本模块把它解析成
//! Win32 `RegisterHotKey` 需要的「修饰键位掩码 + 虚拟键码」。
//! 实际注册由 GUI 层完成（需要窗口句柄与消息循环），这样解析逻辑可以
//! 脱离 GUI 单独做单元测试。

use anyhow::{Result, bail};

/// `MOD_ALT`
pub const MOD_ALT: u32 = 0x0001;
/// `MOD_CONTROL`
pub const MOD_CONTROL: u32 = 0x0002;
/// `MOD_SHIFT`
pub const MOD_SHIFT: u32 = 0x0004;
/// `MOD_WIN`
pub const MOD_WIN: u32 = 0x0008;

/// 解析后的热键：修饰键掩码 + 虚拟键码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

/// 解析 `Ctrl+Alt+M` 形式的字符串。
///
/// 规则：
/// - 大小写不敏感，允许空格（`Ctrl + Alt + M`）；
/// - 修饰键可用 `Ctrl`/`Control`、`Alt`、`Shift`、`Win`；
/// - 必须且只能有一个非修饰键，且必须带至少一个修饰键
///   （否则会和普通按键输入冲突，Windows 也不接受）。
pub fn parse(text: &str) -> Result<Hotkey> {
    let text = text.trim();
    if text.is_empty() {
        bail!("empty hotkey");
    }

    let mut modifiers = 0u32;
    let mut key: Option<u32> = None;

    for part in text.split('+') {
        let part = part.trim();
        if part.is_empty() {
            bail!("malformed hotkey: {text}");
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL,
            "alt" => modifiers |= MOD_ALT,
            "shift" => modifiers |= MOD_SHIFT,
            "win" | "super" | "meta" => modifiers |= MOD_WIN,
            _ => {
                if key.is_some() {
                    bail!("malformed hotkey: more than one key in {text}");
                }
                key = Some(
                    key_code(part).ok_or_else(|| anyhow::anyhow!("unsupported key \"{part}\""))?,
                );
            }
        }
    }

    let Some(vk) = key else {
        bail!("malformed hotkey: no key in {text}");
    };
    if modifiers == 0 {
        bail!("malformed hotkey: at least one modifier is required");
    }
    Ok(Hotkey { modifiers, vk })
}

/// 把 `{modifiers, vk}` 格式化回规范字符串，例如 `Ctrl+Alt+M`。
///
/// 用于把用户输入的宽松写法（`control + alt + m`）回显成规范形式。
pub fn format(hotkey: Hotkey) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if hotkey.modifiers & MOD_CONTROL != 0 {
        parts.push("Ctrl");
    }
    if hotkey.modifiers & MOD_ALT != 0 {
        parts.push("Alt");
    }
    if hotkey.modifiers & MOD_SHIFT != 0 {
        parts.push("Shift");
    }
    if hotkey.modifiers & MOD_WIN != 0 {
        parts.push("Win");
    }

    let key_name = key_name(hotkey.vk).unwrap_or("?");
    if parts.is_empty() {
        key_name.to_string()
    } else {
        format!("{}+{key_name}", parts.join("+"))
    }
}

/// 虚拟键码 → 规范按键名。
///
/// GUI 捕获按键时也要用到（把 Win32 虚拟键码翻译成 `Ctrl+Alt+M` 里的
/// 那一段），所以这里是公开的。
pub fn key_name(vk: u32) -> Option<&'static str> {
    const NAMED: &[(u32, &str)] = &[
        (0x08, "Backspace"),
        (0x09, "Tab"),
        (0x0D, "Enter"),
        (0x1B, "Escape"),
        (0x20, "Space"),
        (0x21, "PageUp"),
        (0x22, "PageDown"),
        (0x23, "End"),
        (0x24, "Home"),
        (0x25, "Left"),
        (0x26, "Up"),
        (0x27, "Right"),
        (0x28, "Down"),
        (0x2D, "Insert"),
        (0x2E, "Delete"),
        (0xBA, ";"),
        (0xBB, "="),
        (0xBC, ","),
        (0xBD, "-"),
        (0xBE, "."),
        (0xBF, "/"),
        (0xC0, "`"),
        (0xDB, "["),
        (0xDC, "\\"),
        (0xDD, "]"),
        (0xDE, "'"),
    ];

    if let Some((_, name)) = NAMED.iter().find(|(code, _)| *code == vk) {
        return Some(name);
    }
    // A-Z
    if (0x41..=0x5A).contains(&vk) {
        // 返回静态字符串：用查表避免分配。
        const LETTERS: [&str; 26] = [
            "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q",
            "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
        ];
        return Some(LETTERS[(vk - 0x41) as usize]);
    }
    // 0-9
    if (0x30..=0x39).contains(&vk) {
        const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
        return Some(DIGITS[(vk - 0x30) as usize]);
    }
    // F1-F24
    if (0x70..=0x87).contains(&vk) {
        const FKEYS: [&str; 24] = [
            "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13",
            "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
        ];
        return Some(FKEYS[(vk - 0x70) as usize]);
    }
    None
}

/// 判断一个虚拟键码是不是「纯修饰键」。
///
/// 修饰键本身不能作为快捷键的主键，捕获时要把它们忽略掉，
/// 继续等用户按下真正的按键。
pub fn is_modifier_key(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | 0x14 | 0x15 | 0x5B | 0x5C | 0xA0..=0xA5
    )
}

/// 修饰键虚拟键码 → 修饰键掩码（`MOD_*`）。
///
/// 捕获组合键时用它自己累计修饰键状态：低级键盘钩子给的是一串独立的
/// 按键事件，按下就加上、抬起就减去，比 `GetKeyState` 更可靠
/// （`GetKeyState` 只反映「本线程最后处理的消息」那一刻的状态）。
pub fn modifier_bit(vk: u32) -> Option<u32> {
    match vk {
        0x10 | 0xA0 | 0xA1 => Some(MOD_SHIFT),
        0x11 | 0xA2 | 0xA3 => Some(MOD_CONTROL),
        0x12 | 0x14 | 0x15 | 0xA4 | 0xA5 => Some(MOD_ALT),
        0x5B | 0x5C => Some(MOD_WIN),
        _ => None,
    }
}

/// 由「虚拟键码 + 修饰键掩码」生成规范快捷键字符串。
///
/// GUI 捕获键盘输入时走这条路径：用户按下 Ctrl+Alt+M 之后，
/// 窗口过程拿到虚拟键码和当前修饰键状态，交给这里翻译成
/// `Ctrl+Alt+M` 这样的字符串。
///
/// 返回 `None` 表示这个键不能作为快捷键（只有修饰键、或者按键名不认识）。
pub fn describe(vk: u32, modifiers: u32) -> Option<String> {
    if is_modifier_key(vk) || key_name(vk).is_none() {
        return None;
    }
    Some(format(Hotkey { modifiers, vk }))
}

/// 按键名 → 虚拟键码。
fn key_code(name: &str) -> Option<u32> {
    let upper = name.to_ascii_uppercase();

    // 单个字母 / 数字
    if upper.len() == 1 {
        let ch = upper.as_bytes()[0];
        if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
            return Some(ch as u32);
        }
    }

    // F1-F24
    if let Some(rest) = upper.strip_prefix('F')
        && let Ok(n) = rest.parse::<u32>()
        && (1..=24).contains(&n)
    {
        return Some(0x70 + n - 1);
    }

    const NAMED: &[(&str, u32)] = &[
        ("BACKSPACE", 0x08),
        ("TAB", 0x09),
        ("ENTER", 0x0D),
        ("RETURN", 0x0D),
        ("ESC", 0x1B),
        ("ESCAPE", 0x1B),
        ("SPACE", 0x20),
        ("PAGEUP", 0x21),
        ("PAGEDOWN", 0x22),
        ("END", 0x23),
        ("HOME", 0x24),
        ("LEFT", 0x25),
        ("UP", 0x26),
        ("RIGHT", 0x27),
        ("DOWN", 0x28),
        ("INSERT", 0x2D),
        ("DELETE", 0x2E),
        (";", 0xBA),
        ("=", 0xBB),
        (",", 0xBC),
        ("-", 0xBD),
        (".", 0xBE),
        ("/", 0xBF),
        ("`", 0xC0),
        ("[", 0xDB),
        ("\\", 0xDC),
        ("]", 0xDD),
        ("'", 0xDE),
    ];

    NAMED
        .iter()
        .find(|(candidate, _)| *candidate == upper)
        .map(|(_, code)| *code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_combinations() {
        let hk = parse("Ctrl+Alt+M").expect("parse");
        assert_eq!(hk.modifiers, MOD_CONTROL | MOD_ALT);
        assert_eq!(hk.vk, 0x4D);

        let hk = parse("shift+ctrl+f5").expect("parse");
        assert_eq!(hk.modifiers, MOD_CONTROL | MOD_SHIFT);
        assert_eq!(hk.vk, 0x74);

        let hk = parse(" Win + Space ").expect("parse");
        assert_eq!(hk.modifiers, MOD_WIN);
        assert_eq!(hk.vk, 0x20);
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(parse("").is_err(), "empty");
        assert!(parse("M").is_err(), "no modifier");
        assert!(parse("Ctrl+").is_err(), "trailing plus");
        assert!(parse("Ctrl+Alt").is_err(), "no key");
        assert!(parse("Ctrl+M+N").is_err(), "two keys");
        assert!(parse("Ctrl+Nope").is_err(), "unknown key");
    }

    #[test]
    fn formats_back_to_canonical_form() {
        assert_eq!(format(parse("control+alt+m").unwrap()), "Ctrl+Alt+M");
        assert_eq!(format(parse("shift+f12").unwrap()), "Shift+F12");
        assert_eq!(format(parse("win+space").unwrap()), "Win+Space");
    }

    #[test]
    fn describes_captured_keyboard_input() {
        // Ctrl+Alt+M：M 的虚拟键码是 0x4D。
        assert_eq!(
            describe(0x4D, MOD_CONTROL | MOD_ALT).as_deref(),
            Some("Ctrl+Alt+M")
        );
        assert_eq!(describe(0x20, MOD_WIN).as_deref(), Some("Win+Space"));
        assert_eq!(
            describe(0x74, MOD_CONTROL | MOD_SHIFT).as_deref(),
            Some("Ctrl+Shift+F5")
        );
    }

    #[test]
    fn rejects_modifier_only_and_unknown_keys() {
        // 只按了 Ctrl，没有任何主键：不能用。
        assert!(describe(0x11, MOD_CONTROL).is_none());
        assert!(describe(0xA2, MOD_CONTROL).is_none());
        // 不认识的键（例如媒体键）。
        assert!(describe(0xB3, MOD_CONTROL).is_none());
        assert!(is_modifier_key(0x11));
        assert!(is_modifier_key(0x5B));
        assert!(!is_modifier_key(0x4D));
    }

    #[test]
    fn maps_modifier_keys_to_mask_bits() {
        assert_eq!(modifier_bit(0x11), Some(MOD_CONTROL));
        assert_eq!(modifier_bit(0xA2), Some(MOD_CONTROL));
        assert_eq!(modifier_bit(0x12), Some(MOD_ALT));
        assert_eq!(modifier_bit(0x10), Some(MOD_SHIFT));
        assert_eq!(modifier_bit(0xA1), Some(MOD_SHIFT));
        assert_eq!(modifier_bit(0x5B), Some(MOD_WIN));
        assert_eq!(modifier_bit(0x4D), None);
    }
}
