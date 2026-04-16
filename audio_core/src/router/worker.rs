//! Router worker thread implementation.

use anyhow::Result;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use windows::Win32::Media::Audio::IAudioClient;

use crate::com_service::router::{
    RouterInitialized, finalize_router, get_mix_format, initialize_router, process_next_packet,
    setup_router_clients,
};
use crate::utils::ComSend;

use super::config::RouterConfig;

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
    let setup_res_send = setup_router_clients(cfg.clone())?;
    let setup_res = setup_res_send.clone().take();
    let source_client = setup_res.source_client.clone();
    let render_clients: Vec<ComSend<IAudioClient>> = setup_res
        .output_clients
        .iter()
        .map(|(_, c)| ComSend::new(c.clone()))
        .collect();

    let pwf_bytes = get_mix_format(ComSend::new(source_client.clone()))?;

    let init_res = initialize_router(
        ComSend::new(source_client),
        render_clients,
        pwf_bytes.clone(),
    )?;

    let result = event_loop(&init_res, &pwf_bytes, &cb, stop_rx);

    finalize_router(setup_res_send)?;

    result
}

fn event_loop<F>(
    init_res: &ComSend<RouterInitialized>,
    pwf_bytes: &ComSend<Vec<u8>>,
    cb: &Arc<F>,
    stop_rx: mpsc::Receiver<()>,
) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    loop {
        match stop_rx.recv_timeout(Duration::from_millis(9)) {
            Ok(()) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let processed = process_next_packet(
                    init_res.clone(),
                    pwf_bytes.clone(),
                    cb.clone(),
                )?;
                if !processed {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }
    Ok(())
}
