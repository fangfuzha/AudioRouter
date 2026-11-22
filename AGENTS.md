# AGENTS.md

> Canonical agent guide for the `AudioRouter` workspace.
> Last refreshed: 2026-11-22 (README 收敛为用户文档 + docs 建立开发索引).
>
> 本文件只保留**入口信息、核心不变量速查与文档索引**；具体的历史记录、
> 平台坑、流程细节都按主题拆到 `docs/` 下的分文件里。

## Project Goal

Lightweight Windows desktop tool that **routes audio from one Windows output
device to multiple other output devices simultaneously** (think
"multi-room from a single source"). Native Rust + WinUI 3 — no WebView,
no Node.js.

Primary binary: `audiorouter` (`cargo run -p audiorouter`).

## Architecture Map

Workspace members (`Cargo.toml`):

| crate         | role                                                            | depends on                                          |
|---------------|-----------------------------------------------------------------|-----------------------------------------------------|
| `audio_core`  | WASAPI loopback capture + multi-render, device enum, hot-plug   | `config`, `windows 0.62`, `windows-core 0.62`       |
| `config`      | Atomic `settings.toml` read/write + schema validation           | `serde`, `toml 1`                                   |
| `app_core`    | `AppController`, i18n, autostart, 进程内命令总线, window placement | `audio_core`, `config`, `windows 0.62`             |
| `audiorouter` | 唯一入口：托盘 + 路由 + WinUI 3 控制面板（单 exe、多线程）      | `app_core`, `audio_core`, `config`, `windows-reactor` (git)  |

Dependency direction is one-way: `audiorouter → app_core → audio_core → config`.
Reverse edges are not allowed. 只有一个可执行文件（`AudioRouter.exe`），
模块之间靠 `app_core::bus` 的进程内命令总线通信，不再有跨进程的命名管道。

Hot path: `router::worker::event_loop` reads from the loopback capture
client and writes to N render clients on a 3ms timer; see
`audio_core/src/router/worker.rs`.

线程模型、窗口生命周期、托盘语义、单实例唤醒见
[docs/architecture.md](docs/architecture.md)。

## Source-of-Truth References

- Routing engine: `audio_core/src/router/{mod,worker,config,state}.rs`
- Device enumeration: `audio_core/src/com_service/{device,watcher}.rs`
- Controller state machine: `app_core/src/controller.rs`
- Config schema: `config/src/config.rs`
- 进程入口（线程编排）: `audiorouter/src/main.rs`
- 托盘 + 全局热键 + 主线程消息循环: `audiorouter/src/desktop.rs`
- 服务线程（路由控制 + 设备枚举 + 状态汇总）: `audiorouter/src/runtime.rs`
- UI 线程宿主（按需创建窗口）: `audiorouter/src/ui_host.rs`
- WinUI 3 界面: `audiorouter/src/app.rs`
- Update + proxy hot-reload: `audiorouter/src/update.rs`
- 服务推送消息 → 控制器 → 重绘: `audiorouter/src/events.rs`
- Window placement + hotkey capture + 显隐: `audiorouter/src/window_utils.rs`
- 进程内命令总线（Request / StateSnapshot / Command）: `app_core/src/bus.rs`
- 共享路径解析（配置目录 / 资源）: `app_core/src/paths.rs`
- Installer script: `scripts/build-installer.ps1`
- Inno Setup 脚本: `installer/AudioRouter.iss`
- 运行时文件白名单: `installer/runtime-files.txt`
- 运行时版本: `installer/runtime-version.txt`

面向终端用户的说明见 [README.md](README.md)；开发者文档索引见
[开发索引](#开发索引)；短期待办见 [TODO.md](TODO.md)。

## 核心不变量（速查）

改动前先扫一眼这几条。完整清单（每条都带「为什么」和踩坑现场）见
[docs/invariants.md](docs/invariants.md)。

- **只有一个 exe**（`AudioRouter.exe`），进程内靠 `app_core::bus` 通信，没有跨进程管道。
- **跨线程命令只有一条通道**：UI / 托盘 / 热键 / 设备变化全部汇进 `bus::Command`，
  服务线程是唯一消费者。
- **配置只有一份**：UI 线程与服务线程共享同一个 `ConfigManager`（`Arc<RwLock<Config>>`）。
- **UI 线程是 STA**：设备枚举必须在 watcher 线程，不能在 UI 线程（否则 RPC_E_CHANGED_MODE）。
- **路由启停不要放主线程**：主线程只跑托盘与热键。
- **UI 线程按需创建**：托盘模式（`--minimized`）下不要无条件起窗口。
- **关窗口 = 隐藏到托盘（默认）**：`WM_CLOSE` 被拦截成 `SW_HIDE`，不销毁窗口。
- **显隐必须回到窗口所属的线程**：托盘线程只 `PostMessage`。
- **最小化的窗口要用 `SW_RESTORE`**，`SW_SHOW` 对最小化窗口无效。
- **列表子项必须带稳定 `with_key()`**，否则 reactor 按位置 diff 会错位。
- **安装包的运行时文件绝不从 `target/` 拿**，必须按白名单从 nupkg/MSIX 抽取。
- **不许提交大文件（> 1MB）**：`scripts/check-large-files.ps1` + CI `size-guard` job 双重阻断，例外登记在 `scripts/large-files-allowlist.txt`。
- **WindowsAppRuntime 版本要四件套一起升**（见 [docs/packaging.md](docs/packaging.md)）。

## Validation Commands

Run from `D:\my_project\AudioRouter`:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
cargo build -p audiorouter                 # debug：单 exe（托盘 + 路由 + 控制面板）
cargo build --release -p audiorouter       # release
```

CI 在 `windows-latest` 上跑 `cargo fmt` + `cargo clippy`（`lint` job）
与 `cargo test`（`test` job），两者并行，见 `.github/workflows/ci.yml`。
CI 不再单独跑 `cargo check`：`cargo clippy --all-targets` 已经覆盖同样的
类型检查范围，而 `check` 只产出 metadata、`test` 需要全量 codegen，
两者产物不共享，等于白编一轮依赖。

打包安装包见 [docs/packaging.md](docs/packaging.md)；发版流程与 CI 细节见
[docs/release.md](docs/release.md)。

## Conventions

- Use `parking_lot` (not `std::sync::Mutex`) when adding shared state
  in `audiorouter` or `app_core`.
- COM interfaces crossing thread boundaries should be wrapped in
  `ComSend<T>` only when the type isn't already `Send`+`Sync`; prefer
  the raw interface and rely on the `windows` crate's own wrappers
  where possible. Keep COM calls inside a `ComApartment` scope.
- `Config` schema changes require updating `Config::validate` and
  adding `#[serde(default)]` for new fields to keep old `settings.toml`
  loadable.
- New error variants that should propagate to the GUI: return
  `anyhow::Error` from `app_core` methods; the controller maps them
  into `status_text`.
- `clippy::too_many_arguments` and `clippy::useless_conversion` are
  expected on `windows-reactor` components — annotate locally with
  `#[allow(...)]` rather than disabling at crate level.

## 开发索引

面向终端用户的内容在 [README.md](README.md)（功能、安装、使用、FAQ）——
**改动 README 前先确认那段是不是用户关心的**；构建、架构、打包、CI 这类
开发者内容一律留在 `docs/` 下。

| 文件 | 内容 |
|---|---|
| [docs/development.md](docs/development.md) | **开发者入口**：环境要求、快速开始、项目结构、开发工作流、调试技巧、平台差异、贡献指南 |
| [docs/architecture.md](docs/architecture.md) | 全局视图与 crate 分工、单进程多线程架构、进程内命令总线、窗口生命周期（按需创建 / 关闭到托盘 / 单实例唤醒）、事件驱动重绘、命名管道历史教训 |
| [docs/invariants.md](docs/invariants.md) | 全部「非显然约束」：构建与打包、`windows` crate API、WinUI/reactor、线程与窗口、音频、脚本与 CI |
| [docs/packaging.md](docs/packaging.md) | 坏包根因与修法、自包含 / 框架依赖两种部署模式与两条独立的更新路径、WindowsAppRuntime 版本钉死、打包脚本用法 |
| [docs/ui-winui.md](docs/ui-winui.md) | WinUI 3 界面的设计现状与改动记录：侧边栏、主页、设置页、托盘、首次启动语言、子类化崩溃 |
| [docs/audio.md](docs/audio.md) | WASAPI 缓冲单位与直通路径、热路径分配、内存占用基线 |
| [docs/release.md](docs/release.md) | 发版流程、CI 工作流与编译缓存、更新检查的镜像回退 |
| [docs/roadmap.md](docs/roadmap.md) | 长期待办与已知缺口（`TODO.md` 之外的项目） |
| [docs/history.md](docs/history.md) | 历史重构记录（2026-08-21 那一轮重构的细节，多数已被后续版本覆盖） |
