# AudioRouter

轻量的桌面音频路由管理工具（基于 egui + Rust）。

## 项目核心功能

- 将`一个音频输出设备`的音频路由到其他`多个音频输出设备`，实现多设备同时播放。

## 主要特点

- 实时列出系统音频设备并支持快速切换
- 纯 Rust 实现，原生 Windows 应用，无 WebView/Node.js 依赖
- 系统托盘常驻，关闭窗口自动隐藏到托盘

## 快速开始

开发环境要求：Rust toolchain。

```bash
# 运行
cargo run --package audio_router

# 构建发布
cargo build --release --package audio_router
```

生成的 exe 位于 `target/release/audio_router.exe`。

## 项目结构

```
audio_core/      # 音频核心库（设备枚举、路由引擎）
config/          # 配置管理（settings.toml）
audio_router/    # 桌面 GUI（egui + 系统托盘）
```

## [待办](TODO.md)
