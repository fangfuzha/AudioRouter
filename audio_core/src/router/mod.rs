//! Audio router module.
//!
//! Handles routing audio from a source device to target devices with configurable
//! channel mixing modes.

mod config;
mod state;
mod worker;

pub use config::RouterConfig;
pub use state::RouterState;

use anyhow::{Result, anyhow};
use parking_lot::RwLock;
use std::sync::{Arc, mpsc};
use std::thread;

/// Main router interface for audio routing operations.
#[derive(Debug, Clone)]
pub struct Router {
    inner: Arc<RwLock<RouterState>>,
}

impl Router {
    /// Creates a new router instance.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RouterState::default())),
        }
    }

    /// Starts routing with a callback to receive captured PCM frames.
    ///
    /// This spawns a worker thread that runs WASAPI capture+render operations.
    ///
    /// # Arguments
    /// * `cfg` - Routing configuration
    /// * `cb` - Callback function receiving PCM frames (samples, sample_rate, channels)
    ///
    /// # Errors
    /// Returns an error if router is already running or if WASAPI setup fails.
    pub fn start_with_callback<F>(&self, cfg: RouterConfig, cb: Arc<F>) -> Result<()>
    where
        F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
    {
        // Check if already running
        {
            let st = self.inner.read();
            if st.running {
                return Err(anyhow!("router already running"));
            }
        }

        // Prepare worker control
        let (stop_tx, stop_rx) = mpsc::channel();
        let cfg_for_worker = cfg.clone();

        // Spawn worker thread with error propagation
        let handle = thread::spawn(move || worker::run_worker(cfg_for_worker, cb, stop_rx));

        // Update internal state
        let mut st = self.inner.write();
        st.cfg = cfg.clone();
        st.running = true;
        st.worker_stop_tx = Some(stop_tx);
        st.worker_join = Some(handle);

        Ok(())
    }

    /// Starts routing with a no-op callback.
    ///
    /// Prefer `start_with_callback` if you need to process the audio frames.
    pub fn start(&self, cfg: RouterConfig) -> Result<()> {
        let noop = Arc::new(|_samples: &[f32], _sr: u32, _ch: u16| {});
        self.start_with_callback(cfg, noop)
    }

    /// Stops the router and waits for the worker thread to exit.
    ///
    /// # Errors
    /// Returns an error if router is not running.
    pub fn stop(&self) -> Result<()> {
        let mut st = self.inner.write();
        if !st.running {
            return Err(anyhow!("router not running"));
        }

        // Signal worker to stop and wait for completion
        if let Some(tx) = st.worker_stop_tx.take() {
            let _ = tx.send(()); // Ignore error if worker already exited
        }

        // Join worker thread and propagate errors
        if let Some(handle) = st.worker_join.take() {
            match handle.join() {
                Ok(Ok(())) => {} // Success
                Ok(Err(e)) => return Err(anyhow!("Worker thread error: {:?}", e)),
                Err(e) => return Err(anyhow!("Worker thread panicked: {:?}", e)),
            }
        }

        st.running = false;
        st.cfg = RouterConfig::default();

        Ok(())
    }

    /// Returns whether the router is currently running.
    pub fn is_running(&self) -> bool {
        self.inner.read().running
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com_service::device::{get_all_output_devices, get_default_output_device};
    use ::config::ChannelMixMode;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_clone_default_to_all_outputs() {
        // 1. Get default output device as source
        let default_dev = get_default_output_device().expect("failed to get default device");
        println!(
            "Source (Default): {} ({})",
            default_dev.friendly_name, default_dev.id
        );

        // 2. Get all output devices, filter out source as targets
        let all_devices = get_all_output_devices().expect("failed to get all devices");
        let target_ids: Vec<String> = all_devices
            .into_iter()
            .filter(|d| {
                d.id != default_dev.id && d.state == crate::com_service::device::DeviceState::Active
            })
            .map(|d| d.id)
            .collect();

        if target_ids.is_empty() {
            println!("No other active output devices found to clone to. Skipping test execution.");
            return;
        }

        println!("Target Devices Count: {}", target_ids.len());
        for tid in &target_ids {
            println!(" - Target ID: {}", tid);
        }

        // 3. Configure Router
        let config = RouterConfig {
            source_device_id: Some(default_dev.id),
            target_config: target_ids
                .into_iter()
                .map(|id| (id, ChannelMixMode::Stereo))
                .collect(),
        };

        let router = Router::new();

        // 4. Define callback to monitor data flow
        let data_received = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let energy_detected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let data_received_cb = data_received.clone();
        let energy_detected_cb = energy_detected.clone();

        let cb = Arc::new(move |samples: &[f32], _sr: u32, _ch: u16| {
            if !samples.is_empty() {
                data_received_cb.store(true, std::sync::atomic::Ordering::SeqCst);
                // Check if any sample is significantly non-zero (simple energy check)
                for &s in samples {
                    if s.abs() > 0.0001 {
                        energy_detected_cb.store(true, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                }
            }
        });

        // 5. Start routing
        router
            .start_with_callback(config, cb)
            .expect("failed to start router");
        assert!(router.is_running());

        // 6. Run for a period (e.g., 5 seconds)
        sleep(Duration::from_secs(5)).await;

        // 7. Stop and verify
        println!("Stopping router...");
        router.stop().expect("failed to stop router");
        assert!(!router.is_running());

        if energy_detected.load(std::sync::atomic::Ordering::SeqCst) {
            println!("Success: Non-silent audio data was captured and processed!");
        } else if data_received.load(std::sync::atomic::Ordering::SeqCst) {
            println!(
                "Notice: Audio packets were received, but they were all SILENT. Please ensure music is playing on the default output device."
            );
        } else {
            println!("Error: No audio packets were received at all.");
        }
    }
}
