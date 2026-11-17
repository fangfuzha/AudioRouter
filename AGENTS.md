# AGENTS.md

> Canonical agent guide for the `AudioRouter` workspace.
> Last refreshed: 2026-11-20 (运行时打包修复 + 自包含/框架依赖双部署模式).

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

For end-user docs see `README.md`; for short-term todos see `TODO.md`.

## Current Status (2026-11-20，运行时打包修复 + 部署模式可切换)

### v0.3.12~v0.3.16 坏包的根因与修法

v0.3.12 之后的安装包只有 exe（3.86 MB），缺 28 个 WinAppSDK DLL / `.pri` /
语言目录，干净机器全新安装是坏的：自包含模式的 `app.manifest` 要求这些
DLL 就地在 exe 旁边，`.pri` 缺失则**窗口创建失败后静默退出**。

根因是 `windows_reactor_setup::as_self_contained()` 的**文件系统副作用**：
它把运行时拷到 `target/<profile>/`，而 sccache 命中时 build script 根本不
执行，副作用就消失了；`cache-targets: false` 又让 `target/` 不被还原。
上游的下载/拷贝失败还全是静默的（只 `println!`），于是 CI 一路绿灯地产出
坏包。

修法（`scripts/build-installer.ps1`）：

- **打包脚本自己准备运行时**，不再依赖 build script 副作用：按
  `installer/runtime-files.txt` 的白名单，从
  `Microsoft.WindowsAppSDK.Runtime` 的 nupkg → MSIX 里抽取条目。
- **清单逐条校验**：`required` 段的条目少一个就 `exit 1`，
  `optional` 段（语言资源）缺失只给提示；另外 14 个关键文件
  （DLL + `.pri`）再单独点名检查。
- **缺依赖一律打包失败**，不会再产出"只装了一个 exe"的包。
- nupkg 优先复用 `%LOCALAPPDATA%\windows-reactor-setup\temp`（上游下载到
  同一个目录、文件名规则一致），CI 里再加一层 actions/cache。

### 两种部署模式

| 模式 | 开关 | 安装包内容 | 运行时来源 |
|---|---|---|---|
| 自包含（默认发布形态） | 默认 | exe + 27 DLL + 4 `.pri` + 86 语言目录 + `Microsoft.UI.Xaml/`（staging 约 62 MB，装包 16 MB） | 随包发布 |
| 框架依赖 | `cargo build --features framework-dependent` | exe + `Microsoft.WindowsAppRuntime.Bootstrap.dll` + `resources.pri`（装包 4 MB） | 系统上已安装的 WindowsAppRuntime 包；缺失时 bootstrapper 弹官方快速安装界面 |

**两个包都作为 Release 资源发布**（`AudioRouter-Setup-<ver>-x64.exe` 自包含、
`AudioRouter-Setup-<ver>-x64-fd.exe` 框架依赖），用户自己选。二者 **AppId 相同、
互斥覆盖**：装了一个再装另一个会替换掉前者，不会并存。

- 模式由 cargo feature `audiorouter/framework-dependent` 决定：
  `audiorouter/build.rs` 读 `CARGO_FEATURE_FRAMEWORK_DEPENDENT` 切换
  `as_framework_dependent()` / `as_self_contained()`；
  `audiorouter/src/main.rs` 在建任何 XAML 对象之前调
  `windows_reactor::bootstrap()`。
- **打包脚本会校验模式与 exe 实际形态一致**：自包含的 exe 内嵌了
  `app.manifest`（含 `asmv3:file`），框架依赖的没有；不一致直接 `exit 1`。
- 框架依赖模式的安装包只有几 MB，但**要求系统已装（或愿意装）Windows
  App Runtime**，适合企业统一分发运行时的场景。

### WindowsAppRuntime 版本

- 版本号在 `installer/runtime-version.txt`，当前是 **2.3.1** —— 与上游
  `windows-reactor-setup` 的 pin 一致，**不要单独抬版本**。
- **为什么必须保持一致**：四样东西按同一个版本生成，缺一不可：
  1. exe 内嵌的 `app.manifest`（上游 pin 里的 WinAppSDK 2.3.5 package fragment）；
  2. Rust 侧 WinRT 绑定（`windows-reactor` 同一 pin 生成的元数据）；
  3. `build.rs` 里 `as_self_contained()` 拷进 `target/<profile>/` 的运行时
     （上游硬编码 `RUNTIME_VER = "2.3.1"`）；
  4. 打包脚本抽取的运行时（`installer/runtime-version.txt`）。

  只抬第 4 个，会让 `cargo run` 用的运行时和发给用户的运行时变成两套；
  而且新版本独有的 WinRT 类既不在 manifest 里、也不在 Rust 绑定里，
  调用会**静默失败** —— 要等哪天真用上新 API 才炸。
- **升级的正确姿势（整套一起升）**：先升 `windows-rs` pin（绑定与
  app.manifest 一起重新生成）→ 再改 `installer/runtime-version.txt` →
  跑打包脚本验证。
- 打包脚本有版本一致性护栏：从上游 crate 源码里读 `RUNTIME_VER`，与
  `installer/runtime-version.txt` 不一致就打印警告（只警告不阻断）。
- 查最新版本：flat container API
  `https://api.nuget.org/v3-flatcontainer/microsoft.windowsappsdk.runtime/index.json`。
- **换版本流程**：改 `installer/runtime-version.txt` → 跑打包脚本 →
  脚本逐条校验清单。清单分两段：
  - `required` 段（运行时 DLL / `.pri` / `Microsoft.UI.Xaml/`）**缺一个就
    打包失败**；
  - `optional` 段（语言资源目录）缺失只给提示，不同版本收录的语言不一样。
- 已知事实：**2.6.5（nuget 上最新稳定版）实测能跑** —— 清单 118 个条目
  全部命中，干净安装后主页与设置页都完整渲染（UIA：主页 44 个后代元素、
  设置页 61 个），设备列表正常。但它是"错位可用"，**不是推荐状态**。
- **框架依赖模式不用管版本**：bootstrapper 只负责"确保框架已安装"，
  之后系统上的 `Microsoft.WindowsAppRuntime.2` 包由 Windows Update /
  Store 自动更新到最新的 2.x（本机实测是 2.6.5.0）。

## Current Status (2026-11-18，单进程多线程：托盘是主线程，窗口是副线程)

以前这个应用是**两个 exe**：`audiorouter_service.exe`（路由 + 托盘 + 热键）
和 `winui3_gui.exe`（WinUI 3 控制面板），靠本地命名管道通信。开发时必须
同时构建两个二进制、还得保证它们同目录，调试一次问题要跨两个进程来回看。
现在合并成**一个 exe**（`AudioRouter.exe`），同样的职责用多线程拆开。

### 线程分工

| 线程 | 职责 | 代码位置 |
|---|---|---|
| **主线程** | 托盘图标、全局热键、Win32 消息循环 | `desktop::run()`（进程入口就停在这里） |
| **服务线程** | 路由启停、设备枚举、状态汇总 | `runtime::run()` |
| **UI 线程** | WinUI 3 控制面板（STA 公寓） | `ui_host::run_ui()`，**按需创建** |
| 音频 worker | WASAPI 采集 + 多路渲染 | `audio_core` 内部（热路径） |

### 关键设计

- **路由控制必须离开主线程。** `Router::start()` 最多要等 5 秒才等到音频
  worker 就绪（`ready_rx.recv_timeout(5s)`），放在主线程会把托盘卡死 5 秒。
  所以主线程只跑托盘与热键，路由启停交给服务线程。
- **UI 线程必须是 STA。** WinUI 3 要求 UI 线程是单线程公寓，创建任何 XAML
  对象之前要先 `CoInitializeEx(COINIT_APARTMENTTHREADED)`。reactor 的
  `App::run()` 会在**调用它的线程**上初始化公寓并接管消息泵，因此
  `App::run()` 要放在一条专门 spawn 出来的线程上；reactor 自己也明确
  支持这种用法（错误信息里写着「from `main` (or another thread you fully
  own)」）。跨线程的 `AsyncSetState::call` 由 reactor 的 marshaller 负责
  搬回 UI 线程。
- **窗口按需创建。** 「启动时最小化」（含开机自启的 `--minimized`）时
  根本不创建窗口，WindowsAppSDK 那 ~120 MB 的框架开销也就不会在开机时
  白白吃掉；等用户第一次点托盘图标时，`ui_host::ensure_ui_thread()` 才把
  UI 线程与窗口建起来。这是单进程架构能保住「低内存常驻」的关键。
- **进程内命令总线取代命名管道。** 所有跨线程请求汇进一条 `mpsc` 通道
  （`app_core::bus::Command`），服务线程是唯一的消费者；服务线程回推状态
  走 `StatePublisher` 回调。管道分帧、连接握手、断开重连那一整套复杂度
  全部消失。
- **配置只有一份。** UI 线程与服务线程共享同一个 `ConfigManager`
  （内部是 `Arc<RwLock<Config>>`），UI 落盘后服务线程立刻读到最新值，
  不再有「谁先写盘」的竞态，也不用各自去读 `settings.toml`。
- **`close_to_tray` 回到经典含义**：true = 关窗口 / Ctrl+W 时把窗口
  **隐藏到托盘**，应用继续在托盘里跑（路由与热键都不中断）；false = 关窗口
  就退出整个应用。窗口过程拦截 `WM_CLOSE` 并 `ShowWindow(SW_HIDE)`，
  而不是销毁窗口——`Application::Start` 一个进程只能调一次，窗口一旦
  销毁就再也叫不回来了。
- **托盘的「显示 / 隐藏」是真切换。** 以前这个菜单项只有「显示」的实现
  （`launcher::show_control_panel()` 永远只 `ShowWindow`），点它根本关不掉
  窗口；托盘「退出」又只杀服务进程，留下一个连不上服务、只会弹「服务连接
  已断开」的孤儿面板。现在两条路都在同一个进程里：显示/隐藏是真正的显隐
  切换，退出则整个进程一起退出。
- **开机自启指向唯一的 exe**（`app_core::autostart` 的 `autostart_target()`），
  带 `--minimized`，于是开机只有托盘与路由。
- **第二个实例靠托盘窗口类名找到常驻实例**：`FindWindowW("AudioRouterServiceWindow")`
  + `PostMessage(WM_TRAY_SHOW_PANEL)`，让常驻实例自己把窗口叫起来。

### 历史包袱：命名管道上不能留挂起的阻塞读

（拆分进程时代踩过的坑，管道已经删除，留个记录以防将来重新引入 IPC。）

最初服务端用「读线程 + 主循环写」的模型，结果两边互等、整个服务卡死：
Windows 对同一个 file object 的**同步 I/O 会串行化**——读线程挂着阻塞
`ReadFile` 时，主循环往同一条管道 `WriteFile` 会一直等那次读返回；而读
又在等客户端的下一帧，双方互等，**死锁**。表现是「命令要等下一条命令
才生效」（启动路由被延迟十几秒）。

当时的解法是**读之前先 `PeekNamedPipe` 看有没有数据，有才读**，任何时刻
都不留挂起的读操作。现在是进程内通道，没有这个问题；但如果将来又引入
跨进程 IPC，别图省事改回阻塞读。

## Current Status (2026-11-18，内存占用优化)

v0.3.16 修掉了音频客户端的一处单位换算 bug，并削减了界面渲染路径上的重复分配。

- **WASAPI 缓冲时长的单位是 100ns，原来写成了 5 秒。**
  `hnsBufferDuration` 的单位是 100ns（1 秒 = 10,000,000），代码里写的
  `50_000_000` 注释成「50ms」，实际是 **5 秒**。引擎照此分配代理缓冲，
  每个音频客户端多占约 **1.9 MB** 私有内存，路由延迟也跟着涨好几秒。
  采集端与渲染端都改成 `500_000`（50ms）。
- **共享模式下不再无谓请求格式转换。** 目标设备的原生混音格式与源一致时
  （Windows 上通常是统一的 48kHz/32bit/2ch），只传格式、不开
  `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`，引擎走直通路径，不再为每个客户端
  建立采样率/位深转换器；设备拒绝直通时自动回退。
- **热路径不再反复分配。** f32 暂存缓冲区复用；`GetBufferSize()` 改为
  初始化时缓存一次；回调改传 `&F`，去掉每包的 `Arc` 克隆。
- **界面渲染路径的重复拷贝收敛。** `I18n` 词表由 `HashMap` 改为静态切片
  （省掉常驻哈希表，clone 只剩几个指针）；渲染不再深拷贝整份 `Config`；
  设备列表每帧克隆次数 3 → 1；输出行的配置读取提到循环外。
- **设备热插拔事件加 300ms 尾随去抖**，避免属性变化引发的全量枚举 + 全量重绘。

实测（release 构建，同一台机器）：

| 场景 | 改前 工作集 | 改后 工作集 |
|---|---|---|
| 空闲（不路由） | 123.1 MB | 123.3 MB |
| 路由 → 1 个输出 | 131.4 MB | 124.3 MB |
| 路由 → 3 个输出 | 139.2 MB | 124.1 MB |

每个音频客户端的私有内存从 1.84 MB 降到 0.02 MB，输出设备数量对内存几乎
不再有影响。空闲的 ~123 MB 是 Windows App SDK + D3D 合成器的固有开销。

## Current Status (2026-11-18)

Four UI complaints fixed in one pass:

- **Sidebar keeps the 0.3.12 native look and now persists its state.** The
  pane is the framework `NavigationView`; its built-in `PaneToggleButton`
  (AutomationId `TogglePaneButton`) draws the hamburger at the **top of the
  pane, above 主页**, and the framework swaps the glyph itself. The expand
  state is read back through `NavigationView::on_pane_open_changed`
  (windows-reactor PR #4782), stored in `general.nav_pane_expanded`
  (`settings.toml`) and restored on the next start. The `windows-reactor`
  git pin moved from `3415ce9` (2026-08-02) to `a48e42e` (2026-08-05, the
  merge that added the pane callback).
- **Channel-mode explanation is tooltip-only.** The always-on line under each
  output row is gone; the ComboBox carries it as `ToolTipService` content
  (`.tooltip_with(hover_hint(&selected_desc))`), so it appears on hover only.
- **Output device list re-laid out.** Rows are a 4-column Grid
  `[Auto, STAR, Pixel(150), Auto]` (checkbox / name+state / mode combo /
  试听), vertically centered, with a `DividerStroke` bottom border and 8px
  inner padding; the list `spacing` is 0 so the rows read as one table.
- **「全局热键」→「打开窗口快捷键」** in both locales, and the shortcut can
  now be **captured from the keyboard** with a 录制 button. `windows-reactor`
  exposes no `KeyDown` event, so capture runs on a **low-level keyboard hook**
  (`WH_KEYBOARD_LL`) installed on the UI thread while recording; the window
  procedure stays as a fallback.

## Current Status (2026-11-17)

Main window, settings page and tray got a usability pass (v0.3.13):

- **Main window**: the status line became a semantic `InfoBar` (Ok / Warning /
  Error), the source device combo shows a placeholder until a device is picked,
  every output row has a **试听** button (plays a test tone on that device from
  a background thread), the selected channel mode is spelled out under the row
  (since 2026-11-18 it is a hover tooltip instead), active outputs are marked "● 输出中", and the
  header shows how many outputs are checked.
- **Settings page**: the page scrolls, saving a bad global hotkey now shows an
  inline `InfoBar` instead of failing silently, unsaved edits are flagged, and
  a **诊断** section opens the config / log folders and shows the version.
- **Tray**: a status line plus enabled/disabled start/stop entries and a
  tooltip mirror the router state; the menu is only rewritten when the state
  actually changes.
- **Navigation**: the framework's own pane-toggle button has no event
  callback, so the whole pane is now drawn by hand (see 2026-11-18); the
  expanded state is persisted in `settings.toml`.
- **Background failures**: no balloon API in the tray, so a failed route
  flashes the window (`FLASHW_ALL`) when the window is hidden.

## Current Status (2026-11-16)

**Tray hide → show no longer kills the app.** The keyboard-focus window
subclassing used two fixed slots (`MAIN_WNDPROC` / `KEYBOARD_WNDPROC`),
and the focus window could be subclassed **twice**. Hiding the window and
showing it again makes WinUI 3 rebuild the keyboard-focus window; when the
focus switched back to the original window, the second `SetWindowLongPtrW`
returned our own `subclass_wndproc`, which was then saved as if it were
the original window procedure. Every message to that window recursed forever,
the stack overflowed, and the process died with
`STATUS_FATAL_USER_CALLBACK_EXCEPTION` (0xc000041d) — visible in the
Windows event log as APPCRASH in `ntdll.dll` / `USER32.dll`. The
original window procedure is now tracked per window, a window is never
subclassed twice, and the previous focus window is restored when the focus
moves.

First-launch language now follows the system UI language:

- `app_core::i18n::detect_system_locale()` reads the user's UI language via
  `GetUserDefaultUILanguage` — the same source Inno Setup uses with its default
  `LanguageDetectionMethod=uilanguage` — and maps it to `zh` / `en`.
- `ConfigManager::load_with_default_language()` applies that language **only**
  when `settings.toml` does not exist yet. An existing config is never
  rewritten, so a language picked in the settings UI survives restarts and
  upgrades.

## Current Status (2026-11-10)

Event-driven rework of the WinUI frontend:

- **The 700ms `DispatcherTimer` poll is gone.** Device hot-plug, router
  state and tray interactions are now event-driven; background threads
  request a repaint through `RenderCx::use_async_state` (an
  `AsyncSetState<u64>`, safe to call from any thread) instead of the GUI
  polling shared state on a timer.
- **Device enumeration moved off the UI thread.** WinUI's UI thread is an STA
  apartment, while `audio_core` needs COM MTA (`ComApartment::mta()`);
  enumerating on the UI thread fails with RPC_E_CHANGED_MODE (0x80010106).
  The device watcher thread now enumerates and the UI only reads the result.
- **Window position memory**: `app_core::window_placement` stores the
  window rect in `HKCU\Software\AudioRouter\WindowPlacement`; the GUI saves
  on `WM_EXITSIZEMOVE` and restores on startup (clamped to the virtual
  screen).
- **Update check uses mirror fallbacks**: the GitHub API and release assets
  are tried against the official endpoint first, then a list of prefix-style
  GitHub proxies (`MIRROR_PREFIXES` in `audiorouter/src/update.rs`).
  Override with the `AUDIO_ROUTER_GITHUB_MIRROR` environment variable.
- `audio_core::Router::subscribe()` added so the GUI can block on router
  events instead of polling `poll_events()`.

## Current Status (2026-08-21)

Refactor pass complete. Key changes:

- `windows` crate unified to 0.62 in `audio_core` and `app_core`
  (via `windows 0.62` + `windows-core 0.62`). `winui3_gui` still pulls
  the `windows 0.62` line through `windows-reactor` git rev
  `3415ce9f856aeaf79555bbbfac5acb02405dcc89`; `windows-sys 0.61` is
  used alongside.
- Removed `callcomapi` dependency; COM apartment is now managed via the
  `windows` crate's `ComApartment` RAII guard in `audio_core`.
- Dropped the `ComSend` wrapper layer (was a Send/Sync convention with
  no compiler enforcement); COM interfaces crossing thread boundaries
  rely on `ComSend<T>` from `audio_core::com_service::watcher` only
  where actually needed, with `register/unregister` now returning
  `Result<()>` and taking references.
- `device.rs` switched from `OsStr + encode_wide` to `HSTRING::from`;
  `utils.rs` switched from manual raw-pointer manipulation to
  `PWSTR::to_string()`.
- `autostart.rs` updated for `windows 0.62` `Option<u32>` parameter
  shape; version bumped to 0.3.10.
- `ureq` 2 → 3 in `winui3_gui`.
- `toml` 0.7 → 1 in `config`.
- `tray-icon` 0.19 → 0.21.
- `parking_lot::Mutex` replaces all `std::sync::Mutex` in `winui3_gui`.
- `ComSend` Send/Sync bounds tightened to `T: Send` / `T: Sync`.
- Worker restart uses exponential backoff (200ms → 2s, 10 attempts).
- `apply_running_config` snapshots and rolls back on stop failure.
- `Config::validate` checks `config_version` and `outputs` uniqueness.
- Release log file rotates at 5 MiB in `init_logger`.
- CI: `cargo fmt --all` runs as a fallback before `--check` so a forgotten
  local `cargo fmt` no longer fails the run; `mozilla-actions/sccache-action`
  upgraded to v0.0.11 (v0.0.8 could not write to the retired Cache API v1,
  so sccache silently ran read-only and never produced a cache entry).
- Clippy + fmt + tests all clean at workspace level.

## High-Priority Remaining Work

1. **TODO.md items**:
   - "Update logic added, not verified" — the download half is now covered
     by `update::tests::downloads_latest_installer` (`#[ignore]`, needs
     network). Run `cargo test -p audiorouter --bin AudioRouter -- --ignored`
     to exercise fetch + download against the real latest release. The
     installer handoff itself (`launch_installer_and_quit`) still needs a
     human pass: it installs over the running build.
   - Multi-channel routing (5.1 → stereo downmix) is out of scope per
     the existing `decode_channel_mask` parsing in
     `audio_core/src/utils.rs`.
2. **Single `windows` version** — `windows-reactor` git rev is the
   last holdout pulling the workspace's `windows 0.62` line; pin or
   vendor once a stable release is published.
3. **窗口句柄改由框架提供** — 上游 issue microsoft/windows-rs#5388：
   `windows-reactor` 暴露
   `ReactorHost::hwnd()`（`IWindowNative::WindowHandle`）与
   `app_window()`（`IWindow2::AppWindow`），之后可以停用
   `EnumWindows` + 标题匹配，并进一步用 `AppWindow.Hide()/Show()`
   取代裸 `ShowWindow`。当前 reactor 手写的 WinAppSDK 绑定还没有
   `Hide()` / `Show()`，需要上游补齐。

## Non-Obvious Invariants

- **安装包的运行时文件绝不从 `target/` 拿。** `target/<profile>/` 里的
  WinAppSDK DLL 是 build script 的副作用，sccache 命中时根本不存在 ——
  打包脚本必须从 nupkg/MSIX 按 `installer/runtime-files.txt` 重新抽取，
  并逐条校验。
- **ISPP 的 `#if` 里 `==` 只能比较字符串**，拿宏和数字比会报
  "Operator not applicable to this operand type"。部署模式改用
  "staging 里有没有 bootstrapper" 这种文件存在性判断。
- **build script 里读 feature 要用 `CARGO_FEATURE_*` 环境变量**：
  `#[cfg(feature = "...")]` 在 `build.rs` 里不生效（build script 是独立的
  crate，拿不到本包的 feature）。
- **自包含包与框架依赖包共用同一个 AppId**：两者互斥覆盖，不会并存。
  发布时两个都上传，用户自己选装哪个。
- **`#[implement]` macro wrapper name** (windows 0.62+): the macro
  generates a `<OriginalName>_Impl` newtype. Manual `impl <Trait>_Impl`
  blocks must target the wrapper, **not** the original name. See
  `audio_core/src/device_watcher.rs` `NotificationClient_Impl`.
- **`PROPVARIANT` internals are not visible** in `windows 0.62`+;
  use `PropVariantToStringAlloc` from
  `windows::Win32::System::Com::StructuredStorage` rather than
  dereferencing the transparent newtype.
- **`CoInitializeEx` returns `HRESULT`**, not `Result<(), Error>`, in
  `windows 0.62`+ — see `ComApartment::mta()` in
  `audio_core/src/router/worker.rs`.
- **`ureq 3` API**:
  - `Agent::config_builder()` not `AgentBuilder::new()`.
  - `.header()` not `.set()`.
  - `Error::StatusCode(u16)` not `Error::Status(u16, Response)`.
  - `body_mut().read_json()` / `into_body().into_reader()` for bodies.
  - Headers via `http::Response::headers()`.
- **`parking_lot::Mutex::lock()` returns a `MutexGuard` directly**,
  no `Result` — every `.lock().unwrap()` in old code became
  `.lock()`. Search-replace was already run; do not reintroduce
  `.lock().unwrap()` on `parking_lot::Mutex`.
- **`windows-reactor` `Element::from(...)` is required** in some
  builder contexts — clippy's `useless_conversion` lint is suppressed
  locally; do not strip the `#[allow]` without testing the UI.
- **COM apartment on the UI thread**: WinUI initializes its UI thread as
  STA. Any `CoInitializeEx(COINIT_MULTITHREADED)` there returns
  RPC_E_CHANGED_MODE (0x80010106). Device enumeration therefore runs on the
  device-watcher thread, never on the UI thread.
- **Cross-thread repaint**: `RenderCx::use_state` returns an Rc-backed
  `SetState` and must stay on the UI thread. Use
  `RenderCx::use_async_state` when a background thread needs to trigger a
  repaint; its `AsyncSetState::call` marshals onto the UI thread. It is a
  no-op when the written value is unchanged, so always write a fresh
  counter value.
- **`windows-reactor` reconciles unkeyed children by position.** A
  `StackPanel` whose children have no `with_key()` is diffed index by index,
  so inserting or removing a child in the *middle* shifts every later sibling
  onto the wrong control — the UI then shows duplicated / missing sections.
  Any list whose children come and go (conditional error bars, device rows,
  empty-state hints) must give every child a stable `with_key()`. `Element::Empty`
  and `Element::Group` are dropped / spliced before diffing, so they are not a
  substitute for a key.
- **The sidebar uses the framework `NavigationView` (0.3.12 behaviour).**
  Its built-in `PaneToggleButton` sits at the top of the pane, above 主页, and
  the framework manages `IsPaneOpen` and the glyph — do not hide it
  (`.pane_toggle_button_visible(false)`) and do not move a replacement button
  into `pane_footer`, or the hamburger ends up at the bottom of the pane.
  The expand state is read back through `NavigationView::on_pane_open_changed`
  (windows-reactor PR #4782) and stored in `general.nav_pane_expanded`.
- **WinUI overrides `IsPaneOpen` when the NavigationView template loads.** A
  value set at mount is discarded: the framework forces its own default
  (expanded) and raises `NavigationPaneOpenChanged`. Hence the pane state is
  modelled twice — *desired* (`nav_expanded`, mirrors the config) and
  *declared* (`pane_declared`, handed to the framework) — and the framework's
  startup event must never be written to the config. reactor's diff only
  calls the setter when the declared value changes, so restoring a collapsed
  pane needs the declared value to yield once (to the native value) before the
  effect pulls it back to the desired value.
- **The reactor dependency is pinned to a git revision, not to crates.io.**
  `windows-reactor 0.100.0` on crates.io is a different snapshot
  (`src/core/...` layout) and does **not** contain `on_pane_open_changed`;
  only the pinned git revision `a48e42e` (PR #4782) carries it. Do not switch
  to the published crate without re-checking that API.
- **The reactor icon API changed in PR #4782.** `Icon::bitmap(uri)` is gone;
  use `Icon::image(uri)` for a full-colour SVG/ImageIcon and
  `Icon::bitmap_icon(uri, show_as_monochrome)` for a WinUI BitmapIcon.
- **An SVG `ImageIcon` fills the space it is given.** Inside a `Button` that
  has no explicit size, the icon stretches and the button grows to the whole
  row/column. Icon buttons therefore carry an explicit `.height(...)` and
  `.padding(...)`; do not remove them thinking they are cosmetic.
- **A `{ThemeResource}`-based Style does not retry.** If the referenced brush
  is not registered before the first render, loading the Style fails and the
  element stays unstyled forever. The sidebar background therefore uses a
  literal light/dark `Color` instead of `AudioRouterSidebarBackground`.
- **Hotkey capture must use a low-level keyboard hook (`WH_KEYBOARD_LL`),** not a
  window procedure. WinUI 3 delivers keyboard messages to the *focus* window
  (a hidden helper window), and the first Alt press puts the system into
  "menu activation" mode, where the next key press is eaten by the system menu
  — so `Ctrl+Alt+M` never reaches the app the first time. The LL hook runs
  before message dispatch, sees every key, and tracks the modifier state itself
  (`app_core::hotkey::modifier_bit`) instead of relying on `GetKeyState`.
  See `keyboard_hook()` / `install_capture_hook()` in
  `audiorouter/src/window_utils.rs`. The window-procedure path is kept only as a
  fallback.
- **A subclassed window may be subclassed only once, and every subclassed
  window needs its own original-wndproc slot.** `SubclassRegistry` (in
  `audiorouter/src/window_utils.rs`) keeps one original window procedure per
  subclassed window; the production code uses the global `REGISTRY`
  instance, tests must use their own instance (Rust runs tests in parallel, and
  the registry is not safe to share across threads). Subclassing the same window
  twice makes `SetWindowLongPtrW` return *our own* `subclass_wndproc`;
  saving that as the "original" makes the window procedure call itself forever
  → stack overflow → `STATUS_FATAL_USER_CALLBACK_EXCEPTION` (0xc000041d)
  kills the process. Hiding and re-showing the window makes WinUI 3 rebuild the
  keyboard-focus window, and the focus switching back to the previous window is
  exactly what triggered the double subclassing. `subclass_window()`
  therefore skips windows already in the table, `restore_subclass()` only
  restores a window whose current wndproc is still ours (guarding against
  HWND reuse), and `subclass_wndproc` falls back to `DefWindowProcW`
  when a window has no recorded original.
- **The main window handle is found by enumerating top-level windows, but only
  ones owned by this process.** `find_hwnd()` caches the handle and
  validates it with `IsWindow`, not `IsWindowVisible`: a hidden
  window is still a valid handle, so the cache survives hide/show and there is
  no full `EnumWindows` on every call while the window is hidden.
  `enum_callback` filters by process id — another process may well have a
  window titled "AudioRouter", and subclassing someone else's window would be
  very bad. Getting the handle from the framework instead
  (`IWindowNative::WindowHandle`) is tracked in microsoft/windows-rs#5388;
  `main.rs`'s single-instance activation deliberately looks across
  processes and therefore must **not** use the PID filter.
- **`FindWindowW` 找不到 WinUI 3 的主窗口，必须用 `EnumWindows` +
  `GetWindowTextW`。** 实测：WinUI 3 窗口的类名是
  `WinUIDesktopWin32WindowClass`，标题确实是 `AudioRouter`，但
  `FindWindowW(NULL, "AudioRouter")` 一律返回 0；同一时刻
  `EnumWindows` 遍历 + `GetWindowTextW` 却能正常找到它。
  拆分进程时代的托盘「显示面板」正是栽在这里：`launcher::activate_existing()`
  找不到窗口 → 又去拉起第二个进程 → 第二个进程同样找不到 → 激活不了就
  退出 → 用户点了托盘图标**什么都不会发生**。
  现在窗口查找统一走 `window_utils::find_hwnd()`（`EnumWindows`）；
  第二实例唤醒改用**托盘窗口的类名**
  （`FindWindowW("AudioRouterServiceWindow", NULL)`）——那是普通 Win32
  窗口，`FindWindowW` 对它是准的。
- **ureq 3 的默认 TLS provider 是 rustls，不是 native-tls。**
  `native-tls` 特性只是「另外可选」，`TlsProvider::default()` 恒为
  `TlsProvider::Rustls`。少了 `rustls` 特性，访问 https 会在 ureq 内部
  panic（`provider is Rustls but feature is not enabled`）。以前是靠
  `app_core` 里那份带默认特性的 ureq 顺带打开的；app_core 不再依赖
  ureq 之后，必须在真正用到它的 crate 上显式开 `rustls`。
- **Writing state during a render does not schedule another render.** A
  `SetState::call` inside the render body updates the value but the UI keeps
  showing the previous frame, so a "clear this message" write can leave stale
  text on screen. Derive the text for *this* frame from the data at hand (e.g.
  the hotkey capture result) instead of relying on a follow-up render.
- **The registered hotkey opens the window, it does not toggle it.** The
  handler calls `window_utils::show_window()`; the tray menu keeps the
  show/hide toggle (`toggle_window_visibility()`). `app_core::hotkey::describe`
  renders a captured combination (`Ctrl+Alt+M`) and `is_modifier_key` rejects
  modifier-only input.
- **`ElementExt::tooltip(text)` never shows anything in WinUI 3.** The
  reactor hands `ToolTipService.SetToolTip` a *boxed* string
  (`IReference<HSTRING>`), but WinUI 3's `ToolTipService::ConvertToToolTip`
  only recognises an `IToolTip` instance or another renderable object — a
  boxed string is dropped into `ToolTip.Content` untouched, so the tooltip
  stays empty and is never shown (no error, no popup window). Use the rich
  path instead: `tooltip_with(Tooltip::rich(text_block(...)))` (see
  `hover_hint()` in `audiorouter/src/app.rs`) creates a real `ToolTip` control
  whose content is a `TextBlock`; that renders correctly in light and dark
  themes.
- **PowerShell 里 `$env:VAR` 后面紧跟 `:` 会被吞进变量名。**
  双引号中的 `"$env:RELEASE_TAG:refs/tags/..."` 被解析成变量
  `RELEASE_TAG:refs`（冒号并入了名字），展开为空串，refspec 变成
  `+refs/tags//tags/v0.3.15`，git 报 `invalid refspec`。凡是要在
  `$env:VAR` 后面紧跟冒号，必须写成 `$($env:VAR)`；`release-windows.yml`
  的 `git fetch` refspec 就是这么写的。
- **CLI builds must be on Windows** — `windows-sys` and `windows`
  are pulled unconditionally; cross-compilation is not configured.
- **WASAPI 的 `hnsBufferDuration` 单位是 100ns，不是毫秒。** 1 秒 =
  10,000_000。采集端和渲染端都写 `500_000`（= 50ms）。写成
  `50_000_000` 会让引擎按 **5 秒** 分配代理缓冲：每个音频客户端白白占
  约 1.9 MB 私有内存，路由延迟也跟着涨好几秒。引擎回报的
  `GetBufferSize()`（帧数）可以直接验证——50ms × 48kHz = 2400 帧，
  写错时是 240000 帧。
- **共享模式下别无脑开 `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM`。** 开着会让
  引擎为每个渲染客户端建立采样率/位深转换器（额外 CPU 与延迟）。
  Windows 的共享模式混音格式在同一台机器上通常是统一的
  （48kHz / 32bit / 2ch EXTENSIBLE），此时只传格式、不传转换标志即可走
  直通路径。比对逻辑在 `audio_core::com_service::router::formats_equivalent`；
  直通被设备拒绝时会自动回退到带转换的写法（invalidated 不回退）。
- **跨线程命令只有一条通道。** UI 请求、托盘菜单、全局热键、设备变化、
  路由事件全部汇进 `app_core::bus::Command` 这条 mpsc 通道，服务线程是
  唯一的消费者。这样所有状态变更都发生在同一个线程上，不必为「谁在改
  状态」操心；服务线程回推状态走 `StatePublisher` 回调。
- **配置只有一份，UI 线程与服务线程共享同一个 `ConfigManager`**
  （内部是 `Arc<RwLock<Config>>`）。UI 落盘后服务线程立刻读到最新值，
  不再有「谁先写盘」的竞态，也不用各自去读 `settings.toml`。
  `ConfigManager` 因此必须保持 `Clone`（克隆共享同一份内存状态）。
- **只有一个 exe**（`AudioRouter.exe`），不再有「同目录找另一个 exe」的
  逻辑。打包脚本只检查这一个文件，`installer/AudioRouter.iss` 的
  `[InstallDelete]` 会清掉旧版本遗留的 `winui3_gui.exe` /
  `audiorouter_service.exe`。
- **关窗口 = 隐藏到托盘（默认）。** `window_utils` 拦截 `WM_CLOSE` 并
  `ShowWindow(SW_HIDE)`，而不是销毁窗口——`Application::Start` 一个进程
  只能调一次，窗口一旦销毁就再也叫不回来了。`close_to_tray = false` 时
  才让框架走正常关闭流程，UI 线程退出 → 整个进程退出。
- **UI 线程是按需创建的。** 「启动时最小化」（含开机自启的 `--minimized`）
  时不创建窗口，WindowsAppSDK 与 D3D 合成器也就不会被加载进来，
  ~120 MB 常驻内存不会白吃。托盘「显示」时才 `ui_host::ensure_ui_thread()`
  把 UI 线程与窗口建起来。**别在托盘模式下无条件启动 UI 线程**，否则
  「低内存常驻」就没了。
- **路由启停不要放主线程。** `Router::start()` 最多等 5 秒才等到音频 worker
  就绪，放在主线程会把托盘卡死。服务线程就是为这件事存在的。
- **显隐必须回到窗口所属的线程。** 托盘线程只 `PostMessage`
  `WM_APP_SHOW` / `WM_APP_HIDE`，真正的 `ShowWindow` 在 UI 线程的窗口
  过程里执行；跨线程直接 `ShowWindow` 不可靠。
- **内存占用基线（2026-11 实测，release 构建，WinUI 3 自包含部署）。**
  空闲态约 123 MB 工作集 / 105 MB 私有字节，其中绝大部分是 Windows App
  SDK（`Microsoft.ui.xaml.dll` 14 MB + `Microsoft.UI.Xaml.Controls.dll`
  6 MB）与 D3D 合成器，属于框架固有开销。**应用自己能控的部分是音频
  客户端**：缓冲时长修好后，每个渲染客户端约 20 KB，路由整体只比空闲态
  多约 1 MB，输出设备数量对内存几乎不再产生影响。

## Release Process

Release is driven by `.github/workflows/release-windows.yml`, which triggers on
pushing a `v*` tag and uploads an Inno Setup installer to a GitHub Release.

1. Bump `version` in `audiorouter/Cargo.toml` to the next patch version — it
   must match the tag, otherwise "check for updates" compares the wrong
   version (the GUI reads it from `CARGO_PKG_VERSION`).
2. Commit the bump together with the code changes.
3. `git push origin main` (CI: `lint` 与 `test` 两个并行 job)。
4. `git tag v<version> && git push origin v<version>` → the release workflow
   builds `cargo build --locked --release`, so `Cargo.lock` must be
   committed and consistent with the manifests.
   发布产物是**两个安装包**：自包含（`AudioRouter-Setup-<ver>-x64.exe`）与
   框架依赖（`AudioRouter-Setup-<ver>-x64-fd.exe`）。workflow 先编两个二进制，
   再由 `scripts/build-installer.ps1` 自己下载并校验 WindowsAppRuntime 运行时
   （nupkg 有 actions/cache），最后用 Inno Setup 打包。运行时缺任何一个
   文件都会让打包步骤失败，不会再发出"只有 exe"的坏包。
   签名步骤排在两个包都生成之后（早于它的话框架依赖包会被漏签）。
   自包含包缺失 = 发布失败；框架依赖包缺失只告警，不阻断发布。
5. tag 推送触发的发布失败时，不必删 tag 重来：修好 workflow 后用
   `gh workflow run release-windows.yml -f tag=v<version>` 手动重跑即可。
   手动触发用的是 **main 分支上的 workflow 文件**，但构建内容仍然取自
   tag（`Checkout release tag` 步骤会 fetch 并 checkout 该 tag）。

已发布的版本历史见 `git tag --sort=-creatordate`。

## CI 与 Release 工作流

两个工作流的设计目标：把「跑完一次」从分钟级压下来，并让编译缓存长期有效。
改造前的实测耗时：CI 188s、Release 505s（其中 413s 花在 release 编译上）。

- **工具链固定在 1.100.0**（见 `rust-toolchain.toml`，两个 workflow 里显式写
  同样的 `toolchain:` 输入）。缓存键包含 rustc 版本，用 `@stable` 的话一旦
  Rust 发新版本，sccache / rust-cache 的指纹全部失效，两个工作流都会退化成
  全量重编。升级工具链时要同时改 `rust-toolchain.toml` 和两个 workflow 的
  `toolchain` 输入。
- **编译复用交给 sccache，缓存只留依赖源码**。`Swatinem/rust-cache` 设
  `cache-targets: false`，只缓存 `~/.cargo/registry` 与 `~/.cargo/git`；
  之前每次 restore/save 要搬运 GB 级 target 目录（CI 里合计 97s/188s）。
  dev/test profile 在 CI 里用 `CARGO_PROFILE_DEV_DEBUG=0` /
  `CARGO_PROFILE_TEST_DEBUG=0` 关掉调试信息，产物体积小一个数量级。
- **CI 拆成 `lint` 与 `test` 两个并行 job**，墙钟时间取两者最大值，
  代价是 runner 分钟数上升。
- **Release 走 sccache + thin LTO**：CI 里用
  `CARGO_PROFILE_RELEASE_LTO=thin` 和
  `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16` 覆盖。仓库默认的
  `lto = true` + `codegen-units = 1`（fat LTO 单 codegen unit，链接阶段
  无法并行）是发布流程最大的耗时点，但默认 profile 保持不变，本地发版仍走
  fat LTO。
- **Release 用浅克隆**（`fetch-depth: 1`），tag 由
  `git fetch --depth 1 origin +refs/tags/<tag>` 单独抓取，
  `workflow_dispatch` / `workflow_call` 手动触发同样可用。
- **tag 解析用 `inputs.tag`**，同时覆盖 `workflow_dispatch` 与
  `workflow_call`；旧写法 `github.event.inputs.tag` 在 `workflow_call`
  下取不到值。
- **`sccache-action` 固定在 v0.0.11，并且必须能连上 Cache Service v2。**
  v0.0.8 不会导出 `ACTIONS_CACHE_SERVICE_V2=on`，sccache 于是去用已被
  GitHub 下线的 Cache API v1：每次写入都失败（`cache_write_errors` 等于
  编译数）、统计里 `effective_cache_mode` 变成 `ReadOnly`、Actions 缓存列表
  里一条 sccache 条目都留不下，命中永远是 0 —— 只剩每次编译多一次远端
  查找的开销。v0.0.11 起会导出该变量，workflow 里也显式写了
  `ACTIONS_CACHE_SERVICE_V2: "on"`，防止 action 版本回退。
  修好后实测：CI 两个 job 命中 126~128 次、miss 只剩 6~7 次（修复前是 0 命中）。
  该版本只认 `version` / `token` / `disable_annotations` 三个输入，没有
  `fail-on-cache-err`（写了是空转，每轮还会刷一条 `Unexpected input` 警告）。

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

To produce an installer:

```powershell
# 自包含（默认发布形态）：脚本自己下载并校验 WindowsAppRuntime 运行时
.\scripts\build-installer.ps1 -Version 0.4.0

# 框架依赖变体：exe 必须用 framework-dependent 特性编译
$env:CARGO_TARGET_DIR = "target\fd"
cargo build --release -p audiorouter --features audiorouter/framework-dependent
.\scripts\build-installer.ps1 -NoBuild -Version 0.4.0 -DeploymentMode framework-dependent -CargoTargetDir "target\fd"
```

打包脚本会校验"指定的部署模式"与"exe 实际形态"一致，不一致直接失败；
再校验运行时清单逐条命中、14 个关键文件在位。任何一项不过都 `exit 1`，
不会产出坏包。

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
