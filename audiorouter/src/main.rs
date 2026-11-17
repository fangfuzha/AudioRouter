// 在 release 模式下隐藏 Windows 控制台窗口；debug 模式保留便于查看日志。
// 隐藏控制台后，env_logger 默认输出到 stderr 不会显示，因此日志改为写文件
// （位于 LOCALAPPDATA\AudioRouter\logs\AudioRouter.log）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! AudioRouter —— 单进程、多线程的 Windows 音频路由工具。
//!
//! 把音频从一个输出设备同时分发到多个其它输出设备（「一个音源，多个房间」）。
//! 原生 Rust + WinUI 3，没有 WebView，也没有 Node.js。
//!
//! ## 线程分工
//!
//! | 线程 | 职责 |
//! |---|---|
//! | **主线程** | 托盘图标、全局热键、Win32 消息循环 |
//! | **服务线程** | 路由启停、设备枚举、状态汇总 |
//! | **UI 线程** | WinUI 3 控制面板（STA 公寓，按需创建） |
//! | 音频 worker | WASAPI 采集 + 多路渲染（audio_core 内部） |
//!
//! 以前这里是两个 exe（常驻服务 + 控制面板），靠命名管道通信；合并成单进程
//! 之后，管道分帧、连接握手、断开重连那一整套复杂度全部消失，开发时也
//! 只需要一条 cargo run 命令。

use std::sync::mpsc;
use std::sync::Arc;

use anyhow::Result;
use parking_lot::Mutex;

use app_core::bus::{Command, ServiceHandle, StatePublisher};
use app_core::controller::AppController;
use app_core::i18n::{detect_system_locale, I18n};
use config::ConfigManager;

mod app;
mod desktop;
mod events;
mod pane_bg_override;
mod runtime;
mod ui_host;
mod update;
mod window_utils;

/// 资源路径解析（托盘、更新包都用得到）。
pub use app_core::paths::resolve_asset_path;

// === 单例检测 ===
//
// 使用 Win32 命名互斥量实现单例：首个实例创建互斥量并保活到进程退出，
// 第二个实例启动时发现同名互斥量已存在，则让已有实例把控制面板叫起来，
// 自己随后退出。
// 互斥量名带 "Local" 前缀，限制在当前用户会话内（多用户/多会话场景各自独立）。

/// 互斥量名称。
const SINGLE_INSTANCE_MUTEX_NAME: &str = "Local\\AudioRouter-SingleInstance-Mutex";

/// 尝试创建单例互斥量。
///
/// 返回 `Some(handle)` 表示这是首个实例，`handle` 是互斥量句柄，
/// 必须保活到 main 结束，进程退出时 OS 自动释放互斥量。
/// 返回 `None` 表示已有实例在运行，调用方应退出。
fn acquire_single_instance() -> Option<*mut core::ffi::c_void> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(SINGLE_INSTANCE_MUTEX_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // CreateMutexW 返回 HANDLE (*mut c_void)。null 表示创建失败。
    let handle = unsafe {
        windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, wide.as_ptr())
    };
    if handle.is_null() {
        // 互斥量创建失败，无法判断，保守地允许启动
        log::warn!("CreateMutexW failed, skipping single-instance check");
        return Some(core::ptr::null_mut());
    }

    // ERROR_ALAlready_EXISTS = 183：表示同名互斥量已由前一个实例创建
    const ERROR_ALREADY_EXISTS: u32 = 183;
    let last_error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    if last_error == ERROR_ALREADY_EXISTS {
        // 已有实例在运行，关闭本进程的句柄并让已有实例显示控制面板
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        activate_existing_window();
        return None;
    }

    Some(handle)
}

/// 让已有实例把控制面板叫起来。
///
/// 托盘窗口的类名在本应用内唯一（单实例保证只有一个），直接找到它并投递
/// 自定义消息即可——常驻实例收到消息后自己把窗口显示出来，顺带把「窗口
/// 应当可见」的状态也更新对。
fn activate_existing_window() {
    let class: Vec<u16> = desktop::TRAY_WINDOW_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(class.as_ptr(), std::ptr::null())
    };
    if hwnd.is_null() {
        log::warn!("已有实例在运行，但没找到它的托盘窗口");
        return;
    }
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
            hwnd,
            desktop::WM_TRAY_SHOW_PANEL,
            0,
            0,
        );
    }
}

/// 初始化日志：release 模式写入文件，debug 模式输出到 stderr。
///
/// 日志文件位于 LOCALAPPDATA\AudioRouter\logs\AudioRouter.log，
/// 启动时检查文件大小，超过阈值则 rotate 到 .old（覆盖），
/// 避免长时运行的进程把磁盘写满。
fn init_logger() {
    let env = env_logger::Env::default().default_filter_or("info");
    let mut builder = env_logger::Builder::from_env(env);
    builder
        .filter_module("icu_segmenter", log::LevelFilter::Off)
        .format_timestamp_micros()
        .format_module_path(true);

    // 仅 release 模式（windows_subsystem = "windows"）需要重定向到文件；
    // debug 模式下保留 stderr，便于开发时直接查看。
    #[cfg(not(debug_assertions))]
    {
        let log_dir = app_core::paths::app_config_dir().join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("AudioRouter.log");
        let old_path = log_dir.join("AudioRouter.log.old");

        // 启动时滚动：超过 5 MiB 就把当前日志 rotate 成 .old
        const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
        let needs_rotate = match std::fs::metadata(&log_path) {
            Ok(meta) => meta.len() > MAX_LOG_BYTES,
            Err(_) => false,
        };
        if needs_rotate {
            // 静默删除旧的 .old（rotate 失败时只 warn，不影响启动）
            if let Err(e) = std::fs::remove_file(&old_path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("Failed to remove old log file: {e}");
                }
            }
            if let Err(e) = std::fs::rename(&log_path, &old_path) {
                eprintln!("Failed to rotate log file: {e}");
            }
        }

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
                // 文件打开失败时回退到 stderr（release 模式下不可见，但至少不会 panic）
                eprintln!("Failed to open log file {}: {e}", log_path.display());
            }
        }
    }

    builder.init();
}

/// 弹一个错误提示框（启动失败时用）。
///
/// 只有框架依赖部署模式会用到它（引导失败时要弹官方安装界面），
/// 自包含模式没有这个调用点。
#[cfg_attr(not(feature = "framework-dependent"), allow(dead_code))]
fn show_fatal_error(message: &str) {
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "AudioRouter"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
        );
    }
}

fn main() -> Result<()> {
    init_logger();

    // 框架依赖部署模式：把 WindowsAppRuntime 框架包挂到当前进程上。
    // 系统上没装对应版本的运行时时，bootstrapper 会弹出官方的快速安装
    // 界面让用户一键安装；装好后流程继续，界面上不会看到任何运行时文件。
    //
    // 必须放在创建任何 XAML 对象之前，否则框架还没挂上就会失败。
    #[cfg(feature = "framework-dependent")]
    {
        if let Err(e) = windows_reactor::bootstrap() {
            log::error!("WindowsAppRuntime 引导失败: {e}");
            show_fatal_error(&format!(
                "无法启动 Windows App Runtime。\n\n{e}\n\n请安装 Windows App Runtime 后重试。"
            ));
            return Ok(());
        }
        log::info!("WindowsAppRuntime bootstrap 完成（框架依赖模式）");
    }

    // 单实例检测：如果已有实例在运行，让它显示控制面板后自己退出。
    // _mutex_handle 必须保活到 main 结束，进程退出时 OS 自动释放互斥量。
    let _mutex_handle = match acquire_single_instance() {
        Some(handle) => handle,
        None => {
            log::info!("Another instance is already running. Activating it and exiting.");
            return Ok(());
        }
    };

    let app_local_data_dir = app_core::paths::app_config_dir();
    // 首次启动（settings.toml 尚不存在）时用系统 UI 语言生成默认设置；
    // 已有配置文件和用户在设置页手动选择的语言都保持不变。
    let default_language = detect_system_locale();
    let config_manager =
        ConfigManager::load_with_default_language(Some(app_local_data_dir), default_language)?;

    // === 进程内命令总线 ===
    //
    // 一条通道走所有跨线程请求：UI 请求、托盘菜单、全局热键、设备变化、
    // 路由事件。服务线程是唯一的消费者。
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let publisher = StatePublisher::default();

    let controller = Arc::new(Mutex::new(AppController::new(
        config_manager.clone(),
        ServiceHandle::new(cmd_tx.clone()),
    )));

    // 跨线程重绘句柄：服务线程推状态后用它请求一次 UI 重绘。
    let render = events::RenderSignal::default();

    // 服务线程 → UI 的状态回调。
    publisher.set_sink(events::make_service_callback(
        Arc::clone(&controller),
        render.clone(),
    ));

    // UI 线程宿主（按需启动控制面板窗口）。
    let ui_host = Arc::new(ui_host::UiHost::new(
        Arc::clone(&controller),
        render.clone(),
        cmd_tx.clone(),
    ));
    ui_host::install(Arc::clone(&ui_host));

    // 启动时是否显示窗口：
    // - --minimized：命令行强制只跑托盘（开机自启就是走这条路）；
    // - 其它：按配置里的「启动时最小化」走。
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start_hidden =
        args.iter().any(|arg| arg == "--minimized") || controller.lock().start_minimized();
    window_utils::set_start_hidden(start_hidden);

    // 开机自启注册表项：现在只有一个 exe，自启指向它并带 --minimized。
    // 这里刷新一次，保证从旧版本（自启指向 audiorouter_service.exe）升级过来后
    // 注册表项指向新的 exe。
    {
        let enabled = controller
            .lock()
            .config_manager
            .handle()
            .read()
            .general
            .start_with_windows;
        if enabled {
            if let Err(e) = app_core::autostart::set_autostart(true) {
                log::warn!("刷新开机自启注册表项失败: {e}");
            }
        }
    }

    // 初始化系统代理监听：读取当前代理并启动后台线程监听注册表变化，
    // 用户在系统设置中切换 VPN 代理时自动热加载，无需重启应用。
    update::init_proxy_watcher();

    // 清理上次更新残留的安装包（%TEMP%\AudioRouter-Updates\*.exe），
    // 避免多次更新后临时文件累积占用磁盘。
    update::cleanup_old_installers();

    // === 服务线程 ===
    //
    // 路由启停、设备枚举、状态汇总都在这里。它不能放在主线程：
    // Router::start() 最多要等 5 秒才等到音频 worker 就绪，会把托盘卡死。
    {
        let cmd_tx = cmd_tx.clone();
        let publisher = publisher.clone();
        let config = config_manager.clone();
        std::thread::Builder::new()
            .name("audiorouter-runtime".to_string())
            .spawn(move || {
                if let Err(e) = runtime::run(cmd_tx, cmd_rx, publisher, config) {
                    log::error!("服务线程退出: {e}");
                }
            })?;
    }

    // === UI 线程 ===
    //
    // 「启动时最小化」模式下根本不创建窗口：WindowsAppSDK 那 ~120 MB 的
    // 常驻开销也就不会在开机时白白吃掉。等用户点托盘图标时再创建。
    if !start_hidden {
        ui_host.ensure_started();
    }

    // === 主线程：托盘 + 全局热键 + Win32 消息循环 ===
    let (i18n, hotkey_value) = {
        let handle = config_manager.handle();
        let cfg = handle.read();
        (
            I18n::new(&cfg.general.language),
            cfg.general.global_hotkey.clone(),
        )
    };
    desktop::run(cmd_tx, &hotkey_value, &i18n)?;

    // 托盘消息循环退出（正常情况下只有进程退出才会走到这里）。
    log::info!("主线程结束");
    Ok(())
}
