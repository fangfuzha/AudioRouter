# 开发指南

> 面向开发者的上手与日常开发说明。终端用户请看 [README](../README.md)。
> 原则性的约束见 [invariants.md](invariants.md)，架构细节见
> [architecture.md](architecture.md)，打包见 [packaging.md](packaging.md)。

## 开发环境要求

- Rust toolchain（版本固定在 `rust-toolchain.toml`，CI 与本地都用 1.100.0）
- Windows App SDK（WinUI 3）
- 可选：Inno Setup 6（只在打安装包时需要）

项目是 **Windows-only**：直接绑定 Win32 + WinUI 3 的 `windows` /
`windows-reactor` crate，没有 WebView、没有 Node.js。

## 快速开始

```bash
# 运行（debug，日志输出到 stderr）
cargo run --package audiorouter

# 构建 release
cargo build --release --package audiorouter
```

生成的 exe 位于 `target/release/AudioRouter.exe`。托盘、路由、控制面板全在
它里面 —— 这是唯一可执行文件。

## 项目结构

```
audio_core/        # 音频核心库（设备枚举、路由引擎）
config/            # 配置管理（settings.toml）
app_core/          # 应用核心逻辑（控制器、i18n、自动启动、进程内命令总线）
audiorouter/       # 唯一入口：托盘（主线程）+ 路由（服务线程）+ WinUI 3 控制面板（UI 线程）
installer/         # Inno Setup 安装脚本 + 运行时文件清单
scripts/           # 构建与打包脚本
assets/            # 共享资源（图标等）
docs/              # 开发者文档（本目录）
```

crate 之间的依赖方向是单向的：`audiorouter → app_core → audio_core → config`，
反向依赖不允许。

## 开发工作流

```bash
# 增量开发
cargo run -p audiorouter                # debug 模式运行，日志输出到 stderr
cargo build --release -p audiorouter    # release 模式构建

# 静态检查（CI 也跑这几步）
cargo fmt --check                                        # 格式
cargo clippy --workspace --all-targets -- -D warnings   # lint
cargo test --workspace --lib                             # 单元测试

# 打安装包（详见 packaging.md）
.\scripts\build-installer.ps1 -Version 0.4.0
```

CI 在 `windows-latest` 上跑 `lint` 与 `test` 两个并行 job，外加一个
`size-guard` job 阻断大文件提交。发版流程见 [release.md](release.md)。

## 调试技巧

- **release 模式日志**：`%LOCALAPPDATA%\AudioRouter\logs\AudioRouter.log`，
  超过 5 MiB 自动 rotate 到 `.old`。
- **dev 模式日志**：直接走 stderr（因为有控制台）。
- **音频路由卡死**：log 里搜 `invalidated` / `Restart attempt`，通常意味着
  设备热插拔或格式切换，worker 会自动重试 10 次。
- **单实例**：首次启动会创建 `Local\AudioRouter-SingleInstance-Mutex`，第二次
  启动会通过托盘窗口类名找到常驻实例，让它把控制面板叫起来后自己退出。
- **更新链路**：`cargo test -p audiorouter --bin AudioRouter -- --ignored` 会真实
  拉取最新 release 的安装包并校验字节数（需要联网）。

## 平台差异

- 项目目前 **Windows-only**（用 `windows` / `windows-reactor` crate 直接绑
  Win32 + WinUI 3）。
- 跨平台编译 `cfg(windows)` guard 保证只拉 Windows 依赖；其他平台理论上能
  编译但跑不起来。
- 部分集成测试（`com_service::device::tests::device_api_behaves`、
  `device_watcher::tests::test_watcher_start_and_stop` 等）需要真机音频设备，
  默认 `#[ignore]`，需要真机调试时手动 `cargo test -- --ignored` 运行。

## 贡献

- 修改前先 `cargo fmt` + `cargo clippy --workspace --all-targets`。
- 新增配置字段记得在 `Config::validate` 里加相应检查；旧的 settings.toml
  要保证能加载（用 `#[serde(default)` 兜底）。
- 升级 `windows` crate 时，**`#[implement]` 宏生成的 wrapper 类型名是
  `<原名>_Impl`**（0.58+），手动 `impl` 块必须写在 wrapper 上，不能写在原名上。
- 跨线程传 COM 指针请用 `ComSend<T>`，它强制 `T: Send`（2026-08 收紧），
  不要用裸 `*mut ...`。
- **不要提交大文件（> 1MB）**：`scripts/check-large-files.ps1` 会在 CI 与发版
  流程里阻断，例外登记在 `scripts/large-files-allowlist.txt`。

短期待办见 [../TODO.md](../TODO.md)，长期待办见 [roadmap.md](roadmap.md)。
