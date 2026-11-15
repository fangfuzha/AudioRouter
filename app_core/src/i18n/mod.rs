//! 国际化模块，提供简单的键值对翻译。
//! 当前支持：英语 (en)、简体中文 (zh)。

mod en;
mod zh;

use std::collections::HashMap;

/// 英文 locale 代码
pub const LOCALE_EN: &str = "en";

/// 简体中文 locale 代码
pub const LOCALE_ZH: &str = "zh";

/// Windows LANGID 中子语言 ID 的掩码（低 10 位）
const SUBLANG_MASK: u16 = 0x3FF;

/// LANG_CHINESE：中文主语言 ID
const LANG_CHINESE: u16 = 0x04;

/// 把 Windows LANGID（如 0x0804）映射到应用内部使用的 locale 代码。
///
/// 只比较主语言 ID：中文（0x04）→ `zh`，其余一律 `en`。
/// 繁体中文同样映射到 `zh`，因为应用目前只提供简体中文翻译。
pub fn locale_from_lang_id(lang_id: u16) -> &'static str {
    if lang_id & SUBLANG_MASK == LANG_CHINESE {
        LOCALE_ZH
    } else {
        LOCALE_EN
    }
}

/// 探测当前用户的系统 UI 语言，返回应用内部使用的 locale 代码。
///
/// 用 `GetUserDefaultUILanguage`，与 Inno Setup 安装向导的
/// `LanguageDetectionMethod=uilanguage`（默认值）语义一致，
/// 因此安装向导与应用界面会跟随同一个系统语言。
#[cfg(target_os = "windows")]
pub fn detect_system_locale() -> &'static str {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;

    // SAFETY: GetUserDefaultUILanguage 无前置条件，不会失败。
    let lang_id = unsafe { GetUserDefaultUILanguage() };
    locale_from_lang_id(lang_id)
}

/// 非 Windows 平台没有系统语言 API，退回英文。
#[cfg(not(target_os = "windows"))]
pub fn detect_system_locale() -> &'static str {
    LOCALE_EN
}

/// 多语言管理器
#[derive(Clone)]
pub struct I18n {
    locale: String,
    strings: HashMap<&'static str, &'static str>,
}

impl I18n {
    /// 根据 locale 代码创建翻译器
    pub fn new(locale: &str) -> Self {
        let strings = match locale {
            LOCALE_ZH => zh::ZH_MAP.iter().copied().collect(),
            _ => en::EN_MAP.iter().copied().collect(),
        };
        Self {
            locale: locale.to_string(),
            strings,
        }
    }

    /// 切换语言
    pub fn set_locale(&mut self, locale: &str) {
        self.strings = match locale {
            LOCALE_ZH => zh::ZH_MAP.iter().copied().collect(),
            _ => en::EN_MAP.iter().copied().collect(),
        };
        self.locale = locale.to_string();
    }

    /// 获取当前 locale 代码
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// 翻译 key，找不到时返回 key 本身
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.strings.get(key).copied().unwrap_or(key)
    }
}

/// 翻译宏，便捷调用
#[macro_export]
macro_rules! t {
    ($i18n:expr, $key:expr) => {
        $i18n.t($key)
    };
    ($i18n:expr, $key:expr, $($args:tt)*) => {{
        let _ = ($i18n, $key);
        // 带参数翻译暂不支持直接格式化，可用 format! + t 组合
        compile_error!("Parameterized i18n not supported yet, use format!() manually");
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_id_maps_to_locale() {
        assert_eq!(locale_from_lang_id(0x0804), LOCALE_ZH); // zh-CN
        assert_eq!(locale_from_lang_id(0x0404), LOCALE_ZH); // zh-TW
        assert_eq!(locale_from_lang_id(0x0C04), LOCALE_ZH); // zh-HK
        assert_eq!(locale_from_lang_id(0x0409), LOCALE_EN); // en-US
        assert_eq!(locale_from_lang_id(0x040C), LOCALE_EN); // fr-FR
        assert_eq!(locale_from_lang_id(0x0000), LOCALE_EN); // 无效值
    }

    #[test]
    fn detect_returns_supported_locale() {
        let locale = detect_system_locale();
        assert!(
            locale == LOCALE_ZH || locale == LOCALE_EN,
            "unexpected locale: {locale}"
        );
    }

    #[test]
    fn new_uses_zh_map_only_for_zh() {
        assert_eq!(I18n::new(LOCALE_ZH).locale(), LOCALE_ZH);
        assert_eq!(I18n::new(LOCALE_EN).locale(), LOCALE_EN);
        // 未知 locale 落回英文词表
        assert_eq!(
            I18n::new("fr").t("Language"),
            I18n::new(LOCALE_EN).t("Language")
        );
    }
}
