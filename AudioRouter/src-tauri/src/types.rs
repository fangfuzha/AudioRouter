use config::ChannelMixMode;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Serialize, Deserialize, Type)]
pub struct DeviceLog {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Type)]
pub struct RoutingParams {
    pub source_id: Option<String>,
    pub targets: Vec<(String, ChannelMixMode)>,
}

#[derive(Debug, Serialize, Type)]
pub struct UiDataTargetDevice {
    pub id: String,
    pub name: String,
    pub mix_mode: ChannelMixMode,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Type)]
pub struct UiData {
    pub source_device: Option<String>,
    pub target_devices: Vec<UiDataTargetDevice>,
    pub is_running: bool,
}
