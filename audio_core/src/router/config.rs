//! Router configuration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouterConfig {
    pub source_device_id: Option<String>,
    pub target_device_ids: Vec<String>,
}
