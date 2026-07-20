// 在 release 模式下隐藏 Windows 控制台窗口；debug 模式保留便于查看日志。
// 隐藏控制台后，env_logger 默认输出到 stderr 不会显示，因此日志改为写文件
// （位于 LOCALAPPDATA\AudioRouter\logs\winui3_gui.log）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

use app_core::controller::AppController;
use audio_core::router::Router;
use config::ConfigManager;
use windows_reactor::*;

mod app;
mod pane_bg_override;
mod tray;
mod update;
mod window_utils;

fn app_config_dir() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(std::path::PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
        .join("AudioRouter")
}

/// 解析项目资源文件路径，兼容多种运行场景：
///
/// 1. `{exe_dir}/{rel_path}` —— 安装后场景（exe 和 assets 在同一目录）
/// 2. 从 exe 目录逐级向上查找（最多 3 级）—— 覆盖 `target/debug/` → 项目根
/// 3. `CARGO_MANIFEST_DIR/../{rel_path}` —— cargo run 环境变量，最准确
/// 4. 当前工作目录 `./{rel_path}` —— 兜底
///
/// 全部找不到则返回 `{exe_dir}/{rel_path}`（调用方自己判断 exists）。
pub fn resolve_asset_path(rel_path: &str) -> std::path::PathBuf {
    let exe_path =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("winui3_gui.exe"));
    let exe_dir = exe_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();

    let mut candidates = vec![exe_dir.join(rel_path)];

    // 逐级向上查找（最多 3 级，覆盖 target/debug/ → 项目根）
    let mut parent = exe_dir.clone();
    for _ in 0..3 {
        if let Some(p) = parent.parent() {
            parent = p.to_path_buf();
            candidates.push(parent.join(rel_path));
        } else {
            break;
        }
    }

    // CARGO_MANIFEST_DIR 环境变量（cargo run/build 时设置）
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        if let Some(workspace_root) = std::path::Path::new(&manifest_dir).parent() {
            candidates.push(workspace_root.join(rel_path));
        }
    }

    // 当前工作目录兜底
    candidates.push(std::path::PathBuf::from(rel_path));

    for path in &candidates {
        if path.exists() {
            return path.clone();
        }
    }

    // 都找不到，返回第一个候选（exe 旁边），让调用方自行处理
    candidates.remove(0)
}

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
    let handle = unsafe { windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
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
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(SINGLE_INSTANCE_WINDOW_TITLE)
        .encode_wide()
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
    // 窗口被隐藏（close_to_tray → SW_HIDE）时 IsIconic 为 false，
    // 因此需要额外检查 IsWindowVisible；不可见时先 ShowWindow。
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
        windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
    }
}

/// 初始化日志：release 模式下写入文件，debug 模式下输出到 stderr。
///
/// 日志文件位于 `LOCALAPPDATA\AudioRouter\logs\winui3_gui.log`，
/// 每次启动追加写入，便于事后排查问题。
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
        let log_dir = app_config_dir().join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("winui3_gui.log");
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

fn main() -> windows_reactor::Result<()> {
    init_logger();

    // 单例检测：如果已有实例在运行，激活已有窗口后退出。
    // _mutex_handle 必须保活到 main 结束，进程退出时 OS 自动释放互斥量。
    let _mutex_handle = match acquire_single_instance() {
        Some(handle) => handle,
        None => {
            log::info!("Another instance is already running. Activating it and exiting.");
            return Ok(());
        }
    };

    let app_local_data_dir = app_config_dir();
    let config_manager = ConfigManager::load(Some(app_local_data_dir)).expect("load config");
    let router = Router::new();
    let controller = Arc::new(Mutex::new(AppController::new(config_manager, router)));

    {
        let mut c = controller.lock().unwrap();
        c.init();
    }

    // 初始化系统代理监听：读取当前代理并启动后台线程监听注册表变化，
    // 用户在系统设置中切换 VPN 代理时自动热加载，无需重启应用。
    update::init_proxy_watcher();

    // 清理上次更新残留的安装包（%TEMP%\AudioRouter-Updates\*.exe），
    // 避免多次更新后临时文件累积占用磁盘。
    update::cleanup_old_installers();

    {
        let c = controller.lock().unwrap();
        let i18n = c.i18n.clone();
        drop(c);
        if let Err(e) = tray::init_tray(i18n) {
            log::warn!("Failed to initialize system tray: {e}");
        }
    }

    // 从配置读取初始 backdrop，在窗口创建时直接应用。
    // 必须通过 App::backdrop 在窗口创建阶段设置，而非在组件 use_effect 中
    // 事后调用 set_backdrop——后者依赖的 ROOT_WINDOW 在 UI 首次挂载后才设置，
    // use_effect 执行时机可能早于该设置，导致 backdrop 被静默丢弃。
    let initial_backdrop = {
        let c = controller.lock().unwrap();
        c.backdrop()
    };
    let reactor_backdrop = match initial_backdrop {
        config::config::Backdrop::Mica => Backdrop::Mica,
        config::config::Backdrop::MicaAlt => Backdrop::MicaAlt,
        config::config::Backdrop::Acrylic => Backdrop::Acrylic,
    };

    let icon_path = resolve_asset_path("assets/icon.ico");

    log::info!("Starting AudioRouter WinUI3 GUI...");
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

    app.run(move || app::RootComponent::new(Arc::clone(&controller)))
}
