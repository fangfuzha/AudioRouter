# 非显然约束（Invariants）

> 这里是 AGENTS.md 里那份长清单的**全文与分组版**。每条都记录了「为什么」，
> 多数还附了踩坑现场，避免下一个人再踩一遍。
> 相关背景见 [architecture.md](architecture.md) / [packaging.md](packaging.md) /
> [ui-winui.md](ui-winui.md) / [audio.md](audio.md)。

## 一、构建与打包

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
- **只有一个 exe**（`AudioRouter.exe`），不再有「同目录找另一个 exe」的
  逻辑。打包脚本只检查这一个文件，`installer/AudioRouter.iss` 的
  `[InstallDelete]` 会清掉旧版本遗留的 `winui3_gui.exe` /
  `audiorouter_service.exe`。
- **CLI builds must be on Windows** — `windows-sys` and `windows`
  are pulled unconditionally; cross-compilation is not configured.

## 二、Rust / `windows` crate API

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
- **ureq 3 的默认 TLS provider 是 rustls，不是 native-tls。**
  `native-tls` 特性只是「另外可选」，`TlsProvider::default()` 恒为
  `TlsProvider::Rustls`。少了 `rustls` 特性，访问 https 会在 ureq 内部
  panic（`provider is Rustls but feature is not enabled`）。以前是靠
  `app_core` 里那份带默认特性的 ureq 顺带打开的；app_core 不再依赖
  ureq 之后，必须在真正用到它的 crate 上显式开 `rustls`。
- **`parking_lot::Mutex::lock()` returns a `MutexGuard` directly**,
  no `Result` — every `.lock().unwrap()` in old code became
  `.lock()`. Search-replace was already run; do not reintroduce
  `.lock().unwrap()` on `parking_lot::Mutex`.
- **`windows-reactor` `Element::from(...)` is required** in some
  builder contexts — clippy's `useless_conversion` lint is suppressed
  locally; do not strip the `#[allow]` without testing the UI.

## 三、WinUI 3 / reactor

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
  and `Element::Group` are dropped / spliced before diffing, so they are not
  a substitute for a key.
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
- **Writing state during a render does not schedule another render.** A
  `SetState::call` inside the render body updates the value but the UI keeps
  showing the previous frame, so a "clear this message" write can leave stale
  text on screen. Derive the text for *this* frame from the data at hand (e.g.
  the hotkey capture result) instead of relying on a follow-up render.

## 四、线程、窗口与托盘

- **跨线程命令只有一条通道。** UI 请求、托盘菜单、全局热键、设备变化、
  路由事件全部汇进 `app_core::bus::Command` 这条 mpsc 通道，服务线程是
  唯一的消费者。这样所有状态变更都发生在同一个线程上，不必为「谁在改
  状态」操心；服务线程回推状态走 `StatePublisher` 回调。
- **配置只有一份，UI 线程与服务线程共享同一个 `ConfigManager`**
  （内部是 `Arc<RwLock<Config>>`）。UI 落盘后服务线程立刻读到最新值，
  不再有「谁先写盘」的竞态，也不用各自去读 `settings.toml`。
  `ConfigManager` 因此必须保持 `Clone`（克隆共享同一份内存状态）。
- **路由启停不要放主线程。** `Router::start()` 最多要等 5 秒才等到音频 worker
  就绪，放在主线程会把托盘卡死。服务线程就是为这件事存在的。
- **UI 线程是按需创建的。** 「启动时最小化」（含开机自启的 `--minimized`）
  时不创建窗口，WindowsAppSDK 与 D3D 合成器也就不会被加载进来，
  ~120 MB 常驻内存不会白吃。托盘「显示」时才 `ui_host::ensure_ui_thread()`
  把 UI 线程与窗口建起来。**别在托盘模式下无条件启动 UI 线程**，否则
  「低内存常驻」就没了。
- **关窗口 = 隐藏到托盘（默认）。** `window_utils` 拦截 `WM_CLOSE` 并
  `ShowWindow(SW_HIDE)`，而不是销毁窗口——`Application::Start` 一个进程
  只能调一次，窗口一旦销毁就再也叫不回来了。`close_to_tray = false` 时
  才让框架走正常关闭流程，UI 线程退出 → 整个进程退出。
- **显隐必须回到窗口所属的线程。** 托盘线程只 `PostMessage`
  `WM_APP_SHOW` / `WM_APP_HIDE`，真正的 `ShowWindow` 在 UI 线程的窗口
  过程里执行；跨线程直接 `ShowWindow` 不可靠。
- **最小化的窗口要用 `SW_RESTORE`，`SW_SHOW` 对它无效。** `SW_SHOW` 的语义
  是「按当前大小和位置显示并激活」，而最小化窗口的「当前状态」就是最小化，
  调用后 `IsIconic` 依然为真——窗口根本不会回到屏幕。这正是「窗口最小化后
  点托盘图标没反应」的根因。托盘「显示」、热键开窗、窗口就绪时的首帧显隐
  统一走 `window_utils::show_window_handle()`：先看 `IsIconic`，最小化时用
  `SW_RESTORE`（还原到最小化**之前**的状态，普通或最大化都保得住），否则用
  `SW_SHOW`。本机实测：最小化的窗口 `ShowWindow(SW_SHOW)` 之后 `IsIconic`
  仍为 `true`，`ShowWindow(SW_RESTORE)` 之后才变成 `false`。
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
- **The registered hotkey opens the window, it does not toggle it.** The
  handler calls `window_utils::show_window()`; the tray menu keeps the
  show/hide toggle (`toggle_window_visibility()`). `app_core::hotkey::describe`
  renders a captured combination (`Ctrl+Alt+M`) and `is_modifier_key` rejects
  modifier-only input.

## 五、音频

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

## 六、脚本与 CI

- **PowerShell 里 `$env:VAR` 后面紧跟 `:` 会被吞进变量名。**
  双引号中的 `"$env:RELEASE_TAG:refs/tags/..."` 被解析成变量
  `RELEASE_TAG:refs`（冒号并入了名字），展开为空串，refspec 变成
  `+refs/tags//tags/v0.3.15`，git 报 `invalid refspec`。凡是要在
  `$env:VAR` 后面紧跟冒号，必须写成 `$($env:VAR)`；`release-windows.yml`
  的 `git fetch` refspec 就是这么写的。
- **`sccache-action` 必须能连上 Cache Service v2。** v0.0.8 不会导出
  `ACTIONS_CACHE_SERVICE_V2=on`，sccache 于是去用已被 GitHub 下线的
  Cache API v1：每次写入都失败、命中率永远是 0。v0.0.11 起会导出该变量，
  workflow 里也显式写了 `ACTIONS_CACHE_SERVICE_V2: "on"`，防止 action
  版本回退。详见 [release.md](release.md)。
- **`swatinem/rust-cache` 必须设 `cache-targets: false`。** 否则每次
  restore/save 都要搬运 GB 级 target 目录（CI 里合计 97s/188s）。
- **工具链版本要三处同步**：`rust-toolchain.toml` + 两个 workflow 里的
  `toolchain:` 输入。缓存键包含 rustc 版本，只在一处改会让所有缓存失效。
