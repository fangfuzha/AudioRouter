//! AudioRouter 公共业务逻辑层，与具体 GUI 框架无关。

pub mod controller;
pub mod i18n;
pub mod update;

#[cfg(target_os = "windows")]
pub mod autostart;
