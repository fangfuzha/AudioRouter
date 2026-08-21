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

## 架构

```
                ┌──────────────────────┐
                │     winui3_gui       │  WinUI 3 桌面 UI（windows-reactor）
                │  (主入口 / 视图层)    │
                └─────────┬────────────┘
                          │ 借用 Arc<Mutex<AppController>>
                ┌─────────▼────────────┐
                │      app_core        │  业务逻辑、Controller、I18n、更新
                │  (与 GUI 解耦)        │
                └─────────┬────────────┘
                          │ 持有 Router + ConfigManager
        ┌─────────────────┼──────────────────┐
        ▼                 ▼                  ▼
  ┌──────────┐      ┌──────────┐       ┌──────────┐
  │audio_core│      │  config  │       │ 系统 API │
  │ WASAPI   │      │ settings │       │  注册表  │
  │ 路由/枚举│      │  .toml   │       │  COM     │
  └──────────┘      └──────────┘       └──────────┘
```

- **audio_core** — 纯音频库。`router` 负责 WASAPI loopback 捕获 + 多端渲染(共享模式,SRC 自动转换),支持设备 invalidated 自动重启 + 指数退避重试。`com_service::device` 提供设备枚举 / 热插拔通知(`DeviceWatcher`)。
- **app_core** — Controller 把 GUI 调用翻译成 `Router.start / stop`,管理 `ConfigManager`,处理 i18n、自动启动、GitHub Release 更新检查与下载。
- **config** — `settings.toml` 原子写入(临时文件 + rename),启动时 schema 校验(版本号 + outputs 唯一性)。
- **winui3_gui** — WinUI 3 前端,windows-reactor 反应式组件。托盘 / 单例 / 代理热加载 / 关闭到托盘等平台行为都在这一层。

## 开发工作流

```bash
# 增量开发
cargo run -p winui3_gui                  # debug 模式运行,日志输出到 stderr
cargo build --release -p winui3_gui      # release 模式构建

# 静态检查(CI 也跑这三步)
cargo fmt --check                        # 格式
cargo clippy --workspace --all-targets -- -D warnings   # lint
cargo test --workspace --lib             # 单元测试

# 打安装包
.\scripts\build-installer.ps1 -Version 0.4.0
```

### 调试技巧

- **release 模式日志**:`%LOCALAPPDATA%\AudioRouter\logs\winui3_gui.log`,超过 5 MiB 自动 rotate 到 `.old`。
- **dev 模式日志**:直接走 stderr(因为有控制台)。
- **音频路由卡死**:log 里搜 `invalidated` / `Restart attempt`,通常意味着设备热插拔或格式切换,worker 会自动重试 10 次。
- **GUI 单实例**:首次启动会创建 `Local\AudioRouter-SingleInstance-Mutex`,第二次启动会唤起已有窗口后退出。

### 平台差异

- 项目目前 **Windows-only**(用 `windows` / `windows-reactor` crate 直接绑 Win32 + WinUI 3)。
- 跨平台编译 `cfg(windows)` guard 保证只拉 Windows 依赖;其他平台理论上能编译但跑不起来。
- 部分集成测试(`com_service::device::tests::device_api_behaves`、`device_watcher::tests::test_watcher_start_and_stop` 等)需要真机音频设备,默认 `#[ignore]`,需要真机调试时手动 `cargo test -- --ignored` 运行。

## 贡献

- 修改前先 `cargo fmt` + `cargo clippy --workspace --all-targets`。
- 新增配置字段记得在 `Config::validate` 里加相应检查;旧的 settings.toml 要保证能加载(用 `#[serde(default)]` 兜底)。
- 升级 `windows` crate 时,**`#[implement]` 宏生成的 wrapper 类型名是 `<原名>_Impl`**(0.58+),手动 `impl` 块必须写在 wrapper 上,不能写在原名上。
- 跨线程传 COM 指针请用 `ComSend<T>`,它强制 `T: Send`(2026-08 收紧),不要用裸 `*mut ...`。
