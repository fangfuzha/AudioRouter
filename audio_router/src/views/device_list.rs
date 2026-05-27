//! 主界面：设备列表视图

use audio_core::com_service::device::DeviceInfo;
use egui::{Color32, CornerRadius, Vec2};

use crate::app::AudioRouterApp;

/// 渲染主界面
pub fn show(ctx: &egui::Context, app: &mut AudioRouterApp) {
    // ========== 顶部工具栏 ==========
    egui::TopBottomPanel::top("toolbar")
        .min_height(40.0)
        .frame(egui::Frame {
            fill: Color32::from_rgb(11, 15, 20),
            inner_margin: egui::Margin::symmetric(16, 8),
            ..Default::default()
        })
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("AudioRouter")
                        .size(14.0)
                        .strong()
                        .color(Color32::from_rgb(234, 234, 234)),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(
                            egui::RichText::new(app.i18n.t("Settings"))
                                .size(12.0)
                                .color(Color32::from_rgb(140, 140, 140)),
                        )
                        .clicked()
                    {
                        app.begin_settings_edit();
                        app.show_settings = true;
                    }
                });
            });
        });

    // ========== 中央内容区 ==========
    egui::CentralPanel::default()
        .frame(egui::Frame {
            fill: Color32::from_rgb(11, 15, 20),
            inner_margin: egui::Margin::symmetric(32, 16),
            ..Default::default()
        })
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("main_scroll")
                .show(ui, |ui| {
                    // --- 源设备选择 ---
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(app.i18n.t("SourceDevice"))
                                .size(10.0)
                                .color(Color32::from_rgb(140, 140, 140))
                                .strong(),
                        );
                        ui.add_space(8.0);

                        let source_text = app
                            .selected_source
                            .as_ref()
                            .and_then(|id| app.devices.iter().find(|d| d.id == *id))
                            .map(|d| d.friendly_name.as_str())
                            .unwrap_or("");

                        let app_devices: Vec<_> = app
                            .devices
                            .iter()
                            .map(|d| (d.id.clone(), d.friendly_name.clone()))
                            .collect();
                        let app_selected_source = app.selected_source.clone();
                        egui::ComboBox::new("source_device", "")
                            .selected_text(
                                egui::RichText::new(source_text)
                                    .color(Color32::from_rgb(234, 234, 234)),
                            )
                            .width(280.0)
                            .show_ui(ui, |ui| {
                                for (id, name) in &app_devices {
                                    if ui
                                        .selectable_label(
                                            app_selected_source.as_deref() == Some(id.as_str()),
                                            name,
                                        )
                                        .clicked()
                                    {
                                        app.select_source_device(id.clone());
                                    }
                                }
                            });
                    });

                    ui.add_space(24.0);

                    // --- 输出设备列表 ---
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(app.i18n.t("OutputDevices"))
                                .size(10.0)
                                .color(Color32::from_rgb(140, 140, 140))
                                .strong(),
                        );
                        ui.add_space(8.0);

                        let filtered: Vec<DeviceInfo> =
                            app.filtered_target_devices().into_iter().cloned().collect();
                        if filtered.is_empty() {
                            ui.label(
                                egui::RichText::new(app.i18n.t("NoDevices"))
                                    .color(Color32::from_rgb(140, 140, 140)),
                            );
                        } else {
                            for device in &filtered {
                                crate::widgets::device_card::show(ui, app, device);
                            }
                        }
                    });
                });
        });

    // ========== 底部状态栏 ==========
    egui::TopBottomPanel::bottom("status_bar")
        .min_height(48.0)
        .frame(egui::Frame {
            fill: Color32::from_rgb(14, 20, 29),
            inner_margin: egui::Margin::symmetric(16, 8),
            ..Default::default()
        })
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&app.status_text)
                        .size(12.0)
                        .color(Color32::from_rgb(140, 140, 140)),
                );

                match &app.update_status {
                    crate::update::UpdateStatus::Available { latest_version, .. } => {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} v{}",
                                app.i18n.t("UpdateAvailable"),
                                latest_version
                            ))
                            .size(12.0)
                            .color(Color32::from_rgb(43, 217, 127)),
                        );
                    }
                    crate::update::UpdateStatus::Checking => {
                        ui.label(
                            egui::RichText::new(app.i18n.t("CheckingUpdate"))
                                .size(12.0)
                                .color(Color32::from_rgb(140, 140, 140)),
                        );
                    }
                    _ => {}
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (btn_text, btn_color) = if app.is_running {
                        (app.i18n.t("Stop"), Color32::from_rgb(255, 77, 77))
                    } else {
                        (app.i18n.t("Start"), Color32::from_rgb(43, 217, 127))
                    };

                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(btn_text)
                                    .size(13.0)
                                    .color(Color32::from_rgb(11, 15, 20))
                                    .strong(),
                            )
                            .fill(btn_color)
                            .corner_radius(CornerRadius::same(12))
                            .min_size(Vec2::new(80.0, 36.0)),
                        )
                        .clicked()
                    {
                        if app.is_running {
                            app.stop_routing();
                        } else {
                            app.start_routing();
                        }
                    }

                    ui.add_space(8.0);

                    if ui
                        .button(
                            egui::RichText::new(app.i18n.t("RefreshDevices"))
                                .size(12.0)
                                .color(Color32::from_rgb(140, 140, 140)),
                        )
                        .clicked()
                    {
                        app.refresh_devices();
                    }
                });
            });
        });
}
