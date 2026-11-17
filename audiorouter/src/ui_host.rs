//! UI 线程宿主：按需创建控制面板窗口。
//!
//! 控制面板跑在自己的 STA 线程上——WinUI 3 的硬性要求：UI 线程必须是单线程
//! 公寓，创建 XAML 对象之前还要先 `CoInitializeEx(COINIT_APARTMENTTHREADED)`。
//! reactor 的 `App::run()` 会在**调用它的线程**上初始化公寓并接管消息泵，
//! 所以这里把 `App::run()` 放到一条专门 spawn 出来的线程上即可。
//!
//! 这里做两件事：
//!
//! 1. **只启动一次**：托盘「显示」是唯一会拉起 UI 线程的入口，反复点击
//!    不会创建第二条 UI 线程；
//! 2. **按需启动**：开机自启（`--minimized`）时只有托盘与路由在跑，
//!    根本不加载 WindowsAppSDK——那 ~120 MB 的框架开销就不会在开机时白白
//!    吃掉；等用户第一次点托盘图标时才把窗口建起来。

use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Result;
use parking_lot::Mutex;
use windows_reactor::{App, Backdrop, InnerConstraints};

use app_core::bus::{Command, Request};
use app_core::controller::AppController;

use crate::events::RenderSignal;

/// UI 线程宿主。托盘线程通过全局 `UI_HOST` 拿到它。
pub struct UiHost {
    controller: Arc<Mutex<AppController>>,
    render: RenderSignal,
    cmd_tx: Sender<Command>,
    /// 是否已经启动过 UI 线程。
    started: Mutex<bool>,
}

impl UiHost {
    pub fn new(
        controller: Arc<Mutex<AppController>>,
        render: RenderSignal,
        cmd_tx: Sender<Command>,
    ) -> Self {
        Self {
            controller,
            render,
            cmd_tx,
            started: Mutex::new(false),
        }
    }

    /// 确保 UI 线程已经启动（幂等）。可以在任意线程上调用。
    pub fn ensure_started(&self) {
        {
            let mut started = self.started.lock();
            if *started {
                return;
            }
            *started = true;
        }

        let controller = Arc::clone(&self.controller);
        let render = self.render.clone();
        let cmd_tx = self.cmd_tx.clone();

        let spawned = std::thread::Builder::new()
            .name("audiorouter-ui".to_string())
            .spawn(move || {
                if let Err(e) = run_ui(controller, render, cmd_tx) {
                    log::error!("UI 线程退出: {e}");
                }
            });

        match spawned {
            Ok(_) => log::info!("UI 线程已启动"),
            Err(e) => {
                // 启动失败就允许下次再试。
                *self.started.lock() = false;
                log::error!("启动 UI 线程失败: {e}");
            }
        }
    }
}

/// 全局 UI 宿主。
static UI_HOST: OnceLock<Arc<UiHost>> = OnceLock::new();

/// 安装全局 UI 宿主（main 里调用一次）。
pub fn install(host: Arc<UiHost>) {
    let _ = UI_HOST.set(host);
}

/// 确保 UI 线程已经启动（任意线程可调用）。没安装宿主时什么都不做。
pub fn ensure_ui_thread() {
    if let Some(host) = UI_HOST.get() {
        host.ensure_started();
    }
}

/// UI 线程主体：建窗口、跑消息泵。
fn run_ui(
    controller: Arc<Mutex<AppController>>,
    render: RenderSignal,
    cmd_tx: Sender<Command>,
) -> Result<()> {
    // 控制器要拿到一次当前状态（设备列表、路由状态）。
    {
        let mut c = controller.lock();
        c.init();
    }

    let app = build_app(&controller)?;
    let result = app.run({
        let controller = Arc::clone(&controller);
        let render = render.clone();
        move || crate::app::RootComponent::new(Arc::clone(&controller), render.clone())
    });

    // 窗口被真正销毁（框架退出）时，整个应用一起收摊。
    // 「关闭到托盘」开启时窗口只是被隐藏，`App::run()` 不会返回，
    // 所以正常关窗口不会走到这里。
    let _ = cmd_tx.send(Command::Ui(Request::Quit));
    Ok(result?)
}

/// 按配置创建 WinUI 应用对象。
fn build_app(controller: &Arc<Mutex<AppController>>) -> Result<App> {
    // 从配置读取初始 backdrop，在窗口创建时直接应用。
    // 必须通过 App::backdrop 在窗口创建阶段设置，而非在组件 use_effect 中
    // 事后调用 set_backdrop——后者依赖的 ROOT_WINDOW 在 UI 首次挂载后才设置，
    // use_effect 执行时机可能早于该设置，导致 backdrop 被静默丢弃。
    let initial_backdrop = {
        let c = controller.lock();
        c.backdrop()
    };
    let reactor_backdrop = match initial_backdrop {
        config::config::Backdrop::Mica => Backdrop::Mica,
        config::config::Backdrop::MicaAlt => Backdrop::MicaAlt,
        config::config::Backdrop::Acrylic => Backdrop::Acrylic,
    };

    let icon_path = crate::resolve_asset_path("assets/icon.ico");

    let mut app = App::new()
        .title("AudioRouter")
        .inner_size(980.0, 720.0)
        .inner_constraints(InnerConstraints {
            min_width: Some(640.0),
            min_height: Some(480.0),
            ..Default::default()
        })
        .backdrop(reactor_backdrop);

    if icon_path.is_file() {
        app = app.icon(icon_path.to_string_lossy().to_string());
    } else {
        log::warn!("Window icon not found: {}", icon_path.display());
    }

    Ok(app)
}
