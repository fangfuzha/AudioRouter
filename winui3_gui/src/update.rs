//! 通过 GitHub API 检测新版本并执行自动更新。
//!
//! - 检测：GET https://api.github.com/repos/{owner}/{repo}/releases/latest
//! - 对比：semver 比较当前版本与最新版 tag（去掉前缀 v）
//! - 下载：从 release assets 中匹配 AudioRouter-Setup-*-x64.exe
//! - 安装：下载到临时目录后启动安装包并退出当前进程

use std::path::PathBuf;
use std::sync::Arc;

use semver::Version;

const GITHUB_OWNER: &str = "fangfuzha";
const GITHUB_REPO: &str = "AudioRouter";
const USER_AGENT: &str = "AudioRouter-Updater";
const INSTALLER_PATTERN: &str = "AudioRouter-Setup-";

fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .tls_connector(Arc::new(
            ureq::native_tls::TlsConnector::new().expect("failed to create native-tls connector"),
        ))
        .build()
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
    let tmp_dir = std::env::temp_dir().join("AudioRouter-Updates");
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
    // 安装完成后自动重启应用（由 Inno Setup 的 postinstall Run 条目处理）
    // 这里我们只启动安装包，然后当前进程退出。
    let path_str = installer_path.to_string_lossy().to_string();

    match std::process::Command::new(&path_str)
        .arg("/VERYSILENT")
        .arg("/SUPPRESSMSGBOXES")
        .arg("/NORESTART")
        .spawn()
    {
        Ok(_) => {
            log::info!("Installer launched: {path_str}");
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
