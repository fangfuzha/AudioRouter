//! GUI 与常驻服务进程之间的 IPC 协议与客户端。
//!
//! 进程拆分的目的：WinUI 3 框架本身要占 120 MB 以上的常驻内存，
//! 而音频路由只需要 2 MB 左右。把两者拆开之后，控制面板（GUI）关掉
//! 退出进程，这 120 MB 全部还给系统，音频路由仍然在服务进程里跑着。
//!
//! 通信走本地命名管道。每一帧的格式是「4 字节小端长度 + UTF-8 JSON」，
//! 用字节流模式自己分帧，避免消息模式在超长帧上丢数据。

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use audio_core::com_service::device::DeviceInfo;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_MODE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe,
};
use windows::core::PCWSTR;

/// 命名管道名。服务进程是单实例的（有名互斥量），所以名字固定即可。
pub const PIPE_NAME: &str = r"\\.\pipe\AudioRouter-Service-v1";

/// 常驻服务进程的可执行文件名（与 GUI 同目录）。
pub const SERVICE_EXE_NAME: &str = "audiorouter_service.exe";

/// 控制面板进程的可执行文件名（与服务同目录）。
pub const GUI_EXE_NAME: &str = "winui3_gui.exe";

/// 管道帧的最大长度（1 MB）。设备列表快照远小于这个上限。
const MAX_FRAME: u32 = 1024 * 1024;

/// 管道读写缓冲区大小。
const PIPE_BUFFER: u32 = 64 * 1024;

/// GUI 发给服务的命令。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// 心跳，用于确认连接仍然可用。
    Ping,
    /// 索取当前完整状态快照。
    State,
    /// 按 settings.toml 里的配置启动（已在运行则先停再启）。
    Start,
    /// 停止路由。
    Stop,
    /// 更新全局热键（注册/注销由服务负责）。
    SetHotkey { value: String },
    /// 重新读取 settings.toml 并刷新托盘（语言变更后调用）。
    RefreshTray,
    /// 退出常驻服务（连同路由一起停掉）。
    Quit,
}

/// 路由 worker 的状态事件。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouterEvent {
    Started,
    Restarting,
    Restarted,
    Failed,
}

/// 服务的完整状态快照。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// 路由是否正在运行。
    pub running: bool,
    /// 当前实际输出到多少个设备。
    pub output_count: usize,
    /// 当前枚举到的输出设备列表。
    pub devices: Vec<DeviceInfo>,
    /// 最近一次路由事件。
    pub router_event: Option<RouterEvent>,
    /// 路由事件序号：每次事件递增，控制面板据此判断是不是新事件。
    pub event_seq: u64,
    /// 最近一次错误信息。
    pub message: Option<String>,
    /// 全局热键注册失败时的原因。
    pub hotkey_error: Option<String>,
}

/// 服务推送给 GUI 的消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerMessage {
    /// 完整状态快照。
    State(Box<StateSnapshot>),
    /// 错误（服务不可用、命令失败等）。
    Error { message: String },
}

/// 管道句柄。读写可能发生在不同线程，因此手动实现 Send/Sync。
struct PipeHandle(HANDLE);

// SAFETY: HANDLE 本身是内核对象句柄，Windows 允许在多个线程间共享
// 同一个命名管道句柄做同步读写；这里不会有两个线程同时写。
unsafe impl Send for PipeHandle {}
unsafe impl Sync for PipeHandle {}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// 读满指定长度。字节流模式下 ReadFile 可能只返回部分数据。
fn read_exact(handle: HANDLE, buf: &mut [u8]) -> Result<()> {
    let mut filled = 0usize;
    while filled < buf.len() {
        let mut read = 0u32;
        unsafe {
            ReadFile(handle, Some(&mut buf[filled..]), Some(&mut read), None)
                .map_err(|e| anyhow!("管道读取失败: {e}"))?;
        }
        if read == 0 {
            bail!("管道已被对端关闭");
        }
        filled += read as usize;
    }
    Ok(())
}

/// 写完指定长度。
fn write_all(handle: HANDLE, buf: &[u8]) -> Result<()> {
    let mut written = 0usize;
    while written < buf.len() {
        let mut n = 0u32;
        unsafe {
            WriteFile(handle, Some(&buf[written..]), Some(&mut n), None)
                .map_err(|e| anyhow!("管道写入失败: {e}"))?;
        }
        if n == 0 {
            bail!("管道写入失败");
        }
        written += n as usize;
    }
    Ok(())
}

/// 写一帧：4 字节小端长度 + 负载。
pub fn write_frame(handle: HANDLE, payload: &[u8]) -> Result<()> {
    let len = u32::try_from(payload.len()).map_err(|_| anyhow!("帧过大"))?;
    write_all(handle, &len.to_le_bytes())?;
    write_all(handle, payload)
}

/// 轮询间隔：等待管道里有新数据时的检查频率。
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// 读一帧，返回负载（不含长度前缀）。
///
/// **不能直接用阻塞读**：同一条管道的读和写会串行化——Windows 对同一个
/// file object 的同步 I/O，一次只允许一个未完成的操作。读线程挂着阻塞读
/// 时，服务端主循环往同一条管道写状态就会一直等那次读返回，而读又在等
/// 客户端的下一帧，双方互等，直接死锁。
///
/// 这里的做法是「先 PeekNamedPipe 看看有没有数据，有才去读」：任何时刻
/// 都不留挂起的读操作，写操作因此不会被拖住。代价是最多一个轮询间隔
/// 的延迟（几毫秒，对本地 UI 完全无感）。
pub fn read_frame(handle: HANDLE) -> Result<Vec<u8>> {
    loop {
        if let Some(frame) = try_read_frame(handle)? {
            return Ok(frame);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 读一帧，最多等待 `timeout`。
///
/// 超时返回 `Ok(None)`，对端关闭返回错误。
pub fn read_frame_timeout(handle: HANDLE, timeout: Duration) -> Result<Option<Vec<u8>>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(frame) = try_read_frame(handle)? {
            return Ok(Some(frame));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 数据到齐时读出一帧，否则返回 `None`（不阻塞）。
///
/// 用 PeekNamedPipe **偷看**长度前缀（不消费），据此判断整帧是否已经
/// 到达：只有整帧都在缓冲区里才真正读走，避免「读了长度、负载还没到」
/// 时把帧拆散。
fn try_read_frame(handle: HANDLE) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    let mut avail = 0u32;
    unsafe {
        PeekNamedPipe(
            handle,
            Some(len_buf.as_mut_ptr() as *mut core::ffi::c_void),
            4,
            None,
            Some(&mut avail),
            None,
        )
        .map_err(|e| anyhow!("查询管道可读字节数失败: {e}"))?;
    }
    if avail < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME {
        bail!("帧过大: {len} 字节");
    }
    // avail 含 4 字节长度前缀。
    let total = 4usize + len as usize;
    if (avail as usize) < total {
        return Ok(None);
    }
    let mut buf = vec![0u8; total];
    read_exact(handle, &mut buf)?;
    Ok(Some(buf[4..].to_vec()))
}

/// 把 Rust 字符串转成以 NUL 结尾的 UTF-16。
fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 服务端管道实例。
pub struct PipeServer {
    handle: PipeHandle,
}

impl PipeServer {
    /// 创建一个命名管道实例（此时还没有客户端连上）。
    pub fn create(name: &str) -> Result<Self> {
        let wide = wide_null(name);
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                NAMED_PIPE_MODE(PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0),
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER,
                PIPE_BUFFER,
                0,
                None,
            )
        };
        if handle.is_invalid() {
            return Err(anyhow!(
                "创建命名管道失败: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self {
            handle: PipeHandle(handle),
        })
    }

    /// 阻塞等待客户端连接。
    pub fn connect(&self) -> Result<()> {
        unsafe {
            match ConnectNamedPipe(self.handle.0, None) {
                Ok(()) => Ok(()),
                // 客户端在 CreateNamedPipe 与 ConnectNamedPipe 之间连上了，
                // 这时也算连接成功。
                Err(e)
                    if e.code()
                        == windows::core::HRESULT::from_win32(
                            windows::Win32::Foundation::ERROR_PIPE_CONNECTED.0,
                        ) =>
                {
                    Ok(())
                }
                Err(e) => Err(anyhow!("等待客户端连接失败: {e}")),
            }
        }
    }

    /// 断开当前客户端，让实例可以接受下一个连接。
    pub fn disconnect(&self) {
        unsafe {
            let _ = DisconnectNamedPipe(self.handle.0);
        }
    }

    /// 复制一份句柄，用于把读和写拆到不同线程。
    pub fn try_clone(&self) -> Result<Self> {
        use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
        use windows::Win32::System::Threading::GetCurrentProcess;

        let process = unsafe { GetCurrentProcess() };
        let mut dup = HANDLE::default();
        unsafe {
            DuplicateHandle(
                process,
                self.handle.0,
                process,
                &mut dup,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
            .map_err(|e| anyhow!("复制管道句柄失败: {e}"))?;
        }
        Ok(Self {
            handle: PipeHandle(dup),
        })
    }

    pub fn handle(&self) -> HANDLE {
        self.handle.0
    }
}

/// 客户端管道连接。
pub struct PipeClient {
    handle: PipeHandle,
}

impl PipeClient {
    /// 连接命名管道。
    pub fn connect(name: &str) -> Result<Self> {
        let wide = wide_null(name);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }?;
        Ok(Self {
            handle: PipeHandle(handle),
        })
    }

    /// 复制一份句柄，用于把读和写拆到不同线程。
    pub fn try_clone(&self) -> Result<Self> {
        use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
        use windows::Win32::System::Threading::GetCurrentProcess;

        let process = unsafe { GetCurrentProcess() };
        let mut dup = HANDLE::default();
        unsafe {
            DuplicateHandle(
                process,
                self.handle.0,
                process,
                &mut dup,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
            .map_err(|e| anyhow!("复制管道句柄失败: {e}"))?;
        }
        Ok(Self {
            handle: PipeHandle(dup),
        })
    }

    pub fn handle(&self) -> HANDLE {
        self.handle.0
    }
}

/// 控制面板侧的服务客户端。
///
/// 写入（发命令）和读取（收推送）分开：读取跑在后台线程上，服务一旦推送
/// 状态就立刻回调 `on_message`，不用轮询，也不用让调用方持着锁去阻塞。
pub struct ServiceClient {
    writer: Mutex<PipeClient>,
}

impl ServiceClient {
    /// 连接常驻服务；`timeout` 内连不上就返回错误。
    ///
    /// `on_message` 在后台读取线程上调用：每次服务推送一条消息就调一次。
    pub fn connect(
        timeout: Duration,
        on_message: Arc<dyn Fn(ServerMessage) + Send + Sync + 'static>,
    ) -> Result<Self> {
        let deadline = Instant::now() + timeout;
        loop {
            match PipeClient::connect(PIPE_NAME) {
                Ok(stream) => return Self::from_stream(stream, on_message),
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(anyhow!("无法连接常驻服务: {e}"));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn from_stream(
        stream: PipeClient,
        on_message: Arc<dyn Fn(ServerMessage) + Send + Sync + 'static>,
    ) -> Result<Self> {
        // 读和写拆到两个线程：读取线程独占一份复制出来的句柄。
        let reader = stream.try_clone()?;

        std::thread::spawn(move || {
            let handle = reader.handle();
            loop {
                match read_frame(handle) {
                    Ok(frame) => match serde_json::from_slice::<ServerMessage>(&frame) {
                        Ok(msg) => on_message(msg),
                        Err(e) => log::warn!("服务发来的消息无法解析: {e}"),
                    },
                    Err(e) => {
                        log::debug!("服务连接已断开: {e}");
                        on_message(ServerMessage::Error {
                            message: format!("服务连接已断开: {e}"),
                        });
                        break;
                    }
                }
            }
        });

        Ok(Self {
            writer: Mutex::new(stream),
        })
    }

    /// 发送一条命令。
    pub fn send(&self, req: &Request) -> Result<()> {
        let payload = serde_json::to_vec(req)?;
        let writer = self.writer.lock();
        write_frame(writer.handle(), &payload)
    }
}

/// 同一目录下另一个可执行文件的路径。
pub fn sibling_exe(name: &str) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join(name);
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}
