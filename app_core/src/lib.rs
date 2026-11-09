//! AudioRouter 公共业务逻辑层，与具体 GUI 框架无关。

pub mod controller;
pub mod i18n;

#[cfg(target_os = "windows")]
pub mod autostart;
#[cfg(target_os = "windows")]
pub mod window_placement;
