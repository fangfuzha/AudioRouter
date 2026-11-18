# 发版与 CI

> 发版流程、CI 工作流与编译缓存、更新检查的镜像回退。

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

## 更新检查的镜像回退

- **Update check uses mirror fallbacks**: the GitHub API and release assets
  are tried against the official endpoint first, then a list of prefix-style
  GitHub proxies (`MIRROR_PREFIXES` in `audiorouter/src/update.rs`).
  Override with the `AUDIO_ROUTER_GITHUB_MIRROR` environment variable.
