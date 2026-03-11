//! Router configuration.

use config::ChannelMixMode;
use serde::{Deserialize, Serialize};

/// Routing configuration specifying source and target devices with their mixing modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    /// Optional source device ID; if None, use default device
    pub source_device_id: Option<String>,
    /// Target device IDs with their channel mixing modes
    pub target_config: Vec<(String, ChannelMixMode)>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            source_device_id: None,
            target_config: Vec::new(),
        }
    }
}
