use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Config {
    pub config_version: i32,
    pub general: General,
    pub source_device_id: String,
    #[serde(default)]
    pub outputs: Vec<Output>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct General {
    pub language: String,
    pub minimized: bool,          // Whether to start minimized to tray
    pub start_with_windows: bool, // Whether to launch app at system startup
    pub auto_route: bool,         // Whether to auto-start routing on app launch
    #[serde(default = "default_true")]
    pub nav_pane_expanded: bool, // Whether the navigation pane is expanded
    #[serde(default)]
    pub backdrop: Backdrop, // Window backdrop material
    #[serde(default = "default_true")]
    pub close_to_tray: bool, // Whether closing the window minimizes to tray
    #[serde(default = "default_true")]
    pub auto_update_check: bool, // Whether to automatically check for updates on startup
    /// 全局热键，用于显示/隐藏窗口；空字符串表示不注册。
    ///
    /// 格式如 `Ctrl+Alt+M`，由 `app_core::hotkey` 解析。
    #[serde(default)]
    pub global_hotkey: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum Backdrop {
    #[default]
    Mica,
    MicaAlt,
    Acrylic,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Output {
    pub device_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Mix mode: "Stereo", "Left", "Right", "Center", etc.
    #[serde(default)]
    pub channel_mode: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum ChannelMode {
    #[default]
    Stereo,
    LeftMono,
    RightMono,
    Mono,
    Swap,
    LeftOnly,
    RightOnly,
}

impl ChannelMode {
    pub fn from_config(value: Option<&str>) -> Self {
        match value {
            Some("LeftMono") | Some("Left") => Self::LeftMono,
            Some("RightMono") | Some("Right") => Self::RightMono,
            Some("Mono") => Self::Mono,
            Some("Swap") => Self::Swap,
            Some("LeftOnly") => Self::LeftOnly,
            Some("RightOnly") => Self::RightOnly,
            _ => Self::Stereo,
        }
    }

    pub fn as_config_str(self) -> &'static str {
        match self {
            Self::Stereo => "Stereo",
            Self::LeftMono => "LeftMono",
            Self::RightMono => "RightMono",
            Self::Mono => "Mono",
            Self::Swap => "Swap",
            Self::LeftOnly => "LeftOnly",
            Self::RightOnly => "RightOnly",
        }
    }
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self::default_with_language("en")
    }
}

impl Config {
    /// 生成一份默认配置，界面语言由调用方指定。
    ///
    /// 首次启动时调用方用系统 UI 语言填充（见
    /// `app_core::i18n::detect_system_locale`）；`Config::default()`
    /// 仍然用英文，保持既有语义。
    pub fn default_with_language(language: &str) -> Self {
        Self {
            config_version: 1,
            general: General {
                language: language.to_string(),
                auto_route: false,
                minimized: false,
                start_with_windows: false,
                nav_pane_expanded: true,
                backdrop: Backdrop::default(),
                close_to_tray: true,
                auto_update_check: true,
                global_hotkey: String::new(),
            },
            source_device_id: String::new(),
            outputs: Vec::new(),
        }
    }

    /// 校验 config 加载后是否处于可接受的 schema 范围内。
    ///
    /// 1. `config_version` 必须匹配当前支持的主版本(目前 1),
    ///    不匹配时记 warn 但不阻断,方便未来追加 v2 schema 时降级读取。
    /// 2. `outputs` 中 `device_id` 必须唯一,防止用户手改配置文件
    ///    填入重复条目导致 controller 逻辑误判。
    pub fn validate(&self) -> Result<()> {
        if self.config_version != 1 {
            log::warn!(
                "Config config_version={} differs from supported version 1; \
                 some fields may be ignored",
                self.config_version
            );
        }

        let mut seen = std::collections::HashSet::new();
        for out in &self.outputs {
            if !seen.insert(&out.device_id) {
                anyhow::bail!("Duplicate output device_id in config: {}", out.device_id);
            }
        }

        Ok(())
    }
}

/// Manager providing thread-safe access and persistence.
///
/// 可克隆：内部只有路径与一个 `Arc<RwLock<Config>>`，克隆出来的实例共享
/// 同一份内存状态。UI 线程与路由线程各自持有一份，改动立刻互相可见。
#[derive(Clone)]
pub struct ConfigManager {
    path: PathBuf,
    inner: Arc<RwLock<Config>>,
}

impl ConfigManager {
    /// Load config from given base path (parent directory), or from default directory if None.
    /// If file does not exist, a default config is created and written.
    pub fn load(basepath: Option<PathBuf>) -> Result<Self> {
        Self::load_with_default_language(basepath, "en")
    }

    /// Same as [`ConfigManager::load`], but the language of the freshly
    /// created default config is taken from `language`.
    ///
    /// An already existing config file wins: the language the user picked in
    /// the settings UI is never overwritten by the system language.
    pub fn load_with_default_language(basepath: Option<PathBuf>, language: &str) -> Result<Self> {
        let config_dir = basepath.unwrap_or_else(default_config_dir);
        let config_path = config_dir.join("settings.toml");

        if config_path.exists() {
            let s = fs::read_to_string(&config_path)
                .with_context(|| format!("reading config file: {}", config_path.display()))?;
            let cfg: Config = toml::from_str(&s).context("parsing TOML config")?;
            cfg.validate()?;
            Ok(Self {
                path: config_path,
                inner: Arc::new(RwLock::new(cfg)),
            })
        } else {
            // create parent dir if needed
            fs::create_dir_all(&config_dir)
                .with_context(|| format!("creating config dir: {}", config_dir.display()))?;

            let cfg = Config::default_with_language(language);
            let toml_str = toml::to_string_pretty(&cfg).context("serializing default config")?;
            let mut f = fs::File::create(&config_path)
                .with_context(|| format!("creating config file: {}", config_path.display()))?;
            f.write_all(toml_str.as_bytes())?;
            Ok(Self {
                path: config_path,
                inner: Arc::new(RwLock::new(cfg)),
            })
        }
    }

    /// 只读取已存在的配置文件，不存在时返回 `None`。
    ///
    /// 服务进程用它读路由配置：如果配置文件还不存在就直接返回 None，
    /// 而不是像 `load` 那样创建一个默认文件——否则服务先启动时会把
    /// GUI 准备好的语言设置覆盖掉。
    pub fn load_if_exists(basepath: Option<PathBuf>) -> Result<Option<Self>> {
        let config_dir = basepath.unwrap_or_else(default_config_dir);
        let config_path = config_dir.join("settings.toml");
        if !config_path.exists() {
            return Ok(None);
        }
        let s = fs::read_to_string(&config_path)
            .with_context(|| format!("reading config file: {}", config_path.display()))?;
        let cfg: Config = toml::from_str(&s).context("parsing TOML config")?;
        cfg.validate()?;
        Ok(Some(Self {
            path: config_path,
            inner: Arc::new(RwLock::new(cfg)),
        }))
    }

    /// Save current config to disk atomically.
    pub fn save(&self) -> Result<()> {
        let cfg = self.inner.read().clone();
        cfg.validate()?;
        let tmp = self.path.with_extension("toml.tmp");
        let s = toml::to_string_pretty(&cfg).context("serializing config")?;
        let mut f = fs::File::create(&tmp)
            .with_context(|| format!("creating tmp config file: {}", tmp.display()))?;
        f.write_all(s.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &self.path).with_context(|| {
            format!(
                "renaming tmp config {} -> {}",
                tmp.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }

    /// Returns a cloneable handle to the inner Arc<RwLock<Config>> to allow reads/writes.
    pub fn handle(&self) -> Arc<RwLock<Config>> {
        self.inner.clone()
    }

    /// Atomically update config using closure and persist to disk.
    pub fn update<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut Config),
    {
        {
            let mut cfg = self.inner.write();
            f(&mut cfg);
        }
        self.save()
    }

    /// Access path used for persistence (useful for tests)
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn default_config_dir() -> PathBuf {
    // Use the directory where the executable is located
    std::env::current_exe()
        .ok()
        .and_then(|exe_path| exe_path.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_serialize_deserialize() {
        let cfg = Config {
            config_version: 1,
            general: General {
                language: "en".to_string(),
                auto_route: false,
                minimized: false,
                start_with_windows: false,
                nav_pane_expanded: true,
                backdrop: Backdrop::default(),
                close_to_tray: true,
                auto_update_check: true,
                global_hotkey: String::new(),
            },
            source_device_id: "src1".to_string(),
            outputs: vec![Output {
                device_id: "out1".to_string(),
                enabled: true,
                channel_mode: None,
            }],
        };
        let s = toml::to_string_pretty(&cfg).expect("serialize");
        let decoded: Config = toml::from_str(&s).expect("deserialize");
        assert_eq!(decoded.config_version, 1);
        assert_eq!(decoded.outputs.len(), 1);
        assert_eq!(decoded.outputs[0].device_id, "out1");
    }

    #[test]
    fn load_creates_default_file() {
        let td = tempdir().unwrap();
        let config_dir = td.path().to_path_buf();
        let expected_config_path = config_dir.join("settings.toml");
        assert!(!expected_config_path.exists());
        let mgr = ConfigManager::load(Some(config_dir)).expect("load");
        assert!(expected_config_path.exists());
        let cfg = mgr.handle();
        let c = cfg.read();
        assert_eq!(c.config_version, 1);
    }

    #[test]
    fn load_with_default_language_uses_given_language() {
        let td = tempdir().unwrap();
        let config_dir = td.path().to_path_buf();
        let expected_config_path = config_dir.join("settings.toml");

        let mgr = ConfigManager::load_with_default_language(Some(config_dir), "zh").expect("load");
        assert_eq!(mgr.handle().read().general.language, "zh");

        let s = fs::read_to_string(&expected_config_path).expect("read file");
        assert!(s.contains("language = \"zh\""));
    }

    #[test]
    fn existing_config_keeps_user_language() {
        let td = tempdir().unwrap();
        let config_dir = td.path().to_path_buf();
        let expected_config_path = config_dir.join("settings.toml");

        // 模拟"用户已经手动选过语言"的既有配置
        let existing = Config::default_with_language("en");
        fs::write(
            &expected_config_path,
            toml::to_string_pretty(&existing).expect("serialize"),
        )
        .expect("write file");

        // 系统语言是中文，但既有配置优先，不能被覆盖
        let mgr = ConfigManager::load_with_default_language(Some(config_dir), "zh").expect("load");
        assert_eq!(mgr.handle().read().general.language, "en");
    }

    #[test]
    fn update_persists_changes() {
        let td = tempdir().unwrap();
        let config_dir = td.path().to_path_buf();
        let expected_config_path = config_dir.join("settings.toml");
        let mgr = ConfigManager::load(Some(config_dir)).expect("load");
        mgr.update(|c| {
            c.general.language = "zh".to_string();
        })
        .expect("update");
        let s = fs::read_to_string(&expected_config_path).expect("read file");
        assert!(s.contains("language = \"zh\""));
    }
}
