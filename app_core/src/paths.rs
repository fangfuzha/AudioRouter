//! 应用目录与资源路径（GUI 与服务进程共用）。
//!
//! 两个进程必须指向同一份 settings.toml 和同一份图标资源，
//! 所以路径解析集中放在这里。

use std::path::PathBuf;

/// 配置目录：`%LOCALAPPDATA%\AudioRouter`（找不到时退回 %APPDATA%）。
pub fn app_config_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("AudioRouter")
}

/// 资源文件路径。兼容多种运行场景：
///
/// 1. `{exe_dir}/{rel_path}` —— 安装后场景（exe 和 assets 在同一目录）
/// 2. 从 exe 目录逐级向上查找（最多 3 级）—— 覆盖 `target/debug/` → 项目根
/// 3. `CARGO_MANIFEST_DIR/../{rel_path}` —— cargo run 环境变量，最准确
/// 4. 当前工作目录 `./{rel_path}` —— 兜底
///
/// 全部找不到则返回 `{exe_dir}/{rel_path}`（调用方自己判断 exists）。
pub fn resolve_asset_path(rel_path: &str) -> PathBuf {
    let exe_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("winui3_gui.exe"));
    let exe_dir = exe_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut candidates = vec![exe_dir.join(rel_path)];

    let mut parent = exe_dir.clone();
    for _ in 0..3 {
        if let Some(p) = parent.parent() {
            parent = p.to_path_buf();
            candidates.push(parent.join(rel_path));
        } else {
            break;
        }
    }

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR")
        && let Some(workspace_root) = PathBuf::from(manifest_dir).parent()
    {
        candidates.push(workspace_root.join(rel_path));
    }

    candidates.push(PathBuf::from(rel_path));

    for path in &candidates {
        if path.exists() {
            return path.clone();
        }
    }

    candidates.remove(0)
}
