//! 应用控制层，封装与具体 GUI 框架无关的状态和操作。

use audio_core::com_service::device::{DeviceInfo, get_all_output_devices};
use audio_core::router::{ChannelMode, Router, RouterConfig, RouterTarget};
use config::ConfigManager;
use config::config::{General, Output};

use crate::i18n::I18n;

/// 应用业务状态和操作入口。
pub struct AppController {
    pub config_manager: ConfigManager,
    pub router: Router,
    pub i18n: I18n,
    pub devices: Vec<DeviceInfo>,
    pub selected_source: Option<String>,
    pub is_running: bool,
    pub status_text: String,
    pub draft_general: General,
    initialized: bool,
}

impl AppController {
    pub fn new(config_manager: ConfigManager, router: Router) -> Self {
        let cfg = config_manager.handle().read().clone();
        let locale = cfg.general.language.clone();

        Self {
            config_manager,
            router,
            i18n: I18n::new(&locale),
            devices: Vec::new(),
            selected_source: if cfg.source_device_id.is_empty() {
                None
            } else {
                Some(cfg.source_device_id.clone())
            },
            is_running: false,
            status_text: String::new(),
            draft_general: cfg.general.clone(),
            initialized: false,
        }
    }

    pub fn init(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.refresh_devices();
        self.is_running = self.router.is_running();

        if self.devices.is_empty() {
            self.status_text = self.i18n.t("NoDevices").to_string();
        }

        self.start_auto_route_if_enabled();
    }

    pub fn refresh_devices(&mut self) {
        match get_all_output_devices() {
            Ok(devices) => self.devices = devices,
            Err(e) => {
                log::error!("Failed to enumerate devices: {e}");
                self.status_text = self.i18n.t("ErrorLoadingDevices").to_string();
            }
        }
    }

    pub fn select_source_device(&mut self, device_id: String) {
        self.selected_source = Some(device_id);
        self.save_routing_config();
    }

    pub fn set_output_enabled(&mut self, device_id: &str, enabled: bool) {
        let device_id = device_id.to_string();
        if let Err(e) = self.config_manager.update(|cfg| {
            if let Some(output) = cfg.outputs.iter_mut().find(|o| o.device_id == device_id) {
                output.enabled = enabled;
            } else {
                cfg.outputs.push(Output {
                    device_id,
                    enabled,
                    channel_mode: Some(ChannelMode::Stereo.as_config_str().to_string()),
                });
            }
        }) {
            log::error!("Save output enabled state failed: {e}");
        }
    }

    pub fn set_output_channel_mode(&mut self, device_id: &str, channel_mode: ChannelMode) {
        let device_id = device_id.to_string();
        if let Err(e) = self.config_manager.update(|cfg| {
            if let Some(output) = cfg.outputs.iter_mut().find(|o| o.device_id == device_id) {
                output.channel_mode = Some(channel_mode.as_config_str().to_string());
            } else {
                cfg.outputs.push(Output {
                    device_id,
                    enabled: false,
                    channel_mode: Some(channel_mode.as_config_str().to_string()),
                });
            }
        }) {
            log::error!("Save output channel mode failed: {e}");
        }
    }

    pub fn start_routing(&mut self) {
        let source_id = match &self.selected_source {
            Some(id) if !id.is_empty() => id.clone(),
            _ => {
                self.status_text = self.i18n.t("SelectDevice").to_string();
                return;
            }
        };

        let cfg = self.config_manager.handle().read().clone();
        let targets: Vec<RouterTarget> = self
            .devices
            .iter()
            .filter_map(|d| {
                if d.id == source_id {
                    return None;
                }
                cfg.outputs
                    .iter()
                    .find(|o| o.device_id == d.id && o.enabled)
                    .map(|o| RouterTarget {
                        device_id: d.id.clone(),
                        channel_mode: ChannelMode::from_config(o.channel_mode.as_deref()),
                    })
            })
            .collect();

        if targets.is_empty() {
            self.status_text = self.i18n.t("SelectDevice").to_string();
            return;
        }

        let router_cfg = RouterConfig {
            source_device_id: Some(source_id),
            targets,
        };

        self.status_text = self.i18n.t("Starting").to_string();
        match self.router.start(router_cfg) {
            Ok(()) => {
                self.is_running = true;
                let running_count = cfg.outputs.iter().filter(|o| o.enabled).count();
                self.status_text = running_count.to_string();
            }
            Err(e) => {
                self.status_text = format!("Error: {e}");
                log::error!("Start routing failed: {e}");
            }
        }
    }

    pub fn stop_routing(&mut self) {
        self.status_text = self.i18n.t("Stopping").to_string();
        match self.router.stop() {
            Ok(()) => {
                self.is_running = false;
                self.status_text = self.i18n.t("StatusReady").to_string();
            }
            Err(e) => {
                self.status_text = format!("Error: {e}");
                log::error!("Stop routing failed: {e}");
            }
        }
    }

    pub fn begin_settings_edit(&mut self) {
        let cfg = self.config_manager.handle().read().clone();
        self.draft_general = cfg.general;
    }

    pub fn save_general_config(&mut self) -> Option<String> {
        let new_language = self.draft_general.language.clone();

        if let Err(e) = self.config_manager.update(|cfg| {
            cfg.general = self.draft_general.clone();
        }) {
            log::error!("Save general config failed: {e}");
            return None;
        }

        if let Err(e) = crate::autostart::set_autostart(self.draft_general.start_with_windows) {
            self.status_text = format!("Error: {e}");
            log::error!("Set autostart failed: {e}");
            return None;
        }

        if new_language != self.i18n.locale() {
            self.i18n.set_locale(&new_language);
            return Some(new_language);
        }

        None
    }

    pub fn save_routing_config(&mut self) {
        let source_id = self.selected_source.clone().unwrap_or_default();
        let outputs: Vec<Output> = self
            .devices
            .iter()
            .filter(|d| d.id != source_id)
            .map(|d| {
                let cfg = self.config_manager.handle().read().clone();
                let existing = cfg.outputs.iter().find(|o| o.device_id == d.id);
                Output {
                    device_id: d.id.clone(),
                    enabled: existing.map(|o| o.enabled).unwrap_or(false),
                    channel_mode: existing.and_then(|o| o.channel_mode.clone()),
                }
            })
            .collect();

        if let Err(e) = self.config_manager.update(|cfg| {
            cfg.source_device_id = source_id;
            cfg.outputs = outputs;
        }) {
            log::error!("Save routing config failed: {e}");
        }
    }

    pub fn filtered_target_devices(&self) -> Vec<&DeviceInfo> {
        let source_id = self.selected_source.as_deref();
        self.devices
            .iter()
            .filter(|d| Some(d.id.as_str()) != source_id)
            .collect()
    }

    fn start_auto_route_if_enabled(&mut self) {
        let cfg = self.config_manager.handle().read().clone();
        if !cfg.general.auto_route || cfg.source_device_id.is_empty() {
            return;
        }

        let enabled_targets: Vec<RouterTarget> = cfg
            .outputs
            .iter()
            .filter(|o| o.enabled)
            .map(|o| RouterTarget {
                device_id: o.device_id.clone(),
                channel_mode: ChannelMode::from_config(o.channel_mode.as_deref()),
            })
            .collect();

        if enabled_targets.is_empty() {
            return;
        }

        let router_cfg = RouterConfig {
            source_device_id: Some(cfg.source_device_id.clone()),
            targets: enabled_targets,
        };
        if self.router.start(router_cfg).is_ok() {
            self.is_running = true;
        }
    }
}
