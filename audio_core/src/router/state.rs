//! Router internal state management.

use super::config::RouterConfig;
use std::sync::mpsc;

/// Internal router state tracking.
#[derive(Debug)]
pub struct RouterState {
    /// Whether the router is currently running.
    pub running: bool,
    /// Current configuration being used.
    pub cfg: RouterConfig,
    /// Channel to signal worker thread to stop.
    pub worker_stop_tx: Option<mpsc::Sender<()>>,
    /// Handle to the worker thread.
    pub worker_join: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
}

impl Default for RouterState {
    fn default() -> Self {
        Self {
            running: false,
            cfg: RouterConfig::default(),
            worker_stop_tx: None,
            worker_join: None,
        }
    }
}
