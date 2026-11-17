// 在 release 模式下隐藏 Windows 控制台窗口；debug 模式保留便于查看日志。
// 隐藏控制台后，env_logger 默认输出到 stderr 不会显示，因此日志改为写文件
// （位于 LOCALAPPDATA\AudioRouter\logs\winui3_gui.log）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! AudioRouter 控制面板。
//!
//! 控制面板只是界面：WinUI 3 框架本身要占 120 MB 以上的常驻内存，而音频
//! 路由只需要 2 MB 左右。因此真正干活的是另一个进程 —— 常驻服务
//! （`audiorouter_service.exe`），它托管路由、托盘图标和全局热键。
//!
//! 控制面板关掉就退出进程，这 120 MB 全部还给系统；托盘图标和路由仍然
//! 在服务进程里跑着，想用的时候从托盘或快捷键再把面板叫出来。

use parking_lot::Mutex;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use app_core::controller::AppController;
use app_core::i18n::detect_system_locale;
use app_core::ipc::{sibling_exe, Request, ServiceClient, SERVICE_EXE_NAME};
use config::ConfigManager;
use windows_reactor::*;

mod app;
mod events;
mod pane_bg_override;
mod update;
mod window_utils;

/// 资源路径解析（托盘、更新包都用得到）。
pub use app_core::paths::resolve_asset_path;

// === 单例检测 ===
//
// 使用 Win32 命名互斥量实现单例：首个实例创建互斥量并保活到进程退出，
// 第二个实例启动时发现同名互斥量已存在，则激活已有窗口后退出。
// 互斥量名使用 "Local\" 前缀，限制在当前用户会话内（多用户/多会话场景各自独立）。

/// 互斥量名称。前缀 `Local\` 限制在当前会话内。
const SINGLE_INSTANCE_MUTEX_NAME: &str = "Local\\AudioRouter-SingleInstance-Mutex";
/// 已有实例的窗口标题，用于 FindWindowW 查找并激活。
const SINGLE_INSTANCE_WINDOW_TITLE: &str = "AudioRouter";

/// 尝试创建单例互斥量。
///
/// 返回 `Some(handle)` 表示这是首个实例，`handle` 是互斥量句柄，
/// 必须保活到进程退出（进程退出时 OS 自动释放，从而允许下一个实例启动）。
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

    // ERROR_ALREADY_EXISTS = 183：表示同名互斥量已由前一个实例创建
    const ERROR_ALREADY_EXISTS: u32 = 183;
    let last_error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    if last_error == ERROR_ALREADY_EXISTS {
        // 已有实例在运行，关闭本进程的句柄并尝试激活已有窗口
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        activate_existing_window();
        return None;
    }

    Some(handle)
}

/// 尝试找到并激活已有实例的主窗口。
///
/// 通过窗口标题查找（WinUI 3 窗口的 HWND 仍可被 FindWindowW 找到）。
/// 如果窗口最小化则先恢复，再 SetForegroundWindow 激活。
fn activate_existing_window() {
    let wide: Vec<u16> = SINGLE_INSTANCE_WINDOW_TITLE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(std::ptr::null(), wide.as_ptr())
    };
    if hwnd.is_null() {
        return;
    }

    // SW_RESTORE = 9：恢复最小化/最大化的窗口；SW_SHOW = 5：显示隐藏窗口
    const SW_RESTORE: i32 = 9;
    const SW_SHOW: i32 = 5;
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(hwnd) } != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindowAsync(hwnd, SW_RESTORE);
        }
    } else if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) } == 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindowAsync(hwnd, SW_SHOW);
        }
    }
    unsafe {
        let focused = windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
        if focused == 0 {
            // 系统前台锁定导致激活失败时，用户会看到「双击了但什么都没发生」。
            // 闪烁任务栏图标给一个视觉提示。
            flash_window_handle(hwnd);
        }
    }
}

/// 让窗口在任务栏闪烁若干次。
fn flash_window_handle(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FlashWindowEx, FLASHWINFO, FLASHW_ALL};

    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd,
        dwFlags: FLASHW_ALL,
        uCount: 6,
        dwTimeout: 0,
    };
    unsafe {
        FlashWindowEx(&info);
    }
}

/// 初始化日志：release 模式下写入文件，debug 模式下输出到 stderr。
///
/// 日志文件位于 `LOCALAPPDATA\AudioRouter\logs\winui3_gui.log`，
/// 启动时检查文件大小，超过阈值则 rotate 到 `.old`（覆盖），
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
        let log_path = log_dir.join("winui3_gui.log");
        let old_path = log_dir.join("winui3_gui.log.old");

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

/// 连接常驻服务。服务没在跑就把它拉起来。
fn connect_service(
    controller_slot: &Arc<OnceLock<Arc<Mutex<AppController>>>>,
    render: events::RenderSignal,
) -> std::result::Result<ServiceClient, String> {
    let callback = events::make_service_callback(Arc::clone(controller_slot), render);

    // 服务可能已经在跑（托盘常驻时）。
    if let Ok(client) = ServiceClient::connect(Duration::from_millis(500), callback.clone()) {
        return Ok(client);
    }

    // 服务没在跑 → 把它拉起来。
    //
    // 服务 exe 必须和 GUI exe 在同一个目录（同目录查找）。开发环境里
    // `cargo run -p winui3_gui` 只会编译 GUI 这一个二进制，服务 exe 要
    // 单独构建，否则这里找不到。
    let exe = sibling_exe(SERVICE_EXE_NAME).ok_or_else(|| {
        format!(
            "找不到常驻服务可执行文件（{SERVICE_EXE_NAME}）。\n\n\
             它必须与当前程序在同一个目录里。\n\
             开发环境请先执行：cargo build -p audiorouter_service"
        )
    })?;
    std::process::Command::new(exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动常驻服务失败: {e}"))?;

    ServiceClient::connect(Duration::from_secs(15), callback).map_err(|e| e.to_string())
}

/// 弹一个错误提示框（服务起不来时用）。
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

fn main() -> windows_reactor::Result<()> {
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

    // 单例检测：如果已有实例在运行，激活已有窗口后退出。
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
        ConfigManager::load_with_default_language(Some(app_local_data_dir), default_language)
            .expect("load config");

    // 连接常驻服务。控制器要等服务连上才建得了，所以先用 OnceLock 占位，
    // 服务推送的消息回来时再填进去。
    let render = events::RenderSignal::default();
    let controller_slot: Arc<OnceLock<Arc<Mutex<AppController>>>> = Arc::new(OnceLock::new());
    let service = match connect_service(&controller_slot, render.clone()) {
        Ok(service) => service,
        Err(e) => {
            log::error!("无法连接常驻服务: {e}");
            show_fatal_error(&format!("无法启动常驻服务。\n\n{e}"));
            return Ok(());
        }
    };

    let controller = Arc::new(Mutex::new(AppController::new(config_manager, service)));
    let _ = controller_slot.set(Arc::clone(&controller));

    // 启动时是否隐藏窗口：
    // - `--minimized`：命令行强制隐藏；
    // - `--show`：常驻服务从托盘把面板叫起来时强制显示（用户是明确点了
    //   图标才叫面板的，配置里的「启动时最小化」在这里不适用）；
    // - 其它：按配置里的「启动时最小化」走。
    let args: Vec<String> = std::env::args().skip(1).collect();
    let force_show = args.iter().any(|arg| arg == "--show");
    let start_hidden = !force_show
        && (args.iter().any(|arg| arg == "--minimized") || controller.lock().start_minimized());
    window_utils::set_start_hidden(start_hidden);

    {
        let mut c = controller.lock();
        c.init();
    }

    // 开机自启指向常驻服务（拆分进程之后，开机只需要路由和托盘常驻，
    // 控制面板用不到）。这里刷新一次注册表项，保证从旧版本升级过来后
    // 自启项指向新的服务 exe。
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

    // 从配置读取初始 backdrop，在窗口创建时直接应用。
    // 必须通过 App::backdrop 在窗口创建阶段设置，而非在组件 use_effect 中
    // 事后调用 set_backdrop——后者依赖的 ROOT_WINDOW 在 UI 首次挂载后才设置，
    // use_effect 执行时机可能早于该设置，导致 backdrop 被静默丢弃。
    let initial_backdrop = {
        let c = controller.lock();
        c.backdrop()
    };
    let reactor_backdrop = match initial_backdrop {
        config::config::Backdrop::Mica => Backdrop::Mica,
        config::config::Backdrop::MicaAlt => Backdrop::MicaAlt,
        config::config::Backdrop::Acrylic => Backdrop::Acrylic,
    };

    let icon_path = resolve_asset_path("assets/icon.ico");

    log::info!("Starting AudioRouter control panel...");
    let mut app = App::new()
        .title("AudioRouter")
        .inner_size(980.0, 720.0)
        .inner_constraints(InnerConstraints {
            min_width: Some(640.0),
            min_height: Some(480.0),
            ..Default::default()
        })
        .backdrop(reactor_backdrop);

    if icon_path.is_file() {
        app = app.icon(icon_path.to_string_lossy().to_string());
    } else {
        log::warn!("Window icon not found: {}", icon_path.display());
    }

    let result = app.run({
        let controller = Arc::clone(&controller);
        let render = render.clone();
        move || app::RootComponent::new(Arc::clone(&controller), render.clone())
    });

    // 控制面板退出了。「关闭到托盘」开启时服务继续常驻（托盘图标还在），
    // 关闭时让服务一起退出，整个应用就没了。
    let keep_service = {
        let c = controller.lock();
        c.close_to_tray()
    };
    if !keep_service {
        let c = controller.lock();
        if let Err(e) = c.service.send(&Request::Quit) {
            log::warn!("通知常驻服务退出失败: {e}");
        }
    }
    log::info!("Control panel exited");

    result
}
