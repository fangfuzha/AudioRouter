//! 国际化模块，提供简单的键值对翻译。
//! 当前支持：英语 (en)、简体中文 (zh)。

mod en;
mod zh;

use std::collections::HashMap;

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
            "zh" => zh::ZH_MAP.iter().copied().collect(),
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
            "zh" => zh::ZH_MAP.iter().copied().collect(),
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

    /// 获取可用语言列表
    pub fn available_languages() -> Vec<(&'static str, &'static str)> {
        vec![("en", "English"), ("zh", "简体中文")]
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
