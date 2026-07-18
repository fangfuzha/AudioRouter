//! Router internal state management.

use super::config::RouterConfig;
use super::worker::WorkerEvent;
use std::sync::Mutex;
use std::sync::mpsc;

/// Internal router state tracking.
pub struct RouterState {
    /// Whether the router is currently running.
    pub running: bool,
    /// Current configuration being used.
    pub cfg: RouterConfig,
    /// Channel to signal worker thread to stop.
    pub worker_stop_tx: Option<mpsc::Sender<()>>,
    /// Handle to the worker thread.
    pub worker_join: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    /// Channel to receive events from worker thread (restart/fail).
    /// 用 Mutex 包装使 Receiver 满足 Sync（mpsc::Receiver 本身不是 Sync）。
    pub worker_event_rx: Option<Mutex<mpsc::Receiver<WorkerEvent>>>,
}

impl std::fmt::Debug for RouterState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterState")
            .field("running", &self.running)
            .field("cfg", &self.cfg)
            .field("has_stop_tx", &self.worker_stop_tx.is_some())
            .field("has_join", &self.worker_join.is_some())
            .field("has_event_rx", &self.worker_event_rx.is_some())
            .finish()
    }
}

impl Default for RouterState {
    fn default() -> Self {
        Self {
            running: false,
            cfg: RouterConfig::default(),
            worker_stop_tx: None,
            worker_join: None,
            worker_event_rx: None,
        }
    }
}
