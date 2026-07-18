//! Router worker thread implementation.

use anyhow::Result;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::com_service::router::{
    MixFormat, RouterInitialized, finalize_router, get_mix_format, initialize_router,
    process_next_packet, setup_router_clients,
};

use super::config::RouterConfig;

/// Worker 发送给主线程的事件。
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// 初始化成功，路由已开始
    Started,
    /// 设备 invalidated，正在尝试重启
    Restarting,
    /// 重启成功
    Restarted,
    /// 发生不可恢复错误，路由已停止
    Failed(String),
}

pub fn run_worker<F>(
    cfg: RouterConfig,
    cb: Arc<F>,
    stop_rx: mpsc::Receiver<()>,
    ready_tx: mpsc::Sender<Result<()>>,
    event_tx: mpsc::Sender<WorkerEvent>,
) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    let result = setup_and_run_routing(cfg, cb, stop_rx, ready_tx, event_tx);
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
    event_tx: mpsc::Sender<WorkerEvent>,
) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    let _com = ComApartment::mta()?;

    // 首次初始化
    let (setup_res, mix_format, init_res) = match setup_and_initialize(&cfg) {
        Ok(v) => v,
        Err(e) => {
            let _ = ready_tx.send(Err(anyhow::anyhow!("{e:?}")));
            let _ = event_tx.send(WorkerEvent::Failed(format!("{e:?}")));
            return Err(e);
        }
    };

    // 通知主线程：初始化成功
    let _ = ready_tx.send(Ok(()));
    let _ = event_tx.send(WorkerEvent::Started);

    // 主循环：事件循环 + 自动重启
    let mut current_setup = setup_res;
    let mut current_mix = mix_format;
    let mut current_init = init_res;

    loop {
        let loop_result = event_loop(&current_init, &current_mix, &cb, &stop_rx);

        // 无论 event_loop 返回 Ok 还是 Err，都要 finalize 当前资源
        let _ = finalize_router(&current_setup);

        match loop_result {
            Ok(()) => {
                // 正常停止（收到 stop 信号）
                return Ok(());
            }
            Err(e) => {
                let err_str = format!("{e:?}");

                // 判断是否为设备 invalidated（格式改变/设备移除等）。
                // process_next_packet 中对可恢复错误统一使用 "invalidated" 关键字。
                let is_invalidated = err_str.to_lowercase().contains("invalidated");

                if !is_invalidated {
                    // 不可恢复错误，通知主线程并退出
                    let _ = event_tx.send(WorkerEvent::Failed(err_str));
                    return Err(e);
                }

                // 设备 invalidated：尝试自动重启
                let _ = event_tx.send(WorkerEvent::Restarting);
                log::info!("Device invalidated, attempting to restart routing...");

                // 检查是否收到 stop 信号（避免在停止过程中重启）
                match stop_rx.recv_timeout(Duration::from_millis(0)) {
                    Ok(()) => return Ok(()),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                }

                // 重试初始化，最多尝试 10 次，每次间隔 500ms
                let mut restarted = false;
                for attempt in 1..=10 {
                    // 在重试间隔内检查 stop 信号
                    for _ in 0..10 {
                        match stop_rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(()) => return Ok(()),
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                        }
                    }

                    log::info!("Restart attempt {attempt}/10...");
                    match setup_and_initialize(&cfg) {
                        Ok((new_setup, new_mix, new_init)) => {
                            current_setup = new_setup;
                            current_mix = new_mix;
                            current_init = new_init;
                            restarted = true;
                            log::info!("Routing restarted successfully on attempt {attempt}");
                            let _ = event_tx.send(WorkerEvent::Restarted);
                            break;
                        }
                        Err(restart_err) => {
                            log::warn!(
                                "Restart attempt {attempt} failed: {restart_err:?}"
                            );
                        }
                    }
                }

                if !restarted {
                    let msg = "Failed to restart routing after 10 attempts";
                    let _ = event_tx.send(WorkerEvent::Failed(msg.to_string()));
                    return Err(anyhow::anyhow!("{msg}"));
                }
            }
        }
    }
}

/// 完成 WASAPI 客户端的 setup 和 initialize。
/// 成功返回 (setup_res, mix_format, init_res)，失败返回 Err。
fn setup_and_initialize(
    cfg: &RouterConfig,
) -> Result<(
    crate::com_service::router::RouterSetupResult,
    MixFormat,
    RouterInitialized,
)> {
    let setup_res = setup_router_clients(cfg)?;
    let mix_format = get_mix_format(&setup_res.source_client)?;
    let init_res = initialize_router(
        &setup_res.source_client,
        &setup_res.output_clients,
        &mix_format,
    )?;
    Ok((setup_res, mix_format, init_res))
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
    stop_rx: &mpsc::Receiver<()>,
) -> Result<()>
where
    F: Fn(&[f32], u32, u16) + Send + Sync + 'static,
{
    loop {
        match stop_rx.recv_timeout(Duration::from_millis(3)) {
            Ok(()) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // 持续处理所有可用的音频包，直到没有数据为止。
                // 这样可以及时处理音频，避免缓冲积累和抖动。
                loop {
                    let processed = process_next_packet(init_res, mix_format, cb.clone())?;
                    if !processed {
                        break;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }
    Ok(())
}
