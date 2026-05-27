//! 设备卡片组件

use audio_core::com_service::device::DeviceInfo;
use config::config::Output;
use egui::{Color32, CornerRadius, Stroke};

use crate::app::AudioRouterApp;

/// 渲染单个设备卡片
pub fn show(ui: &mut egui::Ui, app: &mut AudioRouterApp, device: &DeviceInfo) {
    let cfg = app.config_manager.handle().read().clone();
    let output = cfg.outputs.iter().find(|o| o.device_id == device.id);
    let enabled = output.map(|o| o.enabled).unwrap_or(false);

    let alpha: f32 = if enabled { 1.0 } else { 0.5 };
    let text_color = Color32::from_rgba_premultiplied(234, 234, 234, (alpha * 255.0) as u8);
    let muted_color = Color32::from_rgba_premultiplied(140, 140, 140, (alpha * 255.0) as u8);

    egui::Frame::new()
        .fill(Color32::from_rgba_premultiplied(
            17,
            24,
            35,
            (alpha * 255.0) as u8,
        ))
        .corner_radius(CornerRadius::same(12))
        .stroke(Stroke::new(
            1.0,
            Color32::from_rgba_premultiplied(255, 255, 255, (alpha * 12.0) as u8),
        ))
        .show(ui, |ui| {
            ui.set_min_height(48.0);
            ui.horizontal(|ui| {
                let mut new_enabled = enabled;
                let cb = ui.add(egui::Checkbox::new(&mut new_enabled, ""));
                if cb.clicked() {
                    let device_id = device.id.clone();
                    let _ = app.config_manager.update(|cfg| {
                        if let Some(o) = cfg.outputs.iter_mut().find(|o| o.device_id == device_id) {
                            o.enabled = new_enabled;
                        } else {
                            cfg.outputs.push(Output {
                                device_id,
                                enabled: new_enabled,
                                channel_mode: Some("Stereo".to_string()),
                            });
                        }
                    });
                }

                ui.add_space(12.0);

                // 设备名称和 ID
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&device.friendly_name)
                            .color(text_color)
                            .size(14.0),
                    );
                    ui.label(
                        egui::RichText::new(&device.id)
                            .color(muted_color)
                            .size(11.0),
                    );
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |_ui| {});
            });
        });

    ui.add_space(8.0);
}
