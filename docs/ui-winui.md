# WinUI 3 界面：设计与改动记录

> 侧边栏、主页、设置页、托盘、首次启动语言，以及一次把进程打挂的崩溃。
> 原则性的约束见 [invariants.md](invariants.md)。

## 2026-11-18：Four UI complaints fixed in one pass

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

## 2026-11-17：Main window, settings page and tray usability pass (v0.3.13)

- **Main window**: the status line became a semantic `InfoBar` (Ok / Warning /
  Error), the source device combo shows a placeholder until a device is picked,
  every output row has a **试听** button (plays a test tone on that device from
  a background thread), the selected channel mode is spelled out under the row
  (since 2026-11-18 it is a hover tooltip instead), active outputs are marked "● 输出中", and the
  header shows how many outputs are checked.
- **Settings page**: the page scrolls, saving a bad global hotkey now shows an
  inline `InfoBar` instead of failing silently, unsaved edits are flagged, and a
  **诊断** section opens the config / log folders and shows the version.
- **Tray**: a status line plus enabled/disabled start/stop entries and a
  tooltip mirror the router state; the menu is only rewritten when the state
  actually changes.
- **Navigation**: the framework's own pane-toggle button has no event
  callback, so the whole pane is now drawn by hand (see 2026-11-18); the
  expanded state is persisted in `settings.toml`.
- **Background failures**: no balloon API in the tray, so a failed route
  flashes the window (`FLASHW_ALL`) when the window is hidden.

## 2026-11-16：Tray hide → show no longer kills the app

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

实现细节见 `SubclassRegistry`（`audiorouter/src/window_utils.rs`），
不变量见 [invariants.md](invariants.md#四线程窗口与托盘)。

### First-launch language now follows the system UI language

- `app_core::i18n::detect_system_locale()` reads the user's UI language via
  `GetUserDefaultUILanguage` — the same source Inno Setup uses with its default
  `LanguageDetectionMethod=uilanguage` — and maps it to `zh` / `en`.
- `ConfigManager::load_with_default_language()` applies that language **only**
  when `settings.toml` does not exist yet. An existing config is never
  rewritten, so a language picked in the settings UI survives restarts and
  upgrades.
