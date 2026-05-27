//! Router configuration.

pub use ::config::config::ChannelMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouterConfig {
    pub source_device_id: Option<String>,
    pub targets: Vec<RouterTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterTarget {
    pub device_id: String,
    pub channel_mode: ChannelMode,
}
