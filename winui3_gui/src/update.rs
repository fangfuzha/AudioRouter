//! 通过 GitHub API 检测新版本并执行自动更新。
//!
//! - 检测：GET https://api.github.com/repos/{owner}/{repo}/releases/latest
//! - 对比：semver 比较当前版本与最新版 tag（去掉前缀 v）
//! - 下载：从 release assets 中匹配 AudioRouter-Setup-*-x64.exe
//! - 安装：下载到临时目录后启动安装包并退出当前进程

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use semver::Version;

const GITHUB_OWNER: &str = "fangfuzha";
const GITHUB_REPO: &str = "AudioRouter";
const USER_AGENT: &str = "AudioRouter-Updater";
const INSTALLER_PATTERN: &str = "AudioRouter-Setup-";

/// 全局代理状态缓存。
///
/// 启动时由 `init_proxy_watcher()` 初始化，后台线程监听注册表变化
/// 并自动更新此值。`build_agent()` 从这里读取，避免每次网络请求
/// 都读取注册表，同时支持运行时代理变更热加载。
static PROXY_CACHE: OnceLock<RwLock<Option<String>>> = OnceLock::new();

/// 初始化代理监听：读取当前代理并启动后台监听线程。
///
/// 应在应用启动时调用一次。重复调用安全（第二次起返回缓存的锁）。
pub fn init_proxy_watcher() {
    PROXY_CACHE.get_or_init(|| {
        let proxy = read_system_proxy();
        log::info!("Initial system proxy: {proxy:?}");

        // 启动后台线程监听注册表变化
        std::thread::spawn(watch_proxy_registry);

        RwLock::new(proxy)
    });
}

/// 后台线程：使用 `RegNotifyChangeKeyValue` 监听代理注册表项变化。
///
/// 当用户在系统设置中开启/关闭代理或修改代理地址时，自动重新读取
/// 并更新 `PROXY_CACHE`，实现热加载。使用阻塞式通知，CPU 占用极低。
fn watch_proxy_registry() {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_NOTIFY,
        REG_NOTIFY_CHANGE_LAST_SET,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let subkey = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");

    let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_NOTIFY, &mut hkey)
    };
    if status != 0 {
        log::warn!("Failed to open registry key for proxy notification: {status}");
        return;
    }

    log::info!("Proxy registry watcher started");

    loop {
        // 阻塞等待注册表值变化（最后一个参数 0 = 同步阻塞）
        // windows-sys 中 BOOL 是 i32 类型，0 = FALSE，非零 = TRUE
        let status = unsafe {
            RegNotifyChangeKeyValue(
                hkey,
                0,
                REG_NOTIFY_CHANGE_LAST_SET,
                std::ptr::null_mut(),
                0,
            )
        };

        if status != 0 {
            log::warn!("RegNotifyChangeKeyValue failed: {status}, stopping watcher");
            break;
        }

        // 注册表发生变化，重新读取代理
        let new_proxy = read_system_proxy();

        // 更新全局缓存并记录变化
        if let Some(cache) = PROXY_CACHE.get() {
            let mut current = cache.write().unwrap();
            if *current != new_proxy {
                log::info!("System proxy changed: {:?}", new_proxy);
                *current = new_proxy;
            }
        }
    }

    unsafe { RegCloseKey(hkey) };
}

fn build_agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new().tls_connector(Arc::new(
        ureq::native_tls::TlsConnector::new().expect("failed to create native-tls connector"),
    ));

    // 从全局缓存读取代理设置。
    // 缓存由 init_proxy_watcher() 初始化，后台线程自动监听变化并更新。
    // 如果未初始化（如单元测试），则直接读取注册表作为回退。
    let proxy_url = PROXY_CACHE
        .get()
        .and_then(|cache| cache.read().unwrap().clone())
        .or_else(read_system_proxy);

    if let Some(proxy_url) = proxy_url {
        match ureq::Proxy::new(&proxy_url) {
            Ok(proxy) => {
                log::info!("Using system proxy: {proxy_url}");
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                log::warn!("Invalid proxy URL '{proxy_url}': {e}");
            }
        }
    }

    builder.build()
}

/// 读取 Windows 系统代理设置（通过注册表）。
///
/// Windows 系统代理配置存储在：
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`
/// - `ProxyEnable` (DWORD): 1 = 启用代理
/// - `ProxyServer` (String): 代理地址，格式可能是：
///   - `127.0.0.1:7890`（全局代理）
///   - `http=127.0.0.1:7890;https=127.0.0.1:7890`（按协议分别设置）
///
/// 返回 `http://host:port` 格式的代理 URL，如果没有配置代理则返回 None。
fn read_system_proxy() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_READ,
    };

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let subkey = to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");
    let proxy_enable_name = to_wide("ProxyEnable");
    let proxy_server_name = to_wide("ProxyServer");

    let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey)
    };
    if status != 0 {
        return None;
    }

    // 读取 ProxyEnable 判断是否启用代理
    let mut proxy_enable: u32 = 0;
    let mut len: u32 = std::mem::size_of::<u32>() as u32;
    let mut typ: u32 = 0;
    unsafe {
        RegQueryValueExW(
            hkey,
            proxy_enable_name.as_ptr(),
            std::ptr::null(),
            &mut typ,
            &mut proxy_enable as *mut u32 as *mut u8,
            &mut len,
        );
    }

    if proxy_enable == 0 {
        unsafe { RegCloseKey(hkey) };
        return None;
    }

    // 读取 ProxyServer 获取代理地址
    let mut buf = [0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    let mut typ: u32 = 0;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            proxy_server_name.as_ptr(),
            std::ptr::null(),
            &mut typ,
            buf.as_mut_ptr() as *mut u8,
            &mut len,
        )
    };
    unsafe { RegCloseKey(hkey) };

    if status != 0 {
        return None;
    }

    let proxy_str = String::from_utf16_lossy(&buf[..(len / 2) as usize]);
    let proxy_str = proxy_str.trim_end_matches('\0');

    if proxy_str.is_empty() {
        return None;
    }

    // 解析代理地址：
    // - "127.0.0.1:7890" → 直接使用
    // - "http=127.0.0.1:7890;https=127.0.0.1:7890" → 优先提取 https，回退 http
    let proxy_addr = if proxy_str.contains('=') {
        proxy_str.split(';').find_map(|s| {
            let s = s.trim();
            s.strip_prefix("https=")
                .or_else(|| s.strip_prefix("http="))
        })
    } else {
        Some(proxy_str)
    };

    let addr = proxy_addr?;

    // ureq 需要完整的 URL 格式（包含 http:// 协议前缀）
    if addr.starts_with("http://") || addr.starts_with("https://") {
        Some(addr.to_string())
    } else {
        Some(format!("http://{addr}"))
    }
}

/// GitHub release API 返回的简化结构
#[derive(Debug, serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GithubAsset>,
    draft: bool,
    prerelease: bool,
}

#[derive(Debug, serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// 检查更新的结果
#[derive(Debug, Clone)]
pub enum UpdateCheckResult {
    /// 已是最新版本
    UpToDate,
    /// 发现新版本
    NewVersion {
        version: String,
        download_url: String,
        release_notes: String,
        file_size: u64,
    },
    /// 检查失败
    Failed(String),
}

/// 获取当前应用版本（从 Cargo 编译期环境变量）
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 向 GitHub API 请求最新 release 信息并比较版本。
///
/// 阻塞调用，应在后台线程中执行。
pub fn check_for_updates() -> UpdateCheckResult {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        GITHUB_OWNER, GITHUB_REPO
    );

    let agent = build_agent();

    let release: GithubRelease = match agent
        .get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github.v3+json")
        .call()
    {
        Ok(resp) => match resp.into_json() {
            Ok(r) => r,
            Err(e) => return UpdateCheckResult::Failed(format!("parse response: {e}")),
        },
        Err(ureq::Error::Status(code, resp)) => {
            return UpdateCheckResult::Failed(format!(
                "HTTP {code}: {}",
                resp.status_text()
            ));
        }
        Err(e) => return UpdateCheckResult::Failed(format!("network error: {e}")),
    };

    if release.draft || release.prerelease {
        return UpdateCheckResult::UpToDate;
    }

    // 从 tag 中提取版本号（去掉前缀 v/V）
    let tag = release.tag_name.trim_start_matches('v').trim_start_matches('V');
    let latest = match Version::parse(tag) {
        Ok(v) => v,
        Err(e) => return UpdateCheckResult::Failed(format!("invalid version tag '{tag}': {e}")),
    };

    let current = match Version::parse(current_version()) {
        Ok(v) => v,
        Err(_) => return UpdateCheckResult::Failed(format!(
            "current version '{}' is not valid semver",
            current_version()
        )),
    };

    if latest <= current {
        return UpdateCheckResult::UpToDate;
    }

    // 找到匹配的安装包 asset
    let installer = release.assets.iter().find(|a| {
        a.name.starts_with(INSTALLER_PATTERN)
            && a.name.ends_with("-x64.exe")
    });

    let installer = match installer {
        Some(a) => a,
        None => return UpdateCheckResult::Failed(
            "no installer asset found in latest release".to_string(),
        ),
    };

    UpdateCheckResult::NewVersion {
        version: release.tag_name.clone(),
        download_url: installer.browser_download_url.clone(),
        release_notes: release.body.unwrap_or_default(),
        file_size: installer.size,
    }
}

/// 下载安装包到临时目录，返回本地文件路径。
///
/// 阻塞调用，应在后台线程中执行。
pub fn download_installer(download_url: &str, progress: impl Fn(u64, u64)) -> anyhow::Result<PathBuf> {
    let agent = build_agent();
    let resp = agent
        .get(download_url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| anyhow::anyhow!("download failed: {e}"))?;

    let total_size: u64 = resp
        .header("Content-Length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut reader = resp.into_reader();
    let tmp_dir = updates_tmp_dir();
    std::fs::create_dir_all(&tmp_dir)?;

    let file_name = download_url
        .rsplit('/')
        .next()
        .unwrap_or("AudioRouter-Setup.exe");
    let dest_path = tmp_dir.join(file_name);

    let mut file = std::fs::File::create(&dest_path)?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 8192];

    use std::io::Read;
    use std::io::Write;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        progress(downloaded, total_size);
    }

    file.flush()?;
    Ok(dest_path)
}

/// 启动安装包并退出当前进程。
///
/// 安装包会自动覆盖旧版本，无需手动卸载。
/// 启动后当前进程立即退出，因此这个函数不会返回。
pub fn launch_installer_and_quit(installer_path: &std::path::Path) -> ! {
    // 使用 /VERYSILENT /SUPPRESSMSGBOXES /NORESTART 让 Inno Setup 静默安装
    // 安装完成后自动启动应用（由 Inno Setup 的 postinstall Run 条目处理，
    // 注意 [Run] 段不能有 skipifsilent，否则静默安装不会执行 postinstall）。
    let path_str = installer_path.to_string_lossy().to_string();

    match std::process::Command::new(&path_str)
        .arg("/VERYSILENT")
        .arg("/SUPPRESSMSGBOXES")
        .arg("/NORESTART")
        .spawn()
    {
        Ok(child) => {
            log::info!("Installer launched: {path_str} (pid={})", child.id());
            // 等待短暂时间确保安装程序已启动并开始初始化，
            // 同时让当前进程的文件句柄有时间释放（安装程序需要覆盖 exe）。
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Err(e) => {
            log::error!("Failed to launch installer: {e}");
            // 即使启动失败也退出，避免进程挂起
        }
    }

    std::process::exit(0);
}

/// 格式化文件大小（字节 → 人类可读）
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// 更新安装包的临时下载目录。
///
/// 位于系统临时目录下：`%TEMP%\AudioRouter-Updates\`
fn updates_tmp_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("AudioRouter-Updates")
}

/// 清理临时下载目录中残留的旧安装包。
///
/// 在应用启动时调用，删除 `%TEMP%\AudioRouter-Updates\` 下所有
/// `AudioRouter-Setup-*.exe` 文件。上次更新安装完成后安装包仍残留
/// 在临时目录，新版本启动时负责清理，避免文件累积占用磁盘。
///
/// 清理失败只记录日志不中断启动。
pub fn cleanup_old_installers() {
    let dir = updates_tmp_dir();
    if !dir.exists() {
        return;
    }

    let mut cleaned = 0;
    let mut failed = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with(INSTALLER_PATTERN) && name.ends_with(".exe") {
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            log::info!("Removed old installer: {}", path.display());
                            cleaned += 1;
                        }
                        Err(e) => {
                            log::warn!("Failed to remove {}: {e}", path.display());
                            failed += 1;
                        }
                    }
                }
            }
        }
    }

    if cleaned > 0 || failed > 0 {
        log::info!(
            "Installer cleanup: {cleaned} removed, {failed} failed in {}",
            dir.display()
        );
    }
}
