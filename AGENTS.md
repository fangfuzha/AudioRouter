# AGENTS.md

> Canonical agent guide for the `AudioRouter` workspace.
> Last refreshed: 2026-08-21 (post CI fix + windows 0.62 alignment).

## Project Goal

Lightweight Windows desktop tool that **routes audio from one Windows output
device to multiple other output devices simultaneously** (think
"multi-room from a single source"). Native Rust + WinUI 3 — no WebView,
no Node.js.

Primary binary: `winui3_gui` (`cargo run -p winui3_gui`).

## Architecture Map

Workspace members (`Cargo.toml`):

| crate         | role                                                            | depends on                                          |
|---------------|-----------------------------------------------------------------|-----------------------------------------------------|
| `audio_core`  | WASAPI loopback capture + multi-render, device enum, hot-plug   | `config`, `windows 0.62`, `windows-core 0.62`       |
| `config`      | Atomic `settings.toml` read/write + schema validation           | `serde`, `toml 1`                                   |
| `app_core`    | `AppController`, i18n, autostart, window placement registry    | `audio_core`, `config`, `windows 0.62`              |
| `winui3_gui`  | WinUI 3 frontend (windows-reactor), tray, single-instance, proxy | `app_core`, `audio_core`, `windows-reactor` (git)  |

Dependency direction is one-way: `winui3_gui → app_core → audio_core → config`.
Reverse edges are not allowed.

Hot path: `router::worker::event_loop` reads from the loopback capture
client and writes to N render clients on a 3ms timer; see
`audio_core/src/router/worker.rs`.

## Source-of-Truth References

- Routing engine: `audio_core/src/router/{mod,worker,config,state}.rs`
- Device enumeration: `audio_core/src/com_service/{device,watcher}.rs`
- Controller state machine: `app_core/src/controller.rs`
- Config schema: `config/src/config.rs`
- WinUI app entry: `winui3_gui/src/main.rs`
- Update + proxy hot-reload: `winui3_gui/src/update.rs`
- Tray icon: `winui3_gui/src/tray.rs`
- Background event sources (device hot-plug / router events / tray): `winui3_gui/src/events.rs`
- Window placement + close-to-tray hooks: `winui3_gui/src/window_utils.rs`
- Installer script: `scripts/build-installer.ps1`

For end-user docs see `README.md`; for short-term todos see `TODO.md`.

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
  GitHub proxies (`MIRROR_PREFIXES` in `winui3_gui/src/update.rs`).
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
  upgraded to v0.0.8 with `fail-on-cache-err: false` to ride out ghac
  (artifactcache) jitter.
- Clippy + fmt + tests all clean at workspace level.

## High-Priority Remaining Work

1. **TODO.md items**:
   - "Update logic added, not verified" — the download half is now covered
     by `update::tests::downloads_latest_installer` (`#[ignore]`, needs
     network). Run `cargo test -p winui3_gui --bin winui3_gui -- --ignored`
     to exercise fetch + download against the real latest release. The
     installer handoff itself (`launch_installer_and_quit`) still needs a
     human pass: it installs over the running build.
   - "Icon replacement" — swap `assets/icon.ico` and
     `assets/icon.png` (used by `build.rs` `winres` and tray fallback).
     The current assets are technically fine (PNG has alpha, the ICO carries
     16/32/48/256); this is an art-direction task, so it needs a design
     decision rather than a mechanical swap.
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
  `winui3_gui/src/window_utils.rs`. The window-procedure path is kept only as a
  fallback.
- **A subclassed window may be subclassed only once, and every subclassed
  window needs its own original-wndproc slot.** `SUBCLASSED_HWNDS` /
  `SUBCLASSED_ORIGS` (in `winui3_gui/src/window_utils.rs`) keep one
  original window procedure per subclassed window. Subclassing the same window
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
  (`IWindowNative::WindowHandle`) is tracked in issue #1;
  `main.rs`'s single-instance activation deliberately looks across
  processes and therefore must **not** use the PID filter.
- **Writing state during a render does not schedule another render.** A
  `SetState::call` inside the render body updates the value but the UI keeps
  showing the previous frame, so a "clear this message" write can leave stale
  text on screen. Derive the text for *this* frame from the data at hand (e.g.
  the hotkey capture result) instead of relying on a follow-up render.
- **The registered hotkey opens the window, it does not toggle it.** The
  handler calls `show_and_focus_window()`; the tray menu keeps the
  show/hide toggle. `app_core::hotkey::describe` renders a captured
  combination (`Ctrl+Alt+M`) and `is_modifier_key` rejects modifier-only input.
- **`ElementExt::tooltip(text)` never shows anything in WinUI 3.** The
  reactor hands `ToolTipService.SetToolTip` a *boxed* string
  (`IReference<HSTRING>`), but WinUI 3's `ToolTipService::ConvertToToolTip`
  only recognises an `IToolTip` instance or another renderable object — a
  boxed string is dropped into `ToolTip.Content` untouched, so the tooltip
  stays empty and is never shown (no error, no popup window). Use the rich
  path instead: `tooltip_with(Tooltip::rich(text_block(...)))` (see
  `hover_hint()` in `winui3_gui/src/app.rs`) creates a real `ToolTip` control
  whose content is a `TextBlock`; that renders correctly in light and dark
  themes.
- **CLI builds must be on Windows** — `windows-sys` and `windows`
  are pulled unconditionally; cross-compilation is not configured.

## Release Process

Release is driven by `.github/workflows/release-windows.yml`, which triggers on
pushing a `v*` tag and uploads an Inno Setup installer to a GitHub Release.

1. Bump `version` in `winui3_gui/Cargo.toml` to the next patch version — it
   must match the tag, otherwise "check for updates" compares the wrong
   version (the GUI reads it from `CARGO_PKG_VERSION`).
2. Commit the bump together with the code changes.
3. `git push origin main` (CI: `lint` 与 `test` 两个并行 job)。
4. `git tag v<version> && git push origin v<version>` → the release workflow
   builds `cargo build --locked --release`, so `Cargo.lock` must be
   committed and consistent with the manifests.

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

## Validation Commands

Run from `D:\my_project\AudioRouter`:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
cargo build -p winui3_gui                  # debug
cargo build --release -p winui3_gui        # release
```

CI 在 `windows-latest` 上跑 `cargo fmt` + `cargo clippy`（`lint` job）
与 `cargo test`（`test` job），两者并行，见 `.github/workflows/ci.yml`。
CI 不再单独跑 `cargo check`：`cargo clippy --all-targets` 已经覆盖同样的
类型检查范围，而 `check` 只产出 metadata、`test` 需要全量 codegen，
两者产物不共享，等于白编一轮依赖。

To produce an installer:

```powershell
.\scripts\build-installer.ps1 -Version 0.4.0
```

## Conventions

- Use `parking_lot` (not `std::sync::Mutex`) when adding shared state
  in `winui3_gui` or `app_core`.
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
