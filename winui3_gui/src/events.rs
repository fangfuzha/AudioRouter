//! 后台事件源：把设备热插拔、路由状态、托盘交互转成 UI 重绘请求。
//!
//! 早期实现用一个 700ms 的 `DispatcherTimer` 轮询所有状态，既有最长
//! 700ms 的延迟，又让 UI 每 700ms 无条件重绘一次。现在改为事件驱动：
//!
//! - 设备热插拔：COM `IMMNotificationClient`（audio_core::device_watcher）
//! - 路由状态：audio_core::Router::subscribe() 提供的订阅通道
//! - 托盘交互：tray-icon / muda 的全局事件通道
//!
//! 后台线程只做两件事：投递事件、请求重绘。所有真正的 UI 操作
//! （刷新设备列表、执行托盘命令）仍然在 UI 线程的 render 中完成，
//! 避免跨线程操作 Win32 窗口。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use app_core::controller::AppController;
use audio_core::device_watcher::DeviceWatcher;
use parking_lot::Mutex;
use windows_reactor::AsyncSetState;

use crate::tray::{command_from_menu_event, command_from_tray_event, TrayCommand};

/// 后台线程投递给 UI 线程的待处理事件。
#[derive(Default)]
pub struct PendingEvents {
    /// 路由 worker 产生了新事件，需要同步运行状态。
    pub router_dirty: bool,
    /// 托盘命令（必须在 UI 线程执行）。
    pub tray_commands: Vec<TrayCommand>,
}

/// 请求 UI 重绘的句柄（可跨线程克隆）。
///
/// windows-reactor 的 `AsyncSetState` 在写入相同值时会跳过更新，
/// 所以每次请求都写入一个新的计数值，保证重绘一定被触发。
#[derive(Clone)]
pub struct RenderRequest {
    counter: Arc<AtomicU64>,
    setter: AsyncSetState<u64>,
}

impl RenderRequest {
    /// 创建重绘请求句柄。
    ///
    /// `counter` 与 `setter` 来自同一个组件的状态槽，保证计数单调递增。
    pub fn new(counter: Arc<AtomicU64>, setter: AsyncSetState<u64>) -> Self {
        Self { counter, setter }
    }

    /// 请求一次 UI 重绘（可从任意线程调用）。
    pub fn request(&self) {
        let next = self.counter.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        self.setter.call(next);
    }
}

/// 启动全部后台事件源。
///
/// `watcher_slot` 用来持有设备 watcher 句柄：一旦句柄被释放，
/// COM 通知回调会注销，设备热插拔就再也收不到了。
pub fn spawn_event_sources(
    controller: Arc<Mutex<AppController>>,
    pending: Arc<Mutex<PendingEvents>>,
    watcher_slot: Arc<Mutex<Option<DeviceWatcher>>>,
    render: RenderRequest,
) {
    spawn_device_watcher(Arc::clone(&controller), watcher_slot, render.clone());
    spawn_router_watcher(controller, Arc::clone(&pending), render.clone());
    spawn_tray_watcher(pending, render);
}

/// 设备热插拔监听。
///
/// 设备枚举需要 COM 多线程公寓（MTA），而 WinUI 的 UI 线程是单线程公寓
/// （STA），在 UI 线程上枚举设备会得到 RPC_E_CHANGED_MODE(0x80010106)。
/// 因此枚举放在这个后台线程里做：该线程没有预初始化 COM，
/// `ComApartment::mta()` 可以正常生效。
fn spawn_device_watcher(
    controller: Arc<Mutex<AppController>>,
    watcher_slot: Arc<Mutex<Option<DeviceWatcher>>>,
    render: RenderRequest,
) {
    let (watcher, rx) = match DeviceWatcher::start() {
        Ok(pair) => pair,
        Err(e) => {
            log::warn!("Failed to start device watcher, device list will not auto-refresh: {e}");
            return;
        }
    };

    // 句柄必须一直存活，否则 COM 通知会被注销。
    *watcher_slot.lock() = Some(watcher);

    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            {
                let mut c = controller.lock();
                c.refresh_devices();
            }
            render.request();
        }
    });
}

/// 路由状态监听。
fn spawn_router_watcher(
    controller: Arc<Mutex<AppController>>,
    pending: Arc<Mutex<PendingEvents>>,
    render: RenderRequest,
) {
    let rx = {
        let c = controller.lock();
        c.router.subscribe()
    };

    std::thread::spawn(move || {
        while rx.recv().is_ok() {
            {
                let mut p = pending.lock();
                p.router_dirty = true;
            }
            render.request();
        }
    });
}

/// 托盘交互监听（图标点击 + 菜单项）。
fn spawn_tray_watcher(pending: Arc<Mutex<PendingEvents>>, render: RenderRequest) {
    let Some(menu_ids) = crate::tray::menu_item_ids() else {
        log::warn!("Tray menu ids unavailable, tray menu events are ignored");
        return;
    };

    let tray_rx = crate::tray::tray_event_receiver();
    let menu_rx = crate::tray::menu_event_receiver();
    let pending_tray = Arc::clone(&pending);
    let render_tray = render.clone();

    std::thread::spawn(move || {
        while let Ok(event) = tray_rx.recv() {
            if let Some(cmd) = command_from_tray_event(&event) {
                {
                    let mut p = pending_tray.lock();
                    p.tray_commands.push(cmd);
                }
                render_tray.request();
            }
        }
    });

    std::thread::spawn(move || {
        while let Ok(event) = menu_rx.recv() {
            if let Some(cmd) = command_from_menu_event(&event, &menu_ids) {
                {
                    let mut p = pending.lock();
                    p.tray_commands.push(cmd);
                }
                render.request();
            }
        }
    });
}
