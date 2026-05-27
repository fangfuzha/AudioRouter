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

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct General {
    pub language: String,
    pub minimized: bool,          // Whether to start minimized to tray
    pub start_with_windows: bool, // Whether to launch app at system startup
    pub auto_route: bool,         // Whether to auto-start routing on app launch
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
        Self {
            config_version: 1,
            general: General {
                language: "en".to_string(),
                auto_route: false,
                minimized: false,
                start_with_windows: false,
            },
            source_device_id: String::new(),
            outputs: Vec::new(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        // Currently no validation needed
        Ok(())
    }
}

/// Manager providing thread-safe access and persistence.
pub struct ConfigManager {
    path: PathBuf,
    inner: Arc<RwLock<Config>>,
}

impl ConfigManager {
    /// Load config from given base path (parent directory), or from default directory if None.
    /// If file does not exist, a default config is created and written.
    pub fn load(basepath: Option<PathBuf>) -> Result<Self> {
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

            let cfg = Config::default();
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
