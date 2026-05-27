//! GitHub Release 版本更新检查

use serde::Deserialize;

/// 更新检查状态
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    Available {
        latest_version: String,
        html_url: String,
    },
    UpToDate,
    Error(String),
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
}

/// 检查 GitHub 上是否有新版本
pub fn check_for_update() -> UpdateStatus {
    let url = "https://api.github.com/repos/fangfuzha/AudioRouter/releases/latest";
    let resp = match ureq::get(url)
        .header("User-Agent", "AudioRouter/0.1.0")
        .call()
    {
        Ok(r) => r,
        Err(e) => return UpdateStatus::Error(format!("HTTP request failed: {e}")),
    };

    let release: GitHubRelease = match resp.into_body().read_json() {
        Ok(r) => r,
        Err(e) => return UpdateStatus::Error(format!("JSON parse failed: {e}")),
    };

    let current_version = env!("CARGO_PKG_VERSION");
    fn normalize(v: &str) -> &str {
        v.trim_start_matches(['v', 'V'])
    }

    if normalize(&release.tag_name) != normalize(current_version) {
        UpdateStatus::Available {
            latest_version: release.tag_name,
            html_url: release.html_url,
        }
    } else {
        UpdateStatus::UpToDate
    }
}
