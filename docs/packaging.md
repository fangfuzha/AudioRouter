# 打包与部署

> 安装包制作、部署模式、WindowsAppRuntime 版本钉死。
> 原则性的约束见 [invariants.md](invariants.md)。

## v0.3.12~v0.3.16 坏包的根因与修法

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

## 两种部署模式

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

## WindowsAppRuntime 版本

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

## 打包命令

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
