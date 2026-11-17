//! 服务线程：路由控制、设备枚举、状态汇总。
//!
//! 这部分以前是独立的常驻服务进程（`audiorouter_service.exe`），现在是一条
//! 独立的服务线程：
//!
//! - **主线程**只跑托盘图标、全局热键和 Win32 消息循环，不会被阻塞；
//! - **服务线程**负责路由的启停、设备枚举和状态汇总。`Router::start()` 最多
//!   要等 5 秒才等到音频 worker 就绪，放在主线程会把托盘卡死，必须分开；
//! - **UI 线程**跑 WinUI 3 窗口（STA 公寓）。

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use anyhow::{bail, Result};
use app_core::bus::{
    Command, Request, RouterEvent, ServerMessage, StatePublisher, StateSnapshot, TrayCommand,
};
use app_core::i18n::I18n;
use audio_core::com_service::device::{get_all_output_devices, DeviceInfo};
use audio_core::device_watcher::DeviceWatcher;
use audio_core::router::{ChannelMode, Router, RouterConfig, RouterTarget, WorkerEvent};
use config::ConfigManager;

use crate::desktop;

/// 设备事件去抖窗口。音量变化之类的属性事件会密集触发。
const DEVICE_DEBOUNCE: Duration = Duration::from_millis(300);

/// 服务线程主体。
pub struct Runtime {
    router: Router,
    running: bool,
    output_count: usize,
    devices: Vec<DeviceInfo>,
    last_event: Option<RouterEvent>,
    /// 路由事件序号，每次事件递增。
    event_seq: u64,
    message: Option<String>,
    hotkey_error: Option<String>,
    publisher: StatePublisher,
    /// 与 UI 线程共享同一份配置句柄：UI 落盘后服务线程立刻能读到最新值，
    /// 不必再自己去读一遍 settings.toml（也就没有「谁先写盘」的竞态）。
    config: ConfigManager,
}

impl Runtime {
    fn new(cmd_tx: Sender<Command>, publisher: StatePublisher, config: ConfigManager) -> Self {
        let router = Router::new();
        // 订阅一次即可：Router 反复 start/stop 时订阅关系保持不变。
        let events = router.subscribe();
        let tx = cmd_tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = events.recv() {
                if tx.send(Command::RouterEvent(ev)).is_err() {
                    break;
                }
            }
        });

        Self {
            router,
            running: false,
            output_count: 0,
            devices: Vec::new(),
            last_event: None,
            event_seq: 0,
            message: None,
            hotkey_error: None,
            publisher,
            config,
        }
    }

    fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            running: self.running,
            output_count: self.output_count,
            devices: self.devices.clone(),
            router_event: self.last_event,
            event_seq: self.event_seq,
            message: self.message.clone(),
            hotkey_error: self.hotkey_error.clone(),
        }
    }

    /// 把当前状态推给 UI 线程。
    fn broadcast(&self) {
        self.publisher
            .publish(ServerMessage::State(Box::new(self.snapshot())));
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Ui(req) => self.handle_request(req),
            Command::Tray(cmd) => self.handle_tray(cmd),
            Command::HotkeyPressed => {
                // 全局热键只负责「打开窗口」，不做切换。
                crate::window_utils::show_window();
            }
            Command::DevicesChanged => self.refresh_devices(),
            Command::RouterEvent(ev) => self.handle_router_event(ev),
            Command::HotkeyResult(err) => {
                self.hotkey_error = err;
                self.broadcast();
            }
        }
    }

    fn handle_request(&mut self, req: Request) {
        match req {
            Request::Ping => {}
            Request::State => self.broadcast(),
            Request::Start => self.start_routing(),
            Request::Stop => self.stop_routing(),
            Request::SetHotkey { value } => {
                desktop::request_set_hotkey(&value);
            }
            Request::RefreshTray => {
                // 语言可能刚改过：重新读配置再刷新托盘菜单。
                self.sync_tray();
            }
            Request::Quit => self.quit(),
        }
    }

    fn handle_tray(&mut self, cmd: TrayCommand) {
        match cmd {
            TrayCommand::ShowPanel => crate::window_utils::show_window(),
            TrayCommand::TogglePanel => crate::window_utils::toggle_window_visibility(),
            TrayCommand::StartRouting => self.start_routing(),
            TrayCommand::StopRouting => self.stop_routing(),
            TrayCommand::Quit => self.quit(),
        }
    }

    fn handle_router_event(&mut self, ev: WorkerEvent) {
        self.event_seq += 1;
        match ev {
            WorkerEvent::Started => {
                self.running = true;
                self.last_event = Some(RouterEvent::Started);
                self.message = None;
            }
            WorkerEvent::Restarting => {
                self.last_event = Some(RouterEvent::Restarting);
            }
            WorkerEvent::Restarted => {
                self.running = true;
                self.last_event = Some(RouterEvent::Restarted);
            }
            WorkerEvent::Failed(msg) => {
                self.running = false;
                self.last_event = Some(RouterEvent::Failed);
                self.message = Some(msg);
            }
        }
        self.sync_tray();
        self.broadcast();
    }

    /// 把当前运行状态同步到托盘。
    fn sync_tray(&self) {
        let locale = self.config.handle().read().general.language.clone();
        let i18n = I18n::new(&locale);
        desktop::request_tray_update(&i18n, self.running, self.output_count);
        // 「关闭到托盘」的开关也在这里同步：窗口过程在 UI 线程上读它，
        // 读的是服务线程从共享配置里取到的最新值。
        crate::window_utils::set_close_to_tray(self.config.handle().read().general.close_to_tray);
    }

    /// 重新枚举输出设备。
    fn refresh_devices(&mut self) {
        match get_all_output_devices() {
            Ok(devices) => {
                if devices == self.devices {
                    return;
                }
                self.devices = devices;
                self.broadcast();
            }
            Err(e) => {
                log::error!("枚举输出设备失败: {e}");
            }
        }
    }

    /// 按配置启动路由。
    fn start_routing(&mut self) {
        if self.running {
            self.stop_routing();
        }
        let cfg = match self.build_router_config() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("启动路由失败: {e}");
                self.message = Some(e.to_string());
                self.broadcast();
                return;
            }
        };
        let count = cfg.targets.len();
        match self.router.start(cfg) {
            Ok(()) => {
                self.running = true;
                self.output_count = count;
                self.last_event = Some(RouterEvent::Started);
                self.message = None;
                log::info!("路由已启动，输出到 {count} 个设备");
                self.sync_tray();
            }
            Err(e) => {
                self.running = false;
                self.message = Some(e.to_string());
                log::error!("启动路由失败: {e}");
            }
        }
        self.broadcast();
    }

    fn stop_routing(&mut self) {
        if !self.running {
            return;
        }
        match self.router.stop() {
            Ok(()) => log::info!("路由已停止"),
            Err(e) => log::error!("停止路由失败: {e}"),
        }
        self.running = false;
        self.output_count = 0;
        self.sync_tray();
        self.broadcast();
    }

    /// 退出整个应用：先停掉路由，再结束进程。
    ///
    /// 整个应用只有一个进程，所以「退出托盘」=「退出应用」：控制面板窗口
    /// 随之消失，不会留下一个连不上服务、只会弹「服务已断开」的孤儿窗口。
    fn quit(&mut self) -> ! {
        self.stop_routing();
        log::info!("应用退出");
        std::process::exit(0);
    }

    /// 按共享配置构造路由配置。
    fn build_router_config(&self) -> Result<RouterConfig> {
        let cfg = self.config.handle().read().clone();

        let source = cfg.source_device_id.clone();
        if source.is_empty() {
            bail!("尚未选择源设备");
        }
        let targets: Vec<RouterTarget> = cfg
            .outputs
            .iter()
            .filter(|o| o.enabled)
            .map(|o| RouterTarget {
                device_id: o.device_id.clone(),
                channel_mode: ChannelMode::from_config(o.channel_mode.as_deref()),
            })
            .collect();
        if targets.is_empty() {
            bail!("请至少勾选一个输出设备");
        }

        Ok(RouterConfig {
            source_device_id: Some(source),
            targets,
        })
    }
}

/// 服务线程入口。
///
/// `cmd_tx` 必须和 `cmd_rx` 是同一条通道的两端：设备监听线程与路由事件
/// 线程都靠它把事件送回服务线程。
pub fn run(
    cmd_tx: Sender<Command>,
    cmd_rx: Receiver<Command>,
    publisher: StatePublisher,
    config: ConfigManager,
) -> Result<()> {
    let tx_devices = cmd_tx.clone();
    let mut svc = Runtime::new(cmd_tx, publisher, config);

    // 设备热插拔监听：枚举需要 COM 多线程公寓，放后台线程做。
    std::thread::spawn(move || device_watch_loop(tx_devices));

    // 开机自启的路由：如果配置里开了 auto_route，直接把路由拉起来。
    // 先把判断结果取出来，别拿着配置读锁去调 start_routing（它自己也要读）。
    let auto_route = {
        let handle = svc.config.handle();
        let cfg = handle.read();
        cfg.general.auto_route && !cfg.source_device_id.is_empty()
    };
    if auto_route {
        svc.start_routing();
    }

    // 首帧之前也要把状态推出去：UI 线程可能是几秒后才起来的（托盘模式），
    // 状态得先攒在控制器里。
    svc.refresh_devices();
    svc.sync_tray();
    svc.broadcast();

    while let Ok(cmd) = cmd_rx.recv() {
        svc.handle(cmd);
    }
    Ok(())
}

/// 设备热插拔监听线程（带尾随去抖）。
fn device_watch_loop(cmd_tx: Sender<Command>) {
    let Ok((watcher, rx)) = DeviceWatcher::start() else {
        log::warn!("设备监听启动失败，热插拔不会自动刷新");
        return;
    };
    // watcher 必须一直存活，否则 COM 通知会被注销。
    let _watcher = watcher;

    while rx.recv().is_ok() {
        // 密集事件（音量变化等）只触发一次重新枚举。
        loop {
            match rx.recv_timeout(DEVICE_DEBOUNCE) {
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(_) => continue,
            }
        }
        // 真正的枚举与状态更新交给服务线程（那边持有路由状态）。
        if cmd_tx.send(Command::DevicesChanged).is_err() {
            return;
        }
    }
}
