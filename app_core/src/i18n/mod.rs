//! 国际化模块，提供简单的键值对翻译。
//! 当前支持：英语 (en)、简体中文 (zh)。

mod en;
mod zh;

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
///
/// 词表直接引用编译期的静态切片，不在堆上建 HashMap：
///
/// - 省掉一份常驻的哈希表分配；
/// - I18n 的 clone 从「复制整张表」变成「复制几个指针」。
///
/// 界面每帧都要 clone 一次 I18n，整表复制的开销和堆压力都不小。
/// 词表只有一百来条，线性查找完全够快。
#[derive(Clone)]
pub struct I18n {
    locale: String,
    strings: &'static [(&'static str, &'static str)],
}

impl I18n {
    /// 根据 locale 代码创建翻译器
    pub fn new(locale: &str) -> Self {
        Self {
            locale: locale.to_string(),
            strings: Self::table_for(locale),
        }
    }

    /// 切换语言
    pub fn set_locale(&mut self, locale: &str) {
        self.strings = Self::table_for(locale);
        self.locale = locale.to_string();
    }

    /// 取指定 locale 对应的静态词表
    fn table_for(locale: &str) -> &'static [(&'static str, &'static str)] {
        match locale {
            LOCALE_ZH => zh::ZH_MAP,
            _ => en::EN_MAP,
        }
    }

    /// 获取当前 locale 代码
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// 翻译 key，找不到时返回 key 本身
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.strings
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| *v)
            .unwrap_or(key)
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

    /// 新增文案时最容易漏掉其中一种语言，这里把两个词表的 key 集合对齐。
    #[test]
    fn en_and_zh_maps_have_the_same_keys() {
        use std::collections::BTreeSet;

        let en_keys: BTreeSet<&str> = en::EN_MAP.iter().map(|(k, _)| *k).collect();
        let zh_keys: BTreeSet<&str> = zh::ZH_MAP.iter().map(|(k, _)| *k).collect();

        let missing_in_zh: Vec<&&str> = en_keys.difference(&zh_keys).collect();
        let missing_in_en: Vec<&&str> = zh_keys.difference(&en_keys).collect();
        assert!(missing_in_zh.is_empty(), "missing in zh: {missing_in_zh:?}");
        assert!(missing_in_en.is_empty(), "missing in en: {missing_in_en:?}");
    }
}
