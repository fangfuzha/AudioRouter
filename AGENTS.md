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
| `app_core`    | `AppController`, i18n, autostart, GitHub Release update check    | `audio_core`, `config`, `windows 0.62`              |
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
- Installer script: `scripts/build-installer.ps1`

For end-user docs see `README.md`; for short-term todos see `TODO.md`.

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
   - "Update logic added, not verified" — manually exercise
     `winui3_gui::update::check_for_updates` against a real GitHub
     release to confirm download + installer handoff works.
   - "Icon replacement" — swap `assets/icon.ico` and
     `assets/icon.png` (used by `build.rs` `winres` and tray fallback).
   - Multi-channel routing (5.1 → stereo downmix) is out of scope per
     the existing `decode_channel_mask` parsing in
     `audio_core/src/utils.rs`.
2. **`apply_running_config` UX** — when stop succeeds but the
   subsequent `start_routing` fails, the user sees a stale
   `is_running = false` with no clear "previous routing was torn down"
   indicator. Consider adding an explicit "Stopped" state message.
3. **Single `windows` version** — `windows-reactor` git rev is the
   last holdout pulling the workspace's `windows 0.62` line; pin or
   vendor once a stable release is published.

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
- **CLI builds must be on Windows** — `windows-sys` and `windows`
  are pulled unconditionally; cross-compilation is not configured.

## Validation Commands

Run from `D:\my_project\AudioRouter`:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
cargo build -p winui3_gui                  # debug
cargo build --release -p winui3_gui        # release
```

CI runs the same first four commands on `windows-latest`
(`.github/workflows/ci.yml`).

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
