// 在 release 模式下隐藏 Windows 控制台窗口；debug 模式保留便于查看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! AudioRouter 常驻服务进程。
//!
//! 拆分进程的动机：WinUI 3 框架本身要占 120 MB 以上的常驻内存，而音频
//! 路由只需要 2 MB 左右。把「界面」和「路由」拆成两个进程之后，控制面板
//! 关掉退出进程，这 120 MB 全部还给系统，托盘与路由仍然在服务进程里跑着。
//!
//! 服务进程不依赖 windows-reactor，因此不会加载 Windows App SDK。

mod desktop;
mod launcher;
mod service;

use anyhow::Result;
use std::sync::mpsc;

/// 服务进程单实例互斥量名。
const SERVICE_MUTEX_NAME: &str = r"Local\AudioRouter-Service-Mutex";

fn main() -> Result<()> {
    init_logger();

    if !acquire_single_instance() {
        log::info!("常驻服务已在运行");
        return Ok(());
    }

    let (cmd_tx, cmd_rx) = mpsc::channel();

    // 桌面线程：托盘图标 + 全局热键 + Win32 消息循环。
    let tx_desktop = cmd_tx.clone();
    std::thread::spawn(move || {
        if let Err(e) = desktop::run(tx_desktop) {
            log::error!("桌面线程退出: {e}");
        }
    });

    // 管道线程：接受控制面板的连接。
    let tx_pipe = cmd_tx.clone();
    std::thread::spawn(move || {
        if let Err(e) = service::run_pipe_server(tx_pipe) {
            log::error!("管道服务线程退出: {e}");
        }
    });

    // 主线程：服务状态机。
    service::run(cmd_tx, cmd_rx)
}

/// 尝试成为服务进程的唯一实例。
fn acquire_single_instance() -> bool {
    let wide: Vec<u16> = SERVICE_MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let handle = unsafe {
        windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, wide.as_ptr())
    };
    if handle.is_null() {
        log::warn!("CreateMutexW 失败，跳过单实例检查");
        return true;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;
    let last_error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    if last_error == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        return false;
    }
    true
}

/// 初始化日志：release 模式写文件，debug 模式输出到 stderr。
fn init_logger() {
    let env = env_logger::Env::default().default_filter_or("info");
    let mut builder = env_logger::Builder::from_env(env);
    builder
        .filter_module("icu_segmenter", log::LevelFilter::Off)
        .format_timestamp_micros()
        .format_module_path(false);

    #[cfg(not(debug_assertions))]
    {
        let log_dir = app_core::paths::app_config_dir().join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("audiorouter_service.log");
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => {
                let writer = std::io::BufWriter::new(file);
                builder.target(env_logger::Target::Pipe(Box::new(writer)));
                builder.init();
                log::info!("Log file: {}", log_path.display());
                return;
            }
            Err(e) => {
                eprintln!("Failed to open log file {}: {e}", log_path.display());
            }
        }
    }

    builder.init();
}
