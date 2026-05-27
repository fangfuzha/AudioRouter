//! 应用核心状态和 eframe::App 实现。

use std::ops::{Deref, DerefMut};
use std::sync::mpsc;

use audio_core::router::Router;
use config::ConfigManager;

use crate::controller::AppController;
use crate::tray::{AppToTrayCommand, TrayToAppCommand};
use crate::update::UpdateStatus;
use crate::views;

/// eframe 应用主状态
pub struct AudioRouterApp {
    pub controller: AppController,
    pub window_visible: bool,
    pub show_settings: bool,
    pub app_exit: bool,
    pub request_window_show: bool,
    pub tray_rx: mpsc::Receiver<TrayToAppCommand>,
    pub tray_tx: mpsc::Sender<AppToTrayCommand>,
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

        Self {
            controller: AppController::new(config_manager, router),
            window_visible: initial_window_visible,
            show_settings: false,
            app_exit: false,
            request_window_show: false,
            tray_rx,
            tray_tx,
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
        self.controller.init();

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

    pub fn save_general_config(&mut self) {
        if let Some(new_language) = self.controller.save_general_config() {
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
}

impl Deref for AudioRouterApp {
    type Target = AppController;

    fn deref(&self) -> &Self::Target {
        &self.controller
    }
}

impl DerefMut for AudioRouterApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.controller
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
            self.controller.set_update_status(status);
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
