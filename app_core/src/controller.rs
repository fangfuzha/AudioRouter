//! 应用控制层，封装与具体 GUI 框架无关的状态和操作。

use crate::bus::{Request, RouterEvent, ServiceHandle, StateSnapshot};
use audio_core::com_service::device::DeviceInfo;
use audio_core::router::{ChannelMode, RouterConfig, RouterTarget};
use config::ConfigManager;
use config::config::{General, Output};

use crate::i18n::I18n;

/// 状态文本的语义级别，GUI 用它决定配色 / 是否弹 InfoBar。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusLevel {
    /// 空闲 / 就绪，无突出信息。
    #[default]
    Idle,
    /// 正常运行。
    Ok,
    /// 需要提醒但未失败（例如设备变更后正在重启）。
    Warning,
    /// 出错。
    Error,
}

/// Start 按钮当前被禁用的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartBlock {
    /// 还没有设备（枚举未完成或系统里没有输出设备）。
    NoDevices,
    /// 没有选择源设备。
    NoSource,
    /// 没有勾选任何输出设备。
    NoOutput,
}

/// 应用业务状态和操作入口。
pub struct AppController {
    pub config_manager: ConfigManager,
    /// 进程内服务句柄：把请求丢给服务线程。
    pub service: ServiceHandle,
    pub i18n: I18n,
    pub devices: Vec<DeviceInfo>,
    pub selected_source: Option<String>,
    pub is_running: bool,
    pub status_text: String,
    /// `status_text` 的语义级别，供 GUI 上色。
    pub status_level: StatusLevel,
    /// 全局热键注册失败时的提示（热键在常驻服务进程里注册）。
    pub hotkey_error: Option<String>,
    pub draft_general: General,
    initialized: bool,
    /// 已经处理过的路由事件序号，用来去重。
    last_event_seq: u64,
    /// 是否已经尝试过「开机自动路由」。
    auto_route_checked: bool,
}

impl AppController {
    pub fn new(config_manager: ConfigManager, service: ServiceHandle) -> Self {
        let cfg = config_manager.handle().read().clone();
        let locale = cfg.general.language.clone();

        Self {
            config_manager,
            service,
            i18n: I18n::new(&locale),
            devices: Vec::new(),
            selected_source: if cfg.source_device_id.is_empty() {
                None
            } else {
                Some(cfg.source_device_id.clone())
            },
            is_running: false,
            status_text: String::new(),
            status_level: StatusLevel::Idle,
            hotkey_error: None,
            draft_general: cfg.general.clone(),
            initialized: false,
            last_event_seq: 0,
            auto_route_checked: false,
        }
    }

    /// 更新状态文本与级别（所有状态变更都应走这里）。
    ///
    /// 公开出来是为了让 GUI 层的异步任务（例如试听提示音）也能把结果
    /// 回写到同一条状态通道上。
    pub fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.status_text = text.into();
        self.status_level = level;
    }

    pub fn init(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        // 设备列表、运行状态都由常驻服务推送过来，这里先要一份最新状态。
        let _ = self.service.send(&Request::State);
    }

    /// 应用常驻服务推送的状态快照。
    ///
    /// 设备枚举、路由启停都在服务进程里完成，界面只负责呈现。
    pub fn apply_service_state(&mut self, snapshot: StateSnapshot) {
        let previous = std::mem::take(&mut self.devices);
        let devices_changed = previous != snapshot.devices;
        self.devices = snapshot.devices;
        self.is_running = snapshot.running;

        if devices_changed {
            if self.devices.is_empty() {
                let text = self.i18n.t("NoDevices").to_string();
                self.set_status(StatusLevel::Warning, text);
            } else if !previous.is_empty() {
                // 设备热插拔导致的刷新：给用户一个明确反馈，
                // 否则「界面悄悄变了一下」很容易被忽略。
                let text = self.i18n.t("DevicesUpdated").to_string();
                self.set_status(StatusLevel::Idle, text);
            } else if !self.is_running {
                let text = self.i18n.t("StatusReady").to_string();
                self.set_status(StatusLevel::Idle, text);
            }
            self.prune_invalid_selection();
        }

        // 路由事件带序号：同一个事件只在序号变化时处理一次，
        // 避免把上一次的失败提示反复显示出来。
        if snapshot.event_seq != self.last_event_seq {
            self.last_event_seq = snapshot.event_seq;
            match snapshot.router_event {
                Some(RouterEvent::Started) => {
                    self.status_level = StatusLevel::Ok;
                }
                Some(RouterEvent::Restarting) => {
                    let text = self.i18n.t("Restarting").to_string();
                    self.set_status(StatusLevel::Warning, text.clone());
                    log::info!("Router: {text}");
                }
                Some(RouterEvent::Restarted) => {
                    let text = self.i18n.t("Restarted").to_string();
                    self.set_status(StatusLevel::Ok, text.clone());
                    log::info!("Router: {text}");
                }
                Some(RouterEvent::Failed) => {
                    let msg = snapshot.message.clone().unwrap_or_default();
                    let text = self.i18n.t("RoutingFailed").replace("{error}", &msg);
                    self.set_status(StatusLevel::Error, text);
                    log::error!("Router failed: {msg}");
                }
                None => {}
            }
        }

        // 全局热键在服务进程里注册：被别的程序占用时把原因显示出来。
        if let Some(err) = snapshot.hotkey_error.as_ref() {
            log::warn!("全局热键注册失败: {err}");
            let text = self.i18n.t("HotkeyInUse").to_string();
            self.hotkey_error = Some(text.clone());
            self.set_status(StatusLevel::Error, text);
        } else {
            self.hotkey_error = None;
        }

        // 第一次拿到状态时，如果配置里开了「开机自动路由」而服务还没开始，
        // 补一次启动请求。
        if !self.auto_route_checked {
            self.auto_route_checked = true;
            self.start_auto_route_if_enabled();
        }
    }

    pub fn select_source_device(&mut self, device_id: String) {
        self.selected_source = Some(device_id);
        self.save_routing_config();
        self.apply_running_config();
    }

    /// 当前 Start 按钮应被禁用的原因；返回 `None` 表示可以启动。
    ///
    /// GUI 用它提前禁用按钮并给出具体提示，而不是让用户点了才发现失败。
    pub fn start_block(&self) -> Option<StartBlock> {
        if self.devices.is_empty() {
            return Some(StartBlock::NoDevices);
        }
        match self.selected_source.as_deref() {
            Some(id) if !id.is_empty() => {}
            _ => return Some(StartBlock::NoSource),
        }
        if self.enabled_output_count() == 0 {
            return Some(StartBlock::NoOutput);
        }
        None
    }

    /// 当前已勾选（且仍然存在、不是源设备）的输出设备 id。
    pub fn enabled_output_ids(&self) -> Vec<String> {
        let handle = self.config_manager.handle();
        let cfg = handle.read();
        let source_id = self.selected_source.as_deref().unwrap_or_default();
        self.devices
            .iter()
            .filter(|d| d.id != source_id)
            .filter(|d| cfg.outputs.iter().any(|o| o.device_id == d.id && o.enabled))
            .map(|d| d.id.clone())
            .collect()
    }

    /// 已勾选（且仍然存在、不是源设备）的输出设备数量。
    ///
    /// 渲染路径只需要「有几个」，不需要 id 列表。这里不做任何堆分配，
    /// 也不深拷贝整份配置——界面每帧都会调用。
    pub fn enabled_output_count(&self) -> usize {
        let handle = self.config_manager.handle();
        let cfg = handle.read();
        let source_id = self.selected_source.as_deref().unwrap_or_default();
        self.devices
            .iter()
            .filter(|d| d.id != source_id)
            .filter(|d| cfg.outputs.iter().any(|o| o.device_id == d.id && o.enabled))
            .count()
    }

    /// 一次性勾选/取消勾选所有输出设备。
    pub fn set_all_outputs_enabled(&mut self, enabled: bool) {
        let target_ids: Vec<String> = self
            .filtered_target_devices()
            .into_iter()
            .map(|d| d.id.clone())
            .collect();
        if target_ids.is_empty() {
            return;
        }
        if let Err(e) = self.config_manager.update(|cfg| {
            for id in &target_ids {
                match cfg.outputs.iter_mut().find(|o| &o.device_id == id) {
                    Some(output) => output.enabled = enabled,
                    None => cfg.outputs.push(Output {
                        device_id: id.clone(),
                        enabled,
                        channel_mode: Some(ChannelMode::Stereo.as_config_str().to_string()),
                    }),
                }
            }
        }) {
            log::error!("Save output enabled state failed: {e}");
            return;
        }
        self.apply_running_config();
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
            return;
        }
        self.apply_running_config();
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
            return;
        }
        self.apply_running_config();
    }

    pub fn start_routing(&mut self) {
        // 先在本地校验配置，给出即时的错误反馈；真正的启动由服务执行，
        // 它会按 settings.toml 里的配置重新读取一遍。
        let router_cfg = match self.build_router_config() {
            Some(cfg) => cfg,
            None => return,
        };
        let running_count = router_cfg.targets.len();

        let starting = self.i18n.t("Starting").to_string();
        self.set_status(StatusLevel::Idle, starting);
        match self.service.send(&Request::Start) {
            Ok(()) => {
                let text = self
                    .i18n
                    .t("RunningOn")
                    .replace("{count}", &running_count.to_string());
                self.set_status(StatusLevel::Ok, text);
            }
            Err(e) => {
                self.is_running = false;
                let text = self
                    .i18n
                    .t("ErrorStartRouting")
                    .replace("{error}", &e.to_string());
                self.set_status(StatusLevel::Error, text);
                log::error!("Start routing failed: {e}");
            }
        }
    }

    pub fn stop_routing(&mut self) {
        let stopping = self.i18n.t("Stopping").to_string();
        self.set_status(StatusLevel::Idle, stopping);
        match self.service.send(&Request::Stop) {
            Ok(()) => {
                self.is_running = false;
                let text = self.i18n.t("StatusReady").to_string();
                self.set_status(StatusLevel::Idle, text);
            }
            Err(e) => {
                let text = self
                    .i18n
                    .t("ErrorStopRouting")
                    .replace("{error}", &e.to_string());
                self.set_status(StatusLevel::Error, text);
                log::error!("Stop routing failed: {e}");
            }
        }
    }

    /// 设置页「保存」：把草稿写入配置并返回语言是否发生变化。
    ///
    /// 返回 `Err` 表示输入不合法（目前只有全局热键），此时配置不会被修改，
    /// 调用方应把错误显示在设置页而不是离开页面。
    pub fn save_general_config(&mut self) -> Result<Option<String>, String> {
        if let Err(e) = crate::hotkey::parse(&self.draft_general.global_hotkey)
            && !self.draft_general.global_hotkey.trim().is_empty()
        {
            return Err(e.to_string());
        }

        let new_language = self.draft_general.language.clone();

        if let Err(e) = self.config_manager.update(|cfg| {
            cfg.general = self.draft_general.clone();
        }) {
            log::error!("Save general config failed: {e}");
            return Err(e.to_string());
        }

        if let Err(e) = crate::autostart::set_autostart(self.draft_general.start_with_windows) {
            let text = self
                .i18n
                .t("ErrorSetAutostart")
                .replace("{error}", &e.to_string());
            self.set_status(StatusLevel::Error, text);
            log::error!("Set autostart failed: {e}");
            return Err(e.to_string());
        }

        // 热键的注册与注销由常驻服务负责：设置落盘后立刻通知它，
        // 让新组合键立即生效（被占用时服务会把错误推回来）。
        let hotkey = self.draft_general.global_hotkey.clone();
        if let Err(e) = self.service.send(&Request::SetHotkey { value: hotkey }) {
            log::error!("通知服务更新全局热键失败: {e}");
        }
        // 语言变了要刷新托盘菜单（托盘在服务进程里）。
        if let Err(e) = self.service.send(&Request::RefreshTray) {
            log::error!("通知服务刷新托盘失败: {e}");
        }

        if new_language != self.i18n.locale() {
            self.i18n.set_locale(&new_language);
            return Ok(Some(new_language));
        }

        Ok(None)
    }

    /// 设置页草稿与已保存配置是否存在差异。
    ///
    /// 有差异说明用户改了设置但还没点「保存」，GUI 据此给出提示，
    /// 避免改了一半切走就丢了。
    pub fn settings_dirty(&self) -> bool {
        let saved = self.config_manager.handle().read().general.clone();
        saved != self.draft_general
    }

    pub fn begin_settings_edit(&mut self) {
        let cfg = self.config_manager.handle().read().clone();
        self.draft_general = cfg.general;
    }

    pub fn nav_pane_expanded(&self) -> bool {
        self.config_manager
            .handle()
            .read()
            .general
            .nav_pane_expanded
    }

    pub fn set_nav_pane_expanded(&mut self, expanded: bool) {
        if let Err(e) = self.config_manager.update(|cfg| {
            cfg.general.nav_pane_expanded = expanded;
        }) {
            log::error!("Save nav pane expanded failed: {e}");
        }
    }

    pub fn backdrop(&self) -> config::config::Backdrop {
        self.config_manager.handle().read().general.backdrop
    }

    pub fn close_to_tray(&self) -> bool {
        self.config_manager.handle().read().general.close_to_tray
    }

    /// 配置中保存的全局热键字符串（可能为空 = 不注册）。
    pub fn global_hotkey(&self) -> String {
        self.config_manager
            .handle()
            .read()
            .general
            .global_hotkey
            .clone()
    }

    pub fn start_minimized(&self) -> bool {
        self.config_manager.handle().read().general.minimized
    }

    pub fn save_routing_config(&mut self) {
        let source_id = self.selected_source.clone().unwrap_or_default();
        // 配置只读一次：原来在 map 闭包里每个设备都克隆一份整份配置。
        let handle = self.config_manager.handle();
        let cfg = handle.read();
        let outputs: Vec<Output> = self
            .devices
            .iter()
            .filter(|d| d.id != source_id)
            .map(|d| {
                let existing = cfg.outputs.iter().find(|o| o.device_id == d.id);
                Output {
                    device_id: d.id.clone(),
                    enabled: existing.map(|o| o.enabled).unwrap_or(false),
                    channel_mode: existing.and_then(|o| o.channel_mode.clone()),
                }
            })
            .collect();
        drop(cfg);

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

    fn build_router_config(&mut self) -> Option<RouterConfig> {
        let source_id = match &self.selected_source {
            Some(id) if !id.is_empty() => id.clone(),
            _ => {
                let text = self.i18n.t("SelectSourceDevice").to_string();
                self.set_status(StatusLevel::Warning, text);
                return None;
            }
        };

        let handle = self.config_manager.handle();
        let cfg = handle.read();
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
            let text = self.i18n.t("SelectDevice").to_string();
            self.set_status(StatusLevel::Warning, text);
            return None;
        }

        Some(RouterConfig {
            source_device_id: Some(source_id),
            targets,
        })
    }

    fn apply_running_config(&mut self) {
        if !self.is_running {
            return;
        }
        // 配置已经落盘，服务会按新配置把路由重启一遍（先停再起）。
        if let Err(e) = self.service.send(&Request::Start) {
            log::error!("Apply running config failed: {e}");
            let text = self
                .i18n
                .t("ErrorStopPrevious")
                .replace("{error}", &e.to_string());
            self.set_status(StatusLevel::Error, text);
        }
    }

    fn prune_invalid_selection(&mut self) {
        let Some(selected) = self.selected_source.as_deref() else {
            return;
        };
        if !self.devices.iter().any(|device| device.id == selected) {
            self.selected_source = self.devices.first().map(|device| device.id.clone());
            self.save_routing_config();
        }
    }

    fn start_auto_route_if_enabled(&mut self) {
        if self.is_running {
            return;
        }
        let handle = self.config_manager.handle();
        let cfg = handle.read().clone();
        if !cfg.general.auto_route || cfg.source_device_id.is_empty() {
            return;
        }
        if !cfg.outputs.iter().any(|o| o.enabled) {
            return;
        }
        // 真正启动由服务执行，它会自己读 settings.toml。
        if let Err(e) = self.service.send(&Request::Start) {
            log::error!("Auto route failed: {e}");
        }
    }
}
