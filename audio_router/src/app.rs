//! 应用核心状态和 eframe::App 实现。

use std::sync::mpsc;

use audio_core::com_service::device::DeviceInfo;
use audio_core::com_service::device::get_all_output_devices;
use audio_core::router::{ChannelMode, Router, RouterConfig, RouterTarget};
use config::ConfigManager;
use config::config::{General, Output};

use crate::i18n::I18n;
use crate::tray::{AppToTrayCommand, TrayToAppCommand};
use crate::update::UpdateStatus;
use crate::views;

/// eframe 应用主状态
pub struct AudioRouterApp {
    pub config_manager: ConfigManager,
    pub router: Router,
    pub i18n: I18n,
    pub devices: Vec<DeviceInfo>,
    pub selected_source: Option<String>,
    pub is_running: bool,
    pub status_text: String,
    pub window_visible: bool,
    pub show_settings: bool,
    pub app_exit: bool,
    pub request_window_show: bool,
    pub tray_rx: mpsc::Receiver<TrayToAppCommand>,
    pub tray_tx: mpsc::Sender<AppToTrayCommand>,
    pub update_status: UpdateStatus,
    pub draft_general: General,
    pub initialized: bool,
}

impl AudioRouterApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config_manager: ConfigManager,
        router: Router,
        tray_rx: mpsc::Receiver<TrayToAppCommand>,
        tray_tx: mpsc::Sender<AppToTrayCommand>,
        initial_window_visible: bool,
    ) -> Self {
        Self::setup_style(cc);
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
            window_visible: initial_window_visible,
            show_settings: false,
            app_exit: false,
            request_window_show: false,
            tray_rx,
            tray_tx,
            update_status: UpdateStatus::Idle,
            draft_general: cfg.general.clone(),
            initialized: false,
        }
    }

    fn setup_style(cc: &eframe::CreationContext<'_>) {
        use egui::CornerRadius;
        let mut style = (*cc.egui_ctx.style()).clone();
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.window_corner_radius = CornerRadius::same(12);
        v.window_fill = egui::Color32::from_rgb(11, 15, 20);
        v.panel_fill = egui::Color32::from_rgb(11, 15, 20);
        v.faint_bg_color = egui::Color32::from_rgb(14, 20, 29);
        v.extreme_bg_color = egui::Color32::from_rgb(17, 24, 35);
        v.warn_fg_color = egui::Color32::from_rgb(255, 77, 77);
        v.hyperlink_color = egui::Color32::from_rgb(43, 217, 127);
        v.selection.bg_fill = egui::Color32::from_rgba_premultiplied(43, 217, 127, 80);
        v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 217, 127));
        v.override_text_color = Some(egui::Color32::from_rgb(234, 234, 234));

        let cr8 = CornerRadius::same(8);
        v.widgets.noninteractive.corner_radius = cr8;
        v.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(14, 20, 29);
        v.widgets.noninteractive.weak_bg_fill = egui::Color32::from_rgb(17, 24, 35);
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_premultiplied(255, 255, 255, 12),
        );
        v.widgets.noninteractive.fg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_rgb(140, 140, 140));

        v.widgets.inactive.corner_radius = cr8;
        v.widgets.inactive.bg_fill = egui::Color32::from_rgb(14, 20, 29);
        v.widgets.inactive.bg_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_premultiplied(255, 255, 255, 12),
        );

        v.widgets.hovered.corner_radius = cr8;
        v.widgets.hovered.bg_fill = egui::Color32::from_rgb(17, 24, 35);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_premultiplied(43, 217, 127, 80),
        );

        v.widgets.active.corner_radius = cr8;
        v.widgets.active.bg_fill = egui::Color32::from_rgb(20, 28, 40);
        v.widgets.active.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 217, 127));

        cc.egui_ctx.set_style(style);

        // 加载中文字体以解决中文乱码
        crate::fonts::setup_chinese_font(&cc.egui_ctx);
    }

    pub fn init(&mut self, ctx: &egui::Context) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.refresh_devices();
        self.is_running = self.router.is_running();

        if self.devices.is_empty() {
            self.status_text = self.i18n.t("NoDevices").to_string();
        }

        let cfg = self.config_manager.handle().read().clone();
        if cfg.general.auto_route && !cfg.source_device_id.is_empty() {
            let enabled_targets: Vec<RouterTarget> = cfg
                .outputs
                .iter()
                .filter(|o| o.enabled)
                .map(|o| RouterTarget {
                    device_id: o.device_id.clone(),
                    channel_mode: ChannelMode::from_config(o.channel_mode.as_deref()),
                })
                .collect();
            if !enabled_targets.is_empty() {
                let router_cfg = RouterConfig {
                    source_device_id: Some(cfg.source_device_id.clone()),
                    targets: enabled_targets,
                };
                if self.router.start(router_cfg).is_ok() {
                    self.is_running = true;
                }
            }
        }

        // 异步检查更新
        let ctx_clone = ctx.clone();
        std::thread::spawn(move || {
            let status = crate::update::check_for_update();
            ctx_clone.data_mut(|d| {
                d.insert_temp(egui::Id::new("update_status"), status);
            });
            ctx_clone.request_repaint();
        });
    }

    pub fn refresh_devices(&mut self) {
        match get_all_output_devices() {
            Ok(devices) => {
                self.devices = devices;
            }
            Err(e) => {
                log::error!("Failed to enumerate devices: {e}");
                self.status_text = self.i18n.t("ErrorLoadingDevices").to_string();
            }
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

    pub fn save_general_config(&mut self) {
        let new_language = self.draft_general.language.clone();

        if let Err(e) = self.config_manager.update(|cfg| {
            cfg.general = self.draft_general.clone();
        }) {
            log::error!("Save general config failed: {e}");
            return;
        }

        if let Err(e) = crate::autostart::set_autostart(self.draft_general.start_with_windows) {
            self.status_text = format!("Error: {e}");
            log::error!("Set autostart failed: {e}");
            return;
        }

        if new_language != self.i18n.locale() {
            self.i18n.set_locale(&new_language);
            let _ = self
                .tray_tx
                .send(AppToTrayCommand::UpdateLanguage(new_language));
        }

        self.show_settings = false;
    }

    pub fn handle_tray_commands(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayToAppCommand::ShowWindow => {
                    self.request_window_show = true;
                    ctx.request_repaint();
                }
                TrayToAppCommand::Quit => {
                    self.app_exit = true;
                }
            }
        }
    }

    pub fn filtered_target_devices(&self) -> Vec<&DeviceInfo> {
        let source_id = self.selected_source.as_deref();
        self.devices
            .iter()
            .filter(|d| Some(d.id.as_str()) != source_id)
            .collect()
    }
}

impl eframe::App for AudioRouterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 延迟初始化
        if !self.initialized {
            self.init(ctx);
        }

        // 检查更新状态
        if let Some(status) =
            ctx.data_mut(|d| d.remove_temp::<UpdateStatus>(egui::Id::new("update_status")))
        {
            self.update_status = status;
        }

        // 处理托盘命令
        self.handle_tray_commands(ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.app_exit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.window_visible = false;
            return;
        }

        // 窗口显示控制
        if self.request_window_show {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.request_window_show = false;
            self.window_visible = true;
        }

        // 退出
        if self.app_exit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // 渲染 UI
        views::device_list::show(ctx, self);

        if self.show_settings {
            views::settings::show(ctx, self);
        }
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {}
}
