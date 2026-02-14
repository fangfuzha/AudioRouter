use anyhow::{Result, anyhow};
use config::ChannelMixMode;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use windows::Win32::Media::Audio::IAudioClient;

use crate::channel_mixer::ChannelMixer;

/// Minimal router configuration (stub)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub source_device_id: Option<String>, // Optional source device ID; if None, use default device
    pub target_config: Vec<(String, ChannelMixMode)>, // Target device IDs with their channel mixing modes
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            source_device_id: None,
            target_config: Vec::new(),
        }
    }
}

/// Router state (in-memory stub implementation)
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

#[derive(Debug)]
struct RouterState {
    running: bool,
    cfg: Option<RouterConfig>,
    // Worker control
    worker_stop: Option<Arc<AtomicBool>>,
    worker_join: Option<std::thread::JoinHandle<()>>,
    // Channel mixers for each output device
    mixers: Vec<ChannelMixer>,
}

impl Default for RouterState {
    fn default() -> Self {
        Self {
            running: false,
            cfg: None,
            worker_stop: None,
            worker_join: None,
            mixers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Router {
    inner: Arc<RwLock<RouterState>>,
}
impl Router {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RouterState::default())),
        }
    }

    /// Start routing based on config and provide a callback to receive captured PCM frames (interleaved f32).
    /// This will spawn a worker thread which runs WASAPI capture+render on Windows.
    pub fn start_with_callback<F>(&self, cfg: RouterConfig, cb: Arc<F>) -> Result<()>
    where
        F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
    {
        {
            let st = self.inner.read();
            if st.running {
                return Err(anyhow!("router already running"));
            }
        }

        // prepare worker control
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_for_worker = stop_flag.clone();
        let cfg_for_worker = cfg.clone();

        // worker thread
        let handle = thread::spawn(move || {
            let res = (|| -> Result<()> {
                #[cfg(target_os = "windows")]
                {
                    use crate::{
                        com_service::router::{
                            get_mix_format, initialize_router, process_next_packet,
                            setup_router_clients,
                        },
                        com_worker::ComSend,
                    };

                    // Extract data before moving cfg_for_worker
                    let target_config = cfg_for_worker.target_config.clone();

                    // 1. Setup clients
                    let setup_res_send = setup_router_clients(cfg_for_worker)?;
                    let setup_res = setup_res_send.clone().take();
                    let source_client = setup_res.source_client.clone();
                    let render_clients: Vec<ComSend<IAudioClient>> = setup_res
                        .output_clients
                        .iter()
                        .map(|(_, c)| ComSend::new(c.clone()))
                        .collect();

                    // 2. Get format
                    let pwf_bytes = get_mix_format(ComSend::new(source_client.clone()))?;

                    // 3. Initialize & Start
                    let init_res = initialize_router(
                        ComSend::new(source_client),
                        render_clients,
                        pwf_bytes.clone(),
                    )?;

                    // Create channel mixers for each output device
                    let mixers: Vec<ChannelMixer> = target_config
                        .iter()
                        .map(|(_, mode)| ChannelMixer::new(*mode))
                        .collect();

                    // 4. Main loop
                    while !stop_for_worker.load(Ordering::SeqCst) {
                        let processed = process_next_packet(
                            init_res.clone(),
                            pwf_bytes.clone(),
                            mixers.clone(),
                            cb.clone(),
                        )?;
                        if !processed {
                            thread::sleep(Duration::from_millis(10));
                        }
                    }

                    // 5. Cleanup
                    crate::com_service::router::finalize_router(setup_res_send)?;

                    Ok(())
                }

                #[cfg(not(target_os = "windows"))]
                {
                    let _ = cfg_for_worker;
                    let _ = cb;
                    Err(anyhow!("WASAPI routing only implemented on Windows"))
                }
            })();

            if let Err(e) = res {
                eprintln!("Router worker error: {:?}", e);
            }
        });

        // Update state
        let mut st = self.inner.write();
        st.cfg = Some(cfg.clone());
        st.running = true;
        st.worker_stop = Some(stop_flag);
        st.worker_join = Some(handle);
        // Initialize channel mixers for each output device
        st.mixers = cfg
            .target_config
            .iter()
            .map(|(_, mode)| ChannelMixer::new(*mode))
            .collect();

        Ok(())
    }

    /// Backward-compatible start() which uses a no-op callback. Prefer `start_with_callback`.
    pub fn start(&self, cfg: RouterConfig) -> Result<()> {
        let noop = Arc::new(|_samples: &[f32], _sr: u32, _ch: u16| {});
        self.start_with_callback(cfg, noop)
    }

    /// Stop routing
    pub fn stop(&self) -> Result<()> {
        let mut st = self.inner.write();
        if !st.running {
            return Err(anyhow!("router not running"));
        }

        // signal worker to stop
        if let Some(flag) = st.worker_stop.take() {
            flag.store(true, Ordering::SeqCst);
        }
        // join thread
        if let Some(handle) = st.worker_join.take() {
            let _ = handle.join();
        }

        st.running = false;
        st.cfg = None;
        st.mixers.clear();
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.inner.read().running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com_service::device::{get_all_output_devices, get_default_output_device};
    use std::time::Duration;
    use tokio::time::sleep;

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn test_clone_default_to_all_outputs() {
        // 1. 获取默认输出设备作为源
        let default_dev = get_default_output_device().expect("failed to get default device");
        println!(
            "Source (Default): {} ({})",
            default_dev.friendly_name, default_dev.id
        );

        // 2. 获取所有输出设备，过滤掉源设备作为目标
        let all_devices = get_all_output_devices().expect("failed to get all devices");
        let target_ids: Vec<String> = all_devices
            .into_iter()
            .filter(|d| d.id != default_dev.id && d.state == crate::DeviceState::Active)
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

        // 3. 配置 Router
        let config = RouterConfig {
            source_device_id: Some(default_dev.id),
            target_config: target_ids
                .into_iter()
                .map(|id| (id, ChannelMixMode::Stereo))
                .collect(),
        };

        let router = Router::new();

        // 4. 定义一个简单的回调来监控是否有数据流过
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

        // 5. 启动路由
        router
            .start_with_callback(config, cb)
            .expect("failed to start router");
        assert!(router.is_running());

        // 6. 运行一段时间 (例如 3 秒)
        // 注意：如果在 CI 环境中没播放声音，data_received 可能仍为 false
        sleep(Duration::from_secs(5)).await;

        // 7. 停止并验证
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
