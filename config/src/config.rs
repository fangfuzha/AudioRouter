use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Represents different channel mixing modes for routing stereo audio to multi-channel outputs.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Type, Default,
)]
pub enum ChannelMixMode {
    /// Stereo: Pass through both channels unchanged
    #[default]
    Stereo = 0,
    /// Left: Left channel to all outputs
    Left = 1,
    /// Right: Right channel to all outputs
    Right = 2,
    /// Center: Mono mix (L+R)/2 to all outputs
    Center = 3,
    /// Front Left: Same as Left
    FrontLeft = 4,
    /// Front Right: Same as Right
    FrontRight = 5,
    /// Back Left: Left channel at 85% volume to all outputs
    BackLeft = 6,
    /// Back Right: Right channel at 85% volume to all outputs
    BackRight = 7,
    /// Back/Surround: Mono mix to all outputs
    BackSurround = 8,
    /// Subwoofer (LFE): Mono mix with 1.3x boost to all outputs
    Subwoofer = 9,
}

/// Main configuration structure
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
    #[serde(default = "default_channel_mode")]
    pub channel_mode: ChannelMixMode,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

fn default_channel_mode() -> ChannelMixMode {
    ChannelMixMode::Stereo
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
                channel_mode: ChannelMixMode::Stereo,
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
