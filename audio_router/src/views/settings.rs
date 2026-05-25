//! 设置弹窗视图

use egui::{Color32, CornerRadius};

use crate::app::AudioRouterApp;

/// 渲染设置弹窗
pub fn show(ctx: &egui::Context, app: &mut AudioRouterApp) {
    let settings_title = app.i18n.t("SettingsTitle");
    let mut open = true;

    egui::Window::new(settings_title)
        .id(egui::Id::new("settings_window"))
        .open(&mut open)
        .resizable(false)
        .fixed_size(egui::Vec2::new(320.0, 340.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.add_space(16.0);

            ui.vertical(|ui| {
                setting_checkbox(ui, app.i18n.t("StartWithWindows"), &mut app.draft_general.start_with_windows);
                ui.add_space(12.0);
                setting_checkbox(ui, app.i18n.t("StartMinimized"), &mut app.draft_general.minimized);
                ui.add_space(12.0);
                setting_checkbox(ui, app.i18n.t("AutoRoute"), &mut app.draft_general.auto_route);
                ui.add_space(12.0);

                // 语言选择
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(app.i18n.t("Language"))
                            .size(14.0)
                            .color(Color32::from_rgb(234, 234, 234)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let langs = crate::i18n::I18n::available_languages();
                        let current_lang = app.draft_general.language.clone();
                        let current_name = langs
                            .iter()
                            .find(|(code, _)| *code == current_lang)
                            .map(|(_, name)| *name)
                            .unwrap_or("English");

                        egui::ComboBox::new("settings_language", "")
                            .selected_text(current_name)
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                for (code, name) in &langs {
                                    if ui.selectable_label(current_lang == *code, *name).clicked() {
                                        app.draft_general.language = code.to_string();
                                    }
                                }
                            });
                    });
                });
            });

            ui.add_space(20.0);

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(app.i18n.t("Cancel")).color(Color32::from_rgb(140, 140, 140)))
                                .fill(Color32::from_rgb(17, 24, 35))
                                .corner_radius(CornerRadius::same(12))
                                .min_size(egui::Vec2::new(100.0, 40.0)),
                        )
                        .clicked()
                    {
                        app.show_settings = false;
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(app.i18n.t("Save")).color(Color32::from_rgb(11, 15, 20)).strong())
                                .fill(Color32::from_rgb(43, 217, 127))
                                .corner_radius(CornerRadius::same(12))
                                .min_size(egui::Vec2::new(100.0, 40.0)),
                        )
                        .clicked()
                    {
                        app.save_general_config();
                    }
                });
            });
        });

    if !open {
        app.show_settings = false;
    }
}

fn setting_checkbox(ui: &mut egui::Ui, label: &str, value: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(14.0).color(Color32::from_rgb(234, 234, 234)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Checkbox::new(value, ""));
        });
    });
}
