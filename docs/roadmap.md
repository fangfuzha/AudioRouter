# 长期待办与已知缺口

> `TODO.md` 之外的长期项。按 AGENTS.md 的 `High-Priority Remaining Work`
> 整理，并补上文档拆分时同步记录的其他缺口。

## 1. TODO.md items

- "Update logic added, not verified" — the download half is now covered
  by `update::tests::downloads_latest_installer` (`#[ignore]`, needs
  network). Run `cargo test -p audiorouter --bin AudioRouter -- --ignored`
  to exercise fetch + download against the real latest release. The
  installer handoff itself (`launch_installer_and_quit`) still needs a
  human pass: it installs over the running build.
- Multi-channel routing (5.1 → stereo downmix) is out of scope per
  the existing `decode_channel_mask` parsing in
  `audio_core/src/utils.rs`.

## 2. Single `windows` version

`windows-reactor` git rev is the
last holdout pulling the workspace's `windows 0.62` line; pin or
vendor once a stable release is published.

## 3. 窗口句柄改由框架提供

上游 issue microsoft/windows-rs#5388：`windows-reactor` 暴露
`ReactorHost::hwnd()`（`IWindowNative::WindowHandle`）与
`app_window()`（`IWindow2::AppWindow`），之后可以停用
`EnumWindows` + 标题匹配，并进一步用 `AppWindow.Hide()/Show()`
取代裸 `ShowWindow`。当前 reactor 手写的 WinAppSDK 绑定还没有
`Hide()` / `Show()`，需要上游补齐。

## 4. 打包流程里的重复劳动

`scripts/build-installer.ps1` 现在自己下载并校验运行时（见
[packaging.md](packaging.md)）。`build.rs` 里 `as_self_contained()` 的
拷贝副作用已不再被依赖，但仍然在往 `target/<profile>/` 里写文件——sccache
命中时它不执行，行为不稳定，迟早要清掉。
