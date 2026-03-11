//! Router worker thread implementation.

use anyhow::Result;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use windows::Win32::Media::Audio::IAudioClient;

use crate::channel_mixer::ChannelMixer;
use crate::com_service::router::{
    RouterInitialized, finalize_router, get_mix_format, initialize_router, process_next_packet,
    setup_router_clients,
};
use crate::utils::ComSend;

use super::config::RouterConfig;

/// Worker thread function that handles audio routing.
pub fn run_worker<F>(cfg: RouterConfig, cb: Arc<F>, stop_rx: mpsc::Receiver<()>) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    setup_and_run_routing(cfg, cb, stop_rx)
}

fn setup_and_run_routing<F>(
    cfg: RouterConfig,
    cb: Arc<F>,
    stop_rx: mpsc::Receiver<()>,
) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    // Extract data before moving cfg
    // 1. Setup clients
    let setup_res_send = setup_router_clients(cfg.clone())?;
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
    let mixers: Vec<ChannelMixer> = cfg
        .target_config
        .clone()
        .iter()
        .map(|(_, mode)| ChannelMixer::new(*mode))
        .collect();

    // 4. Main loop
    let result = event_loop(&init_res, &pwf_bytes, &mixers, &cb, stop_rx);

    // 5. Cleanup
    finalize_router(setup_res_send)?;

    result
}

fn event_loop<F>(
    init_res: &ComSend<RouterInitialized>,
    pwf_bytes: &ComSend<Vec<u8>>,
    mixers: &[ChannelMixer],
    cb: &Arc<F>,
    stop_rx: mpsc::Receiver<()>,
) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    loop {
        // Check for stop signal with timeout
        match stop_rx.recv_timeout(Duration::from_millis(9)) {
            Ok(()) => break, // Received stop signal
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // No stop signal, continue processing
                let processed = process_next_packet(
                    init_res.clone(),
                    pwf_bytes.clone(),
                    mixers.to_vec(),
                    cb.clone(),
                )?;
                if !processed {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Channel disconnected, worker should exit
                break;
            }
        }
    }
    Ok(())
}
