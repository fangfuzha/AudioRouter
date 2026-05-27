//! Router worker thread implementation.

use anyhow::Result;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::com_service::router::{
    MixFormat, RouterInitialized, finalize_router, get_mix_format, initialize_router,
    process_next_packet, setup_router_clients,
};

use super::config::RouterConfig;

pub fn run_worker<F>(
    cfg: RouterConfig,
    cb: Arc<F>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<Result<()>>,
) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    let result = setup_and_run_routing(cfg, cb, stop_rx, ready_tx);
    if let Err(e) = &result {
        log::error!("Router worker exited with error: {e:?}");
    }
    result
}

fn setup_and_run_routing<F>(
    cfg: RouterConfig,
    cb: Arc<F>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<Result<()>>,
) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    let _com = ComApartment::mta()?;

    let setup_res = match setup_router_clients(&cfg) {
        Ok(v) => v,
        Err(e) => {
            let _ = ready_tx.send(Err(anyhow::anyhow!("{e:?}")));
            return Err(e);
        }
    };

    let mix_format = match get_mix_format(&setup_res.source_client) {
        Ok(v) => v,
        Err(e) => {
            let _ = ready_tx.send(Err(anyhow::anyhow!("{e:?}")));
            return Err(e);
        }
    };

    let init_res = match initialize_router(
        &setup_res.source_client,
        &setup_res.output_clients,
        &mix_format,
    ) {
        Ok(v) => v,
        Err(e) => {
            let _ = finalize_router(&setup_res);
            let _ = ready_tx.send(Err(anyhow::anyhow!("{e:?}")));
            return Err(e);
        }
    };

    let _ = ready_tx.send(Ok(()));

    let result = event_loop(&init_res, &mix_format, &cb, stop_rx);

    let finalize_result = finalize_router(&setup_res);

    result.and(finalize_result)
}

struct ComApartment;

impl ComApartment {
    fn mta() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .map_err(|e| anyhow::anyhow!("CoInitializeEx failed: {e:?}"))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

fn event_loop<F>(
    init_res: &RouterInitialized,
    mix_format: &MixFormat,
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
                let processed = process_next_packet(init_res, mix_format, cb.clone())?;
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
