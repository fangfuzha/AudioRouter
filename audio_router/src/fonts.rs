//! 字体加载模块
//!
//! 使用 font-kit 跨平台查询系统字体，找到支持中文的字体并注册到 egui。

use font_kit::handle::Handle;
use font_kit::source::SystemSource;

/// 跨平台的 CJK 字体名称列表（按优先级排序）
const CJK_FONT_NAMES: &[&str] = &[
    // Windows
    "Microsoft YaHei",
    "SimHei",
    "DengXian",
    "Microsoft JhengHei",
    // macOS
    "PingFang SC",
    "PingFang TC",
    "STHeiti",
    "Apple LiGothic",
    // Linux
    "Noto Sans CJK SC",
    "Noto Sans CJK",
    "Noto Sans SC",
    "Source Han Sans SC",
    "Source Han Sans",
    "WenQuanYi Micro Hei",
    "WenQuanYi Zen Hei",
    "Droid Sans Fallback",
    "AR PL UMing CN",
];

/// 使用 font-kit 查找系统中的中文字体并配置 egui
pub fn setup_chinese_font(ctx: &egui::Context) {
    let (name, font_bytes) = match find_cjk_font_bytes() {
        Some(result) => result,
        None => {
            log::warn!("No CJK font found on system, Chinese text may display incorrectly");
            return;
        }
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        format!("{name}").clone(),
        std::sync::Arc::new(egui::FontData::from_owned(font_bytes)),
    );

    // 添加到 Proportional 和 Monospace 族作为后备字体
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push(format!("{name}").clone());

    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push(format!("{name}"));

    ctx.set_fonts(fonts);
    log::info!("CJK font loaded and registered with egui: {name}");
}

/// 遍历 CJK 字体名称列表，查找系统上第一个可用的字体并返回其名称和字节数据
fn find_cjk_font_bytes() -> Option<(&'static str, Vec<u8>)> {
    let source = SystemSource::new();

    for name in CJK_FONT_NAMES {
        match source.select_family_by_name(name) {
            Ok(family) => {
                // 取该字体的第一个字形（常规字重）
                if let Some(handle) = family.fonts().first() {
                    match handle {
                        Handle::Path { path, .. } => match std::fs::read(path) {
                            Ok(data) => {
                                log::info!("Loaded CJK font: {name} ({path:?})");
                                return Some((name, data));
                            }
                            Err(e) => {
                                log::debug!("Found CJK font {name} but failed to read: {e}");
                                continue;
                            }
                        },
                        Handle::Memory { bytes, .. } => {
                            log::info!("Loaded CJK font: {name} (from memory)");
                            return Some((name, bytes.to_vec()));
                        }
                    }
                }
            }
            Err(_) => continue,
        }
    }

    None
}
