//! GitHub Release 版本更新检查

use serde::Deserialize;

/// 更新检查状态
#[derive(Debug, Clone, Default)]
pub enum UpdateStatus {
    #[default]
    Idle,
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
    let version = env!("CARGO_PKG_VERSION");
    let resp = match ureq::get(url)
        .header("User-Agent", format!("AudioRouter/{version}"))
        .call()
    {
        Ok(r) => r,
        Err(e) => return UpdateStatus::Error(format!("HTTP request failed: {e}")),
    };

    let release: GitHubRelease = match resp.into_body().read_json() {
        Ok(r) => r,
        Err(e) => return UpdateStatus::Error(format!("JSON parse failed: {e}")),
    };

    // 语义化版本比较
    fn normalize(v: &str) -> &str {
        v.trim_start_matches(['v', 'V'])
    }
    let current = match semver::Version::parse(normalize(version)) {
        Ok(v) => v,
        Err(_) => return UpdateStatus::UpToDate,
    };
    let latest = match semver::Version::parse(normalize(&release.tag_name)) {
        Ok(v) => v,
        Err(_) => {
            // tag 无法解析为 semver，回退到字符串不等比较
            if normalize(&release.tag_name) != normalize(version) {
                return UpdateStatus::Available {
                    latest_version: release.tag_name,
                    html_url: release.html_url,
                };
            }
            return UpdateStatus::UpToDate;
        }
    };

    if latest > current {
        UpdateStatus::Available {
            latest_version: release.tag_name,
            html_url: release.html_url,
        }
    } else {
        UpdateStatus::UpToDate
    }
}
