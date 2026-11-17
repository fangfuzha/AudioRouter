//! 后台事件源：把常驻服务推送的状态写回控制器，并请求一次重绘。
//!
//! 设备枚举、路由启停、托盘与全局热键都发生在常驻服务进程里。服务把
//! 状态快照推过来，这里只负责把它写回控制器并触发 UI 重绘。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use app_core::controller::{AppController, StatusLevel};
use app_core::ipc::ServerMessage;
use parking_lot::Mutex;
use windows_reactor::AsyncSetState;

/// 跨线程的重绘请求句柄。
///
/// windows-reactor 的 `AsyncSetState` 写入相同值时会跳过更新，所以每次
/// 请求都写入一个新的计数值，保证重绘一定被触发。
#[derive(Clone, Default)]
pub struct RenderSignal {
    counter: Arc<AtomicU64>,
    setter: Arc<Mutex<Option<AsyncSetState<u64>>>>,
}

impl RenderSignal {
    /// 绑定 UI 线程创建的状态槽（每次渲染都会重新绑定同一个槽）。
    pub fn attach(&self, setter: AsyncSetState<u64>) {
        *self.setter.lock() = Some(setter);
    }

    /// 请求一次 UI 重绘（可从任意线程调用）。
    pub fn request(&self) {
        let setter = self.setter.lock().clone();
        let Some(setter) = setter else {
            // UI 还没挂载，等下一次状态推送或者首帧渲染即可。
            return;
        };
        let next = self.counter.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        setter.call(next);
    }
}

/// 创建服务推送消息的回调：写回控制器 + 请求重绘。
///
/// `controller_slot` 里放的是控制器的强引用，但控制器要等服务连接建立
/// 之后才创建，所以先用一个 OnceLock 占位。
pub fn make_service_callback(
    controller_slot: Arc<OnceLock<Arc<Mutex<AppController>>>>,
    render: RenderSignal,
) -> Arc<dyn Fn(ServerMessage) + Send + Sync + 'static> {
    Arc::new(move |msg| {
        let Some(controller) = controller_slot.get() else {
            // 控制器还没建好，这条消息作废；界面就绪后会主动要一次状态。
            return;
        };
        match msg {
            ServerMessage::State(snapshot) => {
                {
                    let mut c = controller.lock();
                    c.apply_service_state(*snapshot);
                }
                render.request();
            }
            ServerMessage::Error { message } => {
                {
                    let mut c = controller.lock();
                    c.set_status(StatusLevel::Error, message);
                }
                render.request();
            }
        }
    })
}
