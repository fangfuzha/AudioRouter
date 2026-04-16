//! Router configuration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub source_device_id: Option<String>,
    pub target_device_ids: Vec<String>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            source_device_id: None,
            target_device_ids: Vec::new(),
        }
    }
}
