//! 常驻服务进程的核心：状态、路由管理与管道服务端。
//!
//! 服务进程不加载 WinUI 3，只做三件事：
//!
//! 1. 托管音频路由（WASAPI 采集 + 多路渲染）与设备热插拔监听；
//! 2. 通过命名管道接受控制面板（GUI）的命令，并把状态推回去；
//! 3. 维护托盘图标与全局热键（见 `desktop` 模块）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use app_core::ipc::{
    PIPE_NAME, PipeServer, Request, RouterEvent, ServerMessage, StateSnapshot, read_frame,
    write_frame,
};
use app_core::paths::app_config_dir;
use audio_core::com_service::device::{DeviceInfo, get_all_output_devices};
use audio_core::device_watcher::DeviceWatcher;
use audio_core::router::{ChannelMode, Router, RouterConfig, RouterTarget, WorkerEvent};
use config::ConfigManager;

use crate::desktop::TrayCommand;

/// 服务内部收到的命令。
pub enum ServiceCommand {
    /// 控制面板发来的请求（附客户端 id）。
    Request(u64, Request),
    /// 控制面板连上了。
    ClientConnected(u64, PipeServer),
    /// 控制面板断开了。
    ClientDisconnected(u64),
    /// 托盘菜单命令。
    Tray(TrayCommand),
    /// 全局热键被按下。
    HotkeyPressed,
    /// 设备列表可能变了。
    DevicesChanged,
    /// 路由 worker 的事件。
    RouterEvent(WorkerEvent),
    /// 全局热键注册结果（None 表示成功）。
    HotkeyResult(Option<String>),
}

/// 设备事件去抖窗口。音量变化之类的属性事件会密集触发。
const DEVICE_DEBOUNCE: Duration = Duration::from_millis(300);

/// 服务进程主体。
pub struct Service {
    router: Router,
    running: bool,
    output_count: usize,
    devices: Vec<DeviceInfo>,
    last_event: Option<RouterEvent>,
    /// 路由事件序号，每次事件递增。
    event_seq: u64,
    message: Option<String>,
    hotkey_error: Option<String>,
    clients: Vec<(u64, PipeServer)>,
}

impl Service {
    fn new(cmd_tx: Sender<ServiceCommand>) -> Self {
        let router = Router::new();
        // 订阅一次即可：Router 反复 start/stop 时订阅关系保持不变。
        let events = router.subscribe();
        let tx = cmd_tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = events.recv() {
                if tx.send(ServiceCommand::RouterEvent(ev)).is_err() {
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
            clients: Vec::new(),
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

    /// 把当前状态推给所有连着的控制面板。
    fn broadcast(&self) {
        if self.clients.is_empty() {
            return;
        }
        let payload = match serde_json::to_vec(&ServerMessage::State(Box::new(self.snapshot()))) {
            Ok(p) => p,
            Err(e) => {
                log::error!("序列化状态失败: {e}");
                return;
            }
        };
        for (id, client) in &self.clients {
            if let Err(e) = write_frame(client.handle(), &payload) {
                log::debug!("向控制面板 {id} 推送状态失败: {e}");
            }
        }
    }

    fn handle(&mut self, cmd: ServiceCommand) {
        match cmd {
            ServiceCommand::Request(id, req) => self.handle_request(id, req),
            ServiceCommand::ClientConnected(id, writer) => {
                self.clients.push((id, writer));
                self.broadcast();
            }
            ServiceCommand::ClientDisconnected(id) => {
                self.clients.retain(|(cid, _)| *cid != id);
            }
            ServiceCommand::Tray(cmd) => self.handle_tray(cmd),
            ServiceCommand::HotkeyPressed => {
                crate::launcher::show_control_panel();
            }
            ServiceCommand::DevicesChanged => self.refresh_devices(),
            ServiceCommand::RouterEvent(ev) => self.handle_router_event(ev),
            ServiceCommand::HotkeyResult(err) => {
                self.hotkey_error = err;
                self.broadcast();
            }
        }
    }

    fn handle_request(&mut self, id: u64, req: Request) {
        match req {
            Request::Ping => {}
            Request::State => self.broadcast(),
            Request::Start => self.start_routing(),
            Request::Stop => self.stop_routing(),
            Request::SetHotkey { value } => {
                crate::desktop::request_set_hotkey(&value);
            }
            Request::RefreshTray => {
                // 语言可能刚改过：重新读 settings.toml 再刷新托盘菜单。
                self.sync_tray();
            }
            Request::Quit => {
                self.stop_routing();
                log::info!("收到退出指令，常驻服务退出");
                std::process::exit(0);
            }
        }
        let _ = id;
    }

    fn handle_tray(&mut self, cmd: TrayCommand) {
        match cmd {
            TrayCommand::ShowPanel => crate::launcher::show_control_panel(),
            TrayCommand::StartRouting => self.start_routing(),
            TrayCommand::StopRouting => self.stop_routing(),
            TrayCommand::Quit => {
                self.stop_routing();
                log::info!("常驻服务退出");
                std::process::exit(0);
            }
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
        crate::desktop::request_tray_update(
            &crate::desktop::current_i18n(),
            self.running,
            self.output_count,
        );
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

    /// 按 settings.toml 里的配置启动路由。
    fn start_routing(&mut self) {
        if self.running {
            self.stop_routing();
        }
        let cfg = match build_router_config() {
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
}

/// 从 settings.toml 构造路由配置。
fn build_router_config() -> Result<RouterConfig> {
    let dir = app_config_dir();
    let mgr =
        ConfigManager::load_if_exists(Some(dir))?.ok_or_else(|| anyhow!("尚未创建配置文件"))?;
    let handle = mgr.handle();
    let cfg = handle.read().clone();

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

/// 读取 settings.toml 里的全局热键。
pub fn configured_hotkey() -> String {
    let dir = app_config_dir();
    match ConfigManager::load_if_exists(Some(dir)) {
        Ok(Some(mgr)) => {
            let handle = mgr.handle();
            handle.read().general.global_hotkey.clone()
        }
        _ => String::new(),
    }
}

/// 服务主循环。
///
/// `cmd_tx` 必须和 `cmd_rx` 是同一条通道的两端：设备监听线程与路由事件
/// 线程都靠它把事件送回主循环。
pub fn run(cmd_tx: Sender<ServiceCommand>, cmd_rx: Receiver<ServiceCommand>) -> Result<()> {
    let tx_devices = cmd_tx.clone();
    let mut svc = Service::new(cmd_tx);

    // 设备热插拔监听：枚举需要 COM 多线程公寓，放后台线程做。
    std::thread::spawn(move || device_watch_loop(tx_devices));

    // 开机自启的路由：如果配置里开了 auto_route，直接把路由拉起来。
    if auto_route_enabled() {
        svc.start_routing();
    }

    while let Ok(cmd) = cmd_rx.recv() {
        svc.handle(cmd);
    }
    Ok(())
}

/// settings.toml 里是否开启了开机自动路由。
fn auto_route_enabled() -> bool {
    let dir = app_config_dir();
    match ConfigManager::load_if_exists(Some(dir)) {
        Ok(Some(mgr)) => {
            let handle = mgr.handle();
            let cfg = handle.read();
            cfg.general.auto_route && !cfg.source_device_id.is_empty()
        }
        _ => false,
    }
}

/// 设备热插拔监听线程（带尾随去抖）。
fn device_watch_loop(cmd_tx: Sender<ServiceCommand>) {
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
        // 真正的枚举与状态更新交给服务主循环（那边持有路由状态）。
        if cmd_tx.send(ServiceCommand::DevicesChanged).is_err() {
            return;
        }
    }
}

/// 管道服务端线程：接受控制面板连接并把请求转给主循环。
pub fn run_pipe_server(cmd_tx: Sender<ServiceCommand>) -> Result<()> {
    let next_id = AtomicU64::new(1);
    loop {
        let server = match PipeServer::create(PIPE_NAME) {
            Ok(s) => s,
            Err(e) => {
                log::error!("创建命名管道失败: {e}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if let Err(e) = server.connect() {
            log::warn!("等待控制面板连接失败: {e}");
            continue;
        }

        let id = next_id.fetch_add(1, Ordering::Relaxed);
        let Ok(writer) = server.try_clone() else {
            log::error!("复制管道写句柄失败");
            continue;
        };
        let Ok(reader) = server.try_clone() else {
            log::error!("复制管道读句柄失败");
            continue;
        };

        // 先登记客户端，再启动读线程：读线程结束时会发 ClientDisconnected，
        // 必须保证它在 ClientConnected 之后入队，否则主循环会先删后加。
        let _ = cmd_tx.send(ServiceCommand::ClientConnected(id, writer));

        let tx = cmd_tx.clone();
        std::thread::spawn(move || {
            // 持有原始句柄：管道实例在客户端断开之前不能被销毁。
            let server = server;
            loop {
                match read_frame(reader.handle()) {
                    Ok(frame) => match serde_json::from_slice::<Request>(&frame) {
                        Ok(req) => {
                            if tx.send(ServiceCommand::Request(id, req)).is_err() {
                                break;
                            }
                        }
                        Err(e) => log::warn!("控制面板发来的请求无法解析: {e}"),
                    },
                    Err(e) => {
                        log::debug!("控制面板断开连接: {e}");
                        break;
                    }
                }
            }
            // 显式断开：客户端下一次读写能立刻拿到「管道已结束」，
            // 而不是一直等到超时。
            server.disconnect();
            let _ = tx.send(ServiceCommand::ClientDisconnected(id));
        });

        // 不在这里等待客户端断开：立刻准备接受下一个客户端。
        // 面板是单实例的，但「上一个客户端还没断开」不该把新客户端堵住。
    }
}
