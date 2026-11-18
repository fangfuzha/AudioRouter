# 历史重构记录

> 已经被后续版本覆盖、但仍然有参考价值的重构细节。
> 仍然生效的规则一律看 [invariants.md](invariants.md)。

## 2026-08-21：Refactor pass complete

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
