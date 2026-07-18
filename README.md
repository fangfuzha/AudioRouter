# AudioRouter

轻量的桌面音频路由管理工具（基于 WinUI 3 + Rust）。

## 项目核心功能

- 将`一个音频输出设备`的音频路由到其他`多个音频输出设备`，实现多设备同时播放。

## 主要特点

- 实时列出系统音频设备并支持快速切换
- 纯 Rust 实现，原生 Windows 应用，无 WebView/Node.js 依赖
- 系统托盘常驻，关闭窗口自动隐藏到托盘

## 快速开始

开发环境要求：Rust toolchain + Windows App SDK（WinUI 3）。

```bash
# 运行
cargo run --package winui3_gui

# 构建发布
cargo build --release --package winui3_gui
```

生成的 exe 位于 `target/release/winui3_gui.exe`。

## 项目结构

```
audio_core/      # 音频核心库（设备枚举、路由引擎）
config/          # 配置管理（settings.toml）
app_core/        # 应用核心逻辑（控制器、i18n、自动启动）
winui3_gui/      # WinUI 3 桌面 GUI
installer/       # Inno Setup 安装脚本
scripts/         # 构建与打包脚本
assets/          # 共享资源（图标等）
```

## 打包发布

使用 [Inno Setup](https://jrsoftware.org/isdl.php) 生成 Windows 安装包。

### 前置条件

- 安装 [Inno Setup 6](https://jrsoftware.org/isdl.php)（推荐）
- 或设置环境变量 `ISCC_PATH` 指向 `ISCC.exe` 的完整路径

### 生成安装包

```powershell
# 自动构建 release 并生成安装包
.\scripts\build-installer.ps1

# 指定版本号
.\scripts\build-installer.ps1 -Version 1.0.0

# 跳过构建（已手动 cargo build --release 过）
.\scripts\build-installer.ps1 -NoBuild
```

生成的安装包位于 `installer/Output/` 目录下，文件名形如 `AudioRouter-Setup-<版本>-x64.exe`。

### 安装包特性

- 自包含部署：目标电脑无需安装 Windows App SDK 运行时
- 开始菜单快捷方式
- 可选桌面快捷方式
- 一键卸载（控制面板/设置中）
- 支持中文 / 英文双语安装向导

## [待办](TODO.md)
