# AudioRouter

轻量的桌面音频路由管理工具（基于 Tauri + Vue 3 + Rust）。

## 项目核心功能

- 将`一个音频输出设备`的音频路由到其他`多个音频输出设备`,实现多设备同时播放.

## 主要特点

- 实时列出系统音频设备并支持快速切换
- 本地后台服务（Rust）负责设备交互与性能关键逻辑
- 使用 Tauri 打包为原生 Windows 安装器（NSIS `.exe`）

## 快速开始

开发环境要求：Node.js（LTS）、Rust toolchain、Tauri CLI。

本地运行（前端开发）：

```powershell
# 预览前端(支持热重载)
npm run dev --prefix AudioRouter
# 运行 Tauri 应用（支持热重载）
npm run tauri dev --prefix AudioRouter
```

构建发布包：

```powershell
npm run tauri build --prefix AudioRouter
```

生成的安装包位于：`target/release/bundle/`（默认生成 NSIS `.exe`）。

## 发布（GitHub Actions）

- 本仓库已包含 GitHub Actions 工作流：推送 `v*` 标签时自动构建并创建 Release（仅上传 `.exe`）。
- 若需要代码签名，请在仓库 Secrets 中添加：
  - `WINDOWS_SIGN_CERT` — PFX 文件的 Base64 编码
  - `WINDOWS_SIGN_PASSWORD` — PFX 密码

触发发布示例：

```bash
git tag v1.0.0
git push origin v1.0.0
```

## [待办](TODO.md)

## 贡献

欢迎提交 issue 和 PR。请保持代码风格一致，新增功能请先在 issue 讨论实现细节。
