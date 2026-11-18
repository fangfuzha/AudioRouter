# 架构：单进程多线程

> 对应 AGENTS.md 的 `Architecture Map` 与 `Current Status (2026-11-18)`。
> 原则性的约束见 [invariants.md](invariants.md)。

## 线程分工

以前这个应用是**两个 exe**：`audiorouter_service.exe`（路由 + 托盘 + 热键）
和 `winui3_gui.exe`（WinUI 3 控制面板），靠本地命名管道通信。开发时必须
同时构建两个二进制、还得保证它们同目录，调试一次问题要跨两个进程来回看。
现在合并成**一个 exe**（`AudioRouter.exe`），同样的职责用多线程拆开。

| 线程 | 职责 | 代码位置 |
|---|---|---|
| **主线程** | 托盘图标、全局热键、Win32 消息循环 | `desktop::run()`（进程入口就停在这里） |
| **服务线程** | 路由启停、设备枚举、状态汇总 | `runtime::run()` |
| **UI 线程** | WinUI 3 控制面板（STA 公寓） | `ui_host::run_ui()`，**按需创建** |
| 音频 worker | WASAPI 采集 + 多路渲染 | `audio_core` 内部（热路径） |

## 关键设计

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
- **第二个实例靠托盘窗口类名找到常驻实例**：
  `FindWindowW("AudioRouterServiceWindow")` + `PostMessage(WM_TRAY_SHOW_PANEL)`，
  让常驻实例自己把窗口叫起来（与托盘左键点击同一条路径：
  `window_utils::show_window()`）。
- **单实例用命名互斥量实现**（`main.rs` 的 `acquire_single_instance()`，
  互斥量名 `Local\AudioRouter-SingleInstance-Mutex`，限制在当前用户会话内）。
  已有实例在跑时，第二个实例投递唤醒消息后自己退出。

## 事件驱动的 WinUI 前端（2026-11-10）

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
- `audio_core::Router::subscribe()` added so the GUI can block on router
  events instead of polling `poll_events()`.

## 历史包袱：命名管道上不能留挂起的阻塞读

（拆分进程时代踩过的坑，管道已经删除，留个记录以防将来重新引入 IPC。）

最初服务端用「读线程 + 主循环写」的模型，结果两边互等、整个服务卡死：
Windows 对同一个 file object 的**同步 I/O 会串行化**——读线程挂着阻塞
`ReadFile` 时，主循环往同一条管道 `WriteFile` 会一直等那次读返回；而读
又在等客户端的下一帧，双方互等，**死锁**。表现是「命令要等下一条命令
才生效」（启动路由被延迟十几秒）。

当时的解法是**读之前先 `PeekNamedPipe` 看有没有数据，有才读**，任何时刻
都不留挂起的读操作。现在是进程内通道，没有这个问题；但如果将来又引入
跨进程 IPC，别图省事改回阻塞读。
