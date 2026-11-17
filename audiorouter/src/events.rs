//! 后台事件源：把服务线程推送的状态写回控制器，并请求一次重绘。
//!
//! 设备枚举、路由启停都在服务线程里完成。服务线程把状态快照推过来，
//! 这里只负责把它写回控制器并触发 UI 重绘。
//!
//! 现在是同进程内的线程间调用（以前是跨进程管道），所以不需要再绕
//! 「连接建立之后再填控制器」的占位逻辑。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use app_core::bus::ServerMessage;
use app_core::controller::{AppController, StatusLevel};
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

/// 创建服务状态回调：写回控制器 + 请求重绘。
///
/// 回调运行在**服务线程**上（不是 UI 线程），所以它必须自己保证线程安全：
/// 控制器用互斥量保护，重绘请求由 reactor 负责 marshal 到 UI 线程。
pub fn make_service_callback(
    controller: Arc<Mutex<AppController>>,
    render: RenderSignal,
) -> Arc<dyn Fn(ServerMessage) + Send + Sync + 'static> {
    Arc::new(move |msg| match msg {
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
    })
}
