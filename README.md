# AudioRouter

轻量的桌面音频路由管理工具（基于 WinUI 3 + Rust）。

## 项目核心功能

- 将`一个音频输出设备`的音频路由到其他`多个音频输出设备`，实现多设备同时播放。

## 主要特点

- 实时列出系统音频设备并支持快速切换（设备热插拔即时生效）
- 每个输出设备都可以单独「试听」，确认哪个是哪个音箱
- 每个输出设备可单独选择声道模式（立体声 / 单声道 / 左右声道互换等）
- 纯 Rust 实现，原生 Windows 应用，无 WebView/Node.js 依赖
- **单 exe、多线程**：托盘跑在主线程，路由跑在服务线程，WinUI 3 控制面板跑在自己的 UI 线程
- 控制面板**按需创建**：以「启动时最小化」方式开机时根本不加载 Windows App SDK，上百 MB 的框架开销不会白吃
- 系统托盘常驻，托盘菜单实时显示当前路由状态，「显示 / 隐藏」是真切换
- 可自定义全局热键（留空表示不注册），保存后立即生效；按下热键可随时唤出控制面板
- `Ctrl+W` 与标题栏关闭按钮行为一致：把窗口隐藏到托盘，托盘、路由与全局热键继续运行
- 记住窗口位置、大小与侧栏展开状态，下次启动自动恢复
- 首次启动按系统 UI 语言自动选择中文/英文界面，之后可在设置中随时切换

## 快速开始

开发环境要求：Rust toolchain + Windows App SDK（WinUI 3）。

```bash
# 运行
cargo run --package audiorouter

# 构建发布
cargo build --release --package audiorouter
```

生成的 exe 位于 `target/release/AudioRouter.exe`。托盘、路由、控制面板全在它里面。

## 项目结构

```
audio_core/        # 音频核心库（设备枚举、路由引擎）
config/            # 配置管理（settings.toml）
app_core/          # 应用核心逻辑（控制器、i18n、自动启动、进程内命令总线）
audiorouter/       # 唯一入口：托盘（主线程）+ 路由（服务线程）+ WinUI 3 控制面板（UI 线程）
installer/         # Inno Setup 安装脚本 + 运行时文件清单
scripts/           # 构建与打包脚本
assets/            # 共享资源（图标等）
```

## 打包发布

使用 [Inno Setup](https://jrsoftware.org/isdl.php) 生成 Windows 安装包。

### 前置条件

- 安装 [Inno Setup 6](https://jrsoftware.org/isdl.php)（推荐）
- 或设置环境变量 `ISCC_PATH` 指向 `ISCC.exe` 的完整路径
- 打包时会自动下载 Windows App Runtime 的 nupkg（约 160 MB）并校验，
  无需手动准备运行时

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

### 运行时从哪来

安装包里的 Windows App Runtime **不是**从 `target/` 里"顺带"拷的，而是
打包脚本按 `installer/runtime-files.txt` 的白名单，从
`Microsoft.WindowsAppSDK.Runtime` 的 nupkg → MSIX 里重新抽取的。

这样做的原因：上游 `windows_reactor_setup` 是把运行时拷进 `target/` 的
**build script 副作用**，sccache 命中时 build script 根本不执行，副作用
就没了 —— v0.3.12~v0.3.16 的安装包因此只有 exe，干净机器上装完是坏的。

现在打包脚本会**逐条校验**清单：必需条目少一个就直接失败（不会产出坏包），
可选条目（语言资源）缺失只给提示。

运行时版本写在 `installer/runtime-version.txt`（当前 `2.3.1`，与上游
windows-reactor-setup 的 pin 一致），可用 `-RuntimeVersion <版本>` 临时覆盖。
清单分两段：`required` 段缺一个就打包失败，`optional` 段（语言资源）缺失
只提示。

版本要和上游 pin 保持一致：exe 内嵌的 `app.manifest`、Rust 侧的 WinRT
绑定、以及 `cargo run` 用的运行时都是按上游那版生成的。要换版本就整套
换（先升 windows-rs pin），只换打包用的 DLL 会让开发环境和产品环境跑两套
运行时。打包脚本检测到版本不一致会打印警告。

### 两种部署模式

Release 里会提供**两个安装包**，按需选一个：

| 模式 | 文件名 | 大小 | 目标电脑要求 |
|---|---|---|---|
| 自包含（默认） | `AudioRouter-Setup-<版本>-x64.exe` | 约 16 MB，含整套运行时 | 无需安装 Windows App Runtime |
| 框架依赖 | `AudioRouter-Setup-<版本>-x64-fd.exe` | 约 4 MB，只含 bootstrapper | 需已安装 Windows App Runtime；没有的话首次启动会弹官方快速安装界面 |

已装 Windows App Runtime 的机器用框架依赖包更省事；没装过的用自包含包，
开箱即用。**两者共用同一个 AppId，互斥覆盖**，装了一个再装另一个会替换前者。

构建框架依赖变体：

```powershell
$env:CARGO_TARGET_DIR = "target\fd"
cargo build --release -p audiorouter --features audiorouter/framework-dependent
.\scripts\build-installer.ps1 -NoBuild -Version 1.0.0 -DeploymentMode framework-dependent -CargoTargetDir "target\fd"
```

打包脚本会检查"指定的模式"与"exe 实际形态"是否一致，不一致直接失败。

### 安装包特性

- 自包含部署：目标电脑无需安装 Windows App SDK 运行时
- 开始菜单快捷方式
- 可选桌面快捷方式
- 一键卸载（控制面板/设置中）
- 支持中文 / 英文双语安装向导

## [待办](TODO.md)

## 架构

```
   ┌──────────────────────────────────────────────────┐
   │  audiorouter.exe（单进程 · 多线程）               │
   ├──────────────────────────────────────────────────┤
   │ 主线程   托盘图标 / 全局热键 / Win32 消息循环      │
   │    │ 命令总线 (app_core::bus::Command)            │
   │    ▼                                             │
   │ 服务线程  路由启停 / 设备枚举 / 状态汇总           │
   │    │                                             │
   │ UI 线程    WinUI 3 控制面板（按需创建，STA）       │
   │    │                                             │
   │    ▼ 共享 Arc<Mutex<AppController>>               │
   │ ┌────────────────────────┐  ┌──────────────────┐ │
   │ │       app_core         │  │    audio_core    │ │
   │ │ Controller / I18n /    │  │ WASAPI 路由引擎  │ │
   │ │ 自动启动 / 命令总线    │  │ 设备枚举 / 热插拔 │ │
   │ └───────────┬────────────┘  └──────────────────┘ │
   │             ▼                                     │
   │        ┌─────────┐                                │
   │        │  config │  settings.toml（原子写入）     │
   │        └─────────┘                                │
   └──────────────────────────────────────────────────┘
```

- **audio_core** — 纯音频库。`router` 负责 WASAPI loopback 捕获 + 多端渲染(共享模式,SRC 自动转换),支持设备 invalidated 自动重启 + 指数退避重试。`com_service::device` 提供设备枚举 / 热插拔通知(`DeviceWatcher`)。
- **app_core** — Controller 把界面调用翻译成 `Router.start / stop`,管理 `ConfigManager`,处理 i18n、自动启动、GitHub Release 更新检查与下载。`bus` 是进程内命令总线,取代了旧的跨进程命名管道。
- **config** — `settings.toml` 原子写入(临时文件 + rename),启动时 schema 校验(版本号 + outputs 唯一性)。UI 线程与服务线程共享同一个 `ConfigManager`。
- **audiorouter** — 唯一入口。主线程跑托盘与热键,服务线程跑路由,UI 线程跑 WinUI 3 界面(windows-reactor 反应式组件)。单例 / 代理热加载 / 更新检查等平台行为在这一层。

## 开发工作流

```bash
# 增量开发
cargo run -p audiorouter                # debug 模式运行,日志输出到 stderr
cargo build --release -p audiorouter    # release 模式构建

# 静态检查(CI 也跑这三步)
cargo fmt --check                        # 格式
cargo clippy --workspace --all-targets -- -D warnings   # lint
cargo test --workspace --lib             # 单元测试

# 打安装包
.\scripts\build-installer.ps1 -Version 0.4.0
```

### 调试技巧

- **release 模式日志**:`%LOCALAPPDATA%\AudioRouter\logs\AudioRouter.log`,超过 5 MiB 自动 rotate 到 `.old`。
- **dev 模式日志**:直接走 stderr(因为有控制台)。
- **音频路由卡死**:log 里搜 `invalidated` / `Restart attempt`,通常意味着设备热插拔或格式切换,worker 会自动重试 10 次。
- **单实例**:首次启动会创建 `Local\AudioRouter-SingleInstance-Mutex`,第二次启动会通过托盘窗口类名找到常驻实例,让它把控制面板叫起来后自己退出。

### 平台差异

- 项目目前 **Windows-only**(用 `windows` / `windows-reactor` crate 直接绑 Win32 + WinUI 3)。
- 跨平台编译 `cfg(windows)` guard 保证只拉 Windows 依赖;其他平台理论上能编译但跑不起来。
- 部分集成测试(`com_service::device::tests::device_api_behaves`、`device_watcher::tests::test_watcher_start_and_stop` 等)需要真机音频设备,默认 `#[ignore]`,需要真机调试时手动 `cargo test -- --ignored` 运行。

## 贡献

- 修改前先 `cargo fmt` + `cargo clippy --workspace --all-targets`。
- 新增配置字段记得在 `Config::validate` 里加相应检查;旧的 settings.toml 要保证能加载(用 `#[serde(default)]` 兜底)。
- 升级 `windows` crate 时,**`#[implement]` 宏生成的 wrapper 类型名是 `<原名>_Impl`**(0.58+),手动 `impl` 块必须写在 wrapper 上,不能写在原名上。
- 跨线程传 COM 指针请用 `ComSend<T>`,它强制 `T: Send`(2026-08 收紧),不要用裸 `*mut ...`。
