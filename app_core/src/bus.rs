//! 进程内命令总线：UI 线程 ↔ 服务线程。
//!
//! 这一层以前是跨进程的命名管道（`\\.\pipe\AudioRouter-Service-v1`）：控制面板
//! 与常驻服务是两个 exe，各自一个主线程，靠自研的「4 字节长度 + JSON」分帧
//! 协议通信，还要处理连接握手、断开重连、读写串行化死锁等一整套坑。
//!
//! 合并成单进程之后，两个线程之间直接用 mpsc 通道 + 共享快照即可，管道分帧、
//! 连接握手、连接断开重连这些复杂度全部消失。
//!
//! 线程分工（见 `audiorouter` crate）：
//! - **主线程**：托盘图标、全局热键、Win32 消息循环。
//! - **UI 线程**：WinUI 3 窗口（STA 公寓）。
//! - **服务线程**：路由控制、设备枚举、状态汇总。
//!
//! 所有跨线程请求都汇进同一条 `Command` 通道，服务线程是唯一的消费者；
//! 服务线程回推状态则走 `StatePublisher`。

use std::sync::Arc;
use std::sync::mpsc::Sender;

use anyhow::{Result, anyhow};
use audio_core::com_service::device::DeviceInfo;
use audio_core::router::WorkerEvent;
use parking_lot::Mutex;

/// UI 线程发给服务线程的请求。
#[derive(Debug, Clone)]
pub enum Request {
    /// 心跳，用于确认通道仍然可用。
    Ping,
    /// 索取当前完整状态快照。
    State,
    /// 按当前配置启动（已在运行则先停再启）。
    Start,
    /// 停止路由。
    Stop,
    /// 更新全局热键（注册/注销由主线程的托盘模块负责）。
    SetHotkey { value: String },
    /// 重新读取配置并刷新托盘菜单（语言变更后调用）。
    RefreshTray,
    /// 退出整个应用（连同路由一起停掉）。
    Quit,
}

/// 路由 worker 的状态事件。
#[derive(Debug, Clone, Copy)]
pub enum RouterEvent {
    Started,
    Restarting,
    Restarted,
    Failed,
}

/// 服务的完整状态快照。
#[derive(Debug, Clone, Default)]
pub struct StateSnapshot {
    /// 路由是否正在运行。
    pub running: bool,
    /// 当前实际输出到多少个设备。
    pub output_count: usize,
    /// 当前枚举到的输出设备列表。
    pub devices: Vec<DeviceInfo>,
    /// 最近一次路由事件。
    pub router_event: Option<RouterEvent>,
    /// 路由事件序号：每次事件递增，UI 据此判断是不是新事件。
    pub event_seq: u64,
    /// 最近一次错误信息。
    pub message: Option<String>,
    /// 全局热键注册失败时的原因。
    pub hotkey_error: Option<String>,
}

/// 服务线程推给 UI 线程的消息。
#[derive(Debug, Clone)]
pub enum ServerMessage {
    /// 完整状态快照。
    State(Box<StateSnapshot>),
    /// 错误（路由失败、设备枚举失败等）。
    Error { message: String },
}

/// 托盘菜单命令。
#[derive(Clone, Copy, Debug)]
pub enum TrayCommand {
    /// 显示控制面板窗口（已经显示时只聚焦，不隐藏）。
    ShowPanel,
    /// 显示 / 隐藏控制面板窗口。
    TogglePanel,
    /// 启动路由。
    StartRouting,
    /// 停止路由。
    StopRouting,
    /// 退出整个应用。
    Quit,
}

/// 服务线程收到的所有命令。
///
/// UI 请求、托盘菜单、全局热键、设备变化、路由事件都走这一条通道，
/// 服务线程是唯一的消费者——这样所有状态变更都发生在同一个线程上，
/// 不需要为「谁在改状态」操心。
pub enum Command {
    /// UI 线程发来的请求。
    Ui(Request),
    /// 托盘菜单命令。
    Tray(TrayCommand),
    /// 全局热键被按下。
    HotkeyPressed,
    /// 设备列表可能变了。
    DevicesChanged,
    /// 路由 worker 的事件。
    RouterEvent(WorkerEvent),
    /// 全局热键注册结果（`None` 表示成功）。
    HotkeyResult(Option<String>),
}

/// UI 侧持有的服务句柄：把请求丢进服务线程的通道。
#[derive(Clone)]
pub struct ServiceHandle {
    tx: Sender<Command>,
}

impl ServiceHandle {
    pub fn new(tx: Sender<Command>) -> Self {
        Self { tx }
    }

    /// 发一条请求。服务线程已经退出时返回错误。
    pub fn send(&self, req: &Request) -> Result<()> {
        self.tx
            .send(Command::Ui(req.clone()))
            .map_err(|_| anyhow!("服务线程已退出"))
    }
}

/// 状态接收回调：服务线程 → UI 线程。
type StateSink = Arc<dyn Fn(ServerMessage) + Send + Sync>;

/// 服务线程 → UI 线程的状态通道。
///
/// UI 侧注册一个回调（写回控制器 + 请求重绘），服务线程状态变化时调用它。
/// 回调运行在服务线程上，所以它必须自己保证线程安全。
#[derive(Clone, Default)]
pub struct StatePublisher {
    sink: Arc<Mutex<Option<StateSink>>>,
}

impl StatePublisher {
    /// 注册 UI 侧的接收回调。
    pub fn set_sink(&self, sink: StateSink) {
        *self.sink.lock() = Some(sink);
    }

    /// 推送一条消息（没有注册接收方时静默丢弃）。
    pub fn publish(&self, message: ServerMessage) {
        let sink = self.sink.lock().clone();
        if let Some(sink) = sink {
            sink(message);
        }
    }
}
