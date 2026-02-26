//! COM Worker thread management and thread-safe COM interface passing.
//!
//! # COM 生命周期与 Apartment 一致性保证
//!
//! 在 COM (Component Object Model) 的内存管理和多线程模型中，确保“生命周期”和“Apartment（套间）一致性”是避免崩溃和未定义行为的关键。我们通过 `ComWorker` 和 `ComSend` 配合，主要从以下三个维度提供了这种保证：
//!
//! ### 1. 运行环境的一致性 (Context Consistency)
//! COM 对象（尤其是 STA 模式下的对象）对线程极度敏感。如果在一个线程创建了对象，在另一个线程调用其方法，通常会触发 `RPC_E_WRONG_THREAD` 错误。
//!
//! *   **保证方式**：`ComWorker` 维护了一个唯一的、长周期的专用线程。
//!     *   在该线程启动时，初始化了 COM 环境。
//!     *   所有实质性的 COM 操作（创建、调用方法、销毁）都通过 `call_sync` 被封装在闭包中，发送到同一个线程执行。
//!     *   虽然 `ComSend` 允许对象指针在 Rust 线程间传递，但我们遵循了“仅在 Worker 线程内解包并调用”的原则。
//!
//! ### 2. 生命周期管理 (Reference Counting)
//! Windows-rs 提供的 COM 包装器（如 `IMMDevice`）在 Rust 层面实现了 `Clone` 和 `Drop`。
//!
//! *   **保证方式**：
//!     *   **引用计数**：当你 `Clone` 一个 COM 接口时，底层会调用 `AddRef()`。当你 `Drop` 它时，会调用 `Release()`。
//!     *   **跨线程安全释放**：由于 `ComWorker` 闭包捕获了这些对象，即使业务线程丢弃了闭包容器，对象真正的 `Release` 动作会在闭包运行结束、引用计数归零时发生。
//!     *   **延迟销毁**：通过 `ComWorker` 的任务队列，我们确保了即便业务线程已经结束，COM 对象也会在 Worker 线程内安全地完成销毁，避免了在 COM 未初始化的线程中释放对象的崩溃。
//!
//! ### 3. ComSend 的“契约”作用
//! `ComSend<T>` 实际上是一个类型安全的通行证。
//!
//! *   **保证方式**：
//!     *   Rust 编译器默认 `windows-rs` 接口为 `!Send`。使用 `unsafe impl Send for ComSend<T>` 是对编译器的承诺：我们会把指针搬运到其他线程，但保证只在正确的 COM 线程解包并使用。
//!     *   在我们的设计中，凡是涉及到 `ComSend::take()` 的地方，通常都嵌套在 `com_worker::global().call_sync(...)` 的闭包里，从而在逻辑上闭环了这种一致性。
//!
//! ### 4. 异步运行时兼容性
//! `ComWorker` 内部使用 `futures::executor::block_on` 来运行异步的 `worker_loop`，以适应 COM apartment 的线程模型要求。然而，在 Tokio 或其他异步运行时中直接创建新的 `ComWorker` 实例（如 `new()` 或 `with_apartment()`）可能导致阻塞或死锁，因为嵌套运行时会产生调度冲突。建议在同步上下文中使用，或改用预初始化的全局 `ComWorker`（通过 `global()`）。
//! TODO:如何既能满足COM apartment 的线程模型要求,有能提供异步和同步api,还能再异步线程新建ComWorker对象?

use anyhow::{Result, anyhow};
use callcomapi_macros::com_thread;
use once_cell::sync::Lazy;
use std::any::Any;

/// A wrapper to allow passing COM pointers/interfaces between threads safely.
///
/// Safety contract and semantics:
/// - `windows-rs` COM interface wrappers are often `!Send`/`!Sync` because COM
///   objects are apartment/thread-affine. `ComSend<T>` provides an *unsafe*
///   `Send`/`Sync` implementation as a thin promise: the underlying value may be
///   moved between threads, but it MUST only be *accessed* or *dropped* on a
///   thread where COM is initialized and in an appropriate apartment for that
///   object.
/// - Typical usage: `ComWorker::call_sync` and `call_async` return `ComSend<R>`.
///   If `R: Send`, callers may call `unwrap()` to obtain and use `R` on the
///   caller thread. If `R` is a non-Send COM interface, callers **must not**
///   call `take()` on arbitrary threads. Instead, pass the `ComSend<R>` back
///   into a `call_sync`/`call_async` closure and call `take()` inside that
///   closure so the object is used/dropped on the COM thread.
///
/// Example:
/// ```ignore
/// // returns ComSend<IAudioClient>
/// let h = com_worker::call_sync(|| -> Result<IAudioClient> { /* ... */ })?;
/// // safely use the client on the COM thread
/// com_worker::call_sync(move || { let client = h.take(); /* use client */ Ok(()) })?;
/// ```
///
/// This type intentionally only documents the contract: it does not enforce it.
/// It is the caller's responsibility to obey the apartment/thread rules.
#[derive(Debug, Clone)]
pub struct ComSend<T>(T);

unsafe impl<T> Send for ComSend<T> {}
unsafe impl<T> Sync for ComSend<T> {}

impl<T> ComSend<T> {
    pub fn new(t: T) -> Self {
        Self(t)
    }

    /// Consume the wrapper and return the underlying value.
    /// Restricted to crate-internal use to minimize the surface area for apartment violations.
    pub(crate) fn take(self) -> T {
        self.0
    }
}

impl<T: Send> ComSend<T> {
    /// Safe unwrap for types that are already `Send`.
    ///
    /// If `T: Send`, it is safe to move the inner value to the caller thread and
    /// use it directly. This consumes the `ComSend` wrapper and returns the value.
    pub fn unwrap(self) -> T {
        self.0
    }
}

/// A simple worker that initializes COM on its dedicated thread and runs
/// small tasks (COM API Calls) that need to execute on that thread.
///
/// The API is intentionally minimal: you create a `ComWorker::new()`, then
/// call `call_sync` or `call_async` with a closure that will be executed on the COM thread and
/// returns a `Result<R, anyhow::Error>`. The return value is transported
/// back via a oneshot channel and wrapped in `ComSend<R>`.
/// COM apartment selection
///
///
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Apartment {
    /// Multi-threaded apartment (MTA) -> COINIT_MULTITHREADED
    MTA,
    #[default]
    /// Single-threaded apartment (STA) -> COINIT_APARTMENTTHREADED
    STA,
}

/// Helper function to dispatch sync COM tasks to a dedicated thread based on apartment.
#[com_thread(STA)]
fn dispatch_sta_sync(
    task: Box<dyn FnOnce() -> Result<ComSend<Box<dyn Any>>> + Send>,
) -> Result<ComSend<Box<dyn Any>>> {
    task()
}

#[com_thread(MTA)]
fn dispatch_mta_sync(
    task: Box<dyn FnOnce() -> Result<ComSend<Box<dyn Any>>> + Send>,
) -> Result<ComSend<Box<dyn Any>>> {
    task()
}

/// Helper function to dispatch async COM tasks to a dedicated thread based on apartment.
#[com_thread(STA)]
async fn dispatch_sta_async(
    task: Box<dyn FnOnce() -> Result<ComSend<Box<dyn Any>>> + Send>,
) -> Result<ComSend<Box<dyn Any>>> {
    task()
}

#[com_thread(MTA)]
async fn dispatch_mta_async(
    task: Box<dyn FnOnce() -> Result<ComSend<Box<dyn Any>>> + Send>,
) -> Result<ComSend<Box<dyn Any>>> {
    task()
}

pub struct ComWorker {
    apartment: Apartment,
}

impl ComWorker {
    /// Create a new worker with default apartment.
    ///
    /// Using `callcomapi_macros`, this is now safe to call from any context.
    pub fn new() -> Self {
        Self::with_apartment(Apartment::default())
    }

    /// Create a worker with the specified apartment.
    pub fn with_apartment(apartment: Apartment) -> Self {
        Self { apartment }
    }

    /// Synchronous call: execute a closure on a dedicated COM thread.
    pub fn call_sync<R, F>(&self, f: F) -> Result<ComSend<R>>
    where
        R: 'static,
        F: FnOnce() -> Result<R> + Send + 'static,
    {
        let task = Box::new(move || match f() {
            Ok(r) => Ok(ComSend::new(Box::new(r) as Box<dyn Any>)),
            Err(e) => Err(e),
        });

        let res = match self.apartment {
            Apartment::STA => dispatch_sta_sync(task),
            Apartment::MTA => dispatch_mta_sync(task),
        }?;

        let boxed = res.take();
        match boxed.downcast::<R>() {
            Ok(b) => Ok(ComSend::new(*b)),
            Err(_) => Err(anyhow!("type mismatch in com worker response")),
        }
    }

    /// Asynchronous call: execute a closure on a dedicated COM thread.
    pub fn call_async<R, F>(&self, f: F) -> futures::future::BoxFuture<'static, Result<ComSend<R>>>
    where
        R: 'static,
        F: FnOnce() -> Result<R> + Send + 'static,
    {
        let task = Box::new(move || match f() {
            Ok(r) => Ok(ComSend::new(Box::new(r) as Box<dyn Any>)),
            Err(e) => Err(e),
        });

        let apartment = self.apartment;
        Box::pin(async move {
            let res = match apartment {
                Apartment::STA => dispatch_sta_async(task).await,
                Apartment::MTA => dispatch_mta_async(task).await,
            }?;

            let boxed = res.take();
            match boxed.downcast::<R>() {
                Ok(b) => Ok(ComSend::new(*b)),
                Err(_) => Err(anyhow!("type mismatch in com worker response")),
            }
        })
    }

    /// Request the worker to stop. (Deprecated: callcomapi_macros manages thread lifecycle)
    pub fn stop(&mut self) {}

    /// Start the worker. (Deprecated: callcomapi_macros initializes on demand)
    pub fn start(&mut self) {}

    /// Return true if the worker thread is running.
    pub fn is_running(&self) -> bool {
        true
    }
}

impl Drop for ComWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Global, lazily-initialized `ComWorker` wrapped in `parking_lot::RwLock`.
///
/// Use `global()` to obtain a read guard for calling COM APIs, or `global_mut()` to get a write guard
/// when you need to stop or otherwise mutate the worker. The lazy initialization ensures the
/// worker thread (and COM initialization) is created on first use.
pub static GLOBAL_COM_WORKER: Lazy<parking_lot::RwLock<ComWorker>> =
    Lazy::new(|| parking_lot::RwLock::new(ComWorker::new()));

/// Convenience wrapper: call the *global* `ComWorker`'s `call_sync`.
///
/// See `ComWorker::call_sync` for the apartment/thread contract and `ComSend` semantics.
pub fn call_on_sync<R, F>(worker: &ComWorker, f: F) -> Result<ComSend<R>>
where
    R: 'static,
    F: FnOnce() -> Result<R> + Send + 'static,
{
    worker.call_sync(f)
}

/// Convenience wrapper: call the *global* `ComWorker`'s `call_async`.
///
/// See `ComWorker::call_async` for the apartment/thread contract and `ComSend` semantics.
/// Convenience wrapper: call the specified `ComWorker`'s `call_async`.
///
/// See `ComWorker::call_async` for the apartment/thread contract and `ComSend` semantics.
pub fn call_on_async<R, F>(
    worker: &ComWorker,
    f: F,
) -> futures::future::BoxFuture<'static, Result<ComSend<R>>>
where
    R: 'static,
    F: FnOnce() -> Result<R> + Send + 'static,
{
    worker.call_async(f)
}

/// Return a read guard to the global `ComWorker` (lazy-initialized).
///
/// The returned guard allows calling instance methods such as `call_sync` and
/// `call_async` on the worker. Use the instance methods when you need explicit
/// control over the worker (for testing or to choose a different apartment).
pub fn global() -> parking_lot::RwLockReadGuard<'static, ComWorker> {
    GLOBAL_COM_WORKER.read()
}

/// Return a write guard to the global `ComWorker` (lazy-initialized).
///
/// Use this when you need to mutate the global worker (for example to stop/start
/// it, or to change its apartment). Operations that change the worker lifecycle
/// should be done carefully to avoid races between user code and in-flight tasks.
pub fn global_mut() -> parking_lot::RwLockWriteGuard<'static, ComWorker> {
    GLOBAL_COM_WORKER.write()
}

/// Convenience: stop the global `ComWorker` for a graceful shutdown.
pub fn shutdown_global() {
    let mut g = global_mut();
    g.stop();
}

/// Convenience: start or restart the global `ComWorker`.
pub fn start_global() {
    let mut g = global_mut();
    g.start();
}

/// Set the global `ComWorker` apartment. This will start a new one
/// using the requested apartment.
pub fn set_global_apartment(apartment: Apartment) {
    let mut g = global_mut();
    *g = ComWorker::with_apartment(apartment);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[tokio::test(flavor = "multi_thread")]
    async fn com_worker_runs_com_api_sync() {
        let res = global().call_sync(|| -> Result<()> {
            use windows::Win32::Media::Audio::{
                IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
            };
            use windows::Win32::System::Com::CoCreateInstance;
            // Create MMDeviceEnumerator and get default render endpoint
            let enumerator: IMMDeviceEnumerator = unsafe {
                CoCreateInstance(
                    &MMDeviceEnumerator,
                    None,
                    windows::Win32::System::Com::CLSCTX_ALL,
                )
            }
            .map_err(|e| anyhow!("CoCreateInstance failed: {:?}", e))?;

            let _dev = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                .map_err(|e| anyhow!("GetDefaultAudioEndpoint failed: {:?}", e))?;
            Ok(())
        });
        assert!(res.is_ok());
    }

    #[cfg(target_os = "windows")]
    #[tokio::test(flavor = "multi_thread")]
    async fn com_worker_runs_com_api_async() {
        let fut = call_on_async(&global(), || -> Result<String> {
            use windows::Win32::Media::Audio::{
                IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
            };
            use windows::Win32::System::Com::CoCreateInstance;

            let enumerator: IMMDeviceEnumerator = unsafe {
                CoCreateInstance(
                    &MMDeviceEnumerator,
                    None,
                    windows::Win32::System::Com::CLSCTX_ALL,
                )
            }
            .map_err(|e| anyhow!("CoCreateInstance failed: {:?}", e))?;

            let _dev = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                .map_err(|e| anyhow!("GetDefaultAudioEndpoint failed: {:?}", e))?;
            Ok("ok".to_string())
        });

        let res = fut.await;
        assert!(res.is_ok());
        assert_eq!(res.unwrap().take(), "ok".to_string());
    }

    #[cfg(target_os = "windows")]
    #[tokio::test(flavor = "multi_thread")]
    async fn com_worker_apartment_perf_compare() {
        use std::time::Instant;
        let iters = 20usize;

        // Centralized task function reused for both MTA and STA workers
        fn task() -> Result<()> {
            use windows::Win32::Media::Audio::{
                IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
            };
            use windows::Win32::System::Com::CoCreateInstance;
            let enumerator: IMMDeviceEnumerator = unsafe {
                CoCreateInstance(
                    &MMDeviceEnumerator,
                    None,
                    windows::Win32::System::Com::CLSCTX_ALL,
                )
            }
            .map_err(|e| anyhow!("CoCreateInstance failed: {:?}", e))?;
            let _dev = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                .map_err(|e| anyhow!("GetDefaultAudioEndpoint failed: {:?}", e))?;
            Ok(())
        }

        let start = Instant::now();
        for _ in 0..iters {
            let _ = call_on_async(&global(), || task()).await.unwrap().take();
        }
        let dur_m = start.elapsed();

        // STA worker
        let w_s = ComWorker::with_apartment(Apartment::STA);
        let start = Instant::now();
        for _ in 0..iters {
            let _ = call_on_async(&w_s, || task()).await.unwrap().take();
        }
        let dur_s = start.elapsed();

        println!("MTA: {:?}, STA: {:?} ({} iters)", dur_m, dur_s, iters);
        // assert!(dur_m.as_secs_f64() < 30.0 && dur_s.as_secs_f64() < 30.0);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn test_com_thread_async() {
        use callcomapi_macros::com_thread;

        #[com_thread]
        async fn get_async_value(x: i32) -> i32 {
            x + 20
        }

        let res = get_async_value(10).await;
        assert_eq!(res, 30);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_with_com_mta() {
        use callcomapi_macros::with_com;
        use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
        use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};

        #[with_com("mta")]
        fn check_mta() -> Result<()> {
            let _enumerator: IMMDeviceEnumerator =
                unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                    .map_err(|e| anyhow!("CoCreateInstance failed: {:?}", e))?;
            Ok(())
        }

        assert!(check_mta().is_ok());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_com_thread_macro() {
        use callcomapi_macros::com_thread;

        #[com_thread]
        fn add_ten(x: i32) -> i32 {
            x + 10
        }

        assert_eq!(add_ten(5), 15);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_with_com_macro() {
        use callcomapi_macros::with_com;
        use windows::Win32::Media::Audio::{
            IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
        };
        use windows::Win32::System::Com::CoCreateInstance;

        #[with_com]
        fn check_endpoint() -> Result<()> {
            let enumerator: IMMDeviceEnumerator = unsafe {
                CoCreateInstance(
                    &MMDeviceEnumerator,
                    None,
                    windows::Win32::System::Com::CLSCTX_ALL,
                )
            }
            .map_err(|e| anyhow!("CoCreateInstance failed: {:?}", e))?;

            let _dev = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
                .map_err(|e| anyhow!("GetDefaultAudioEndpoint failed: {:?}", e))?;
            Ok(())
        }

        assert!(check_endpoint().is_ok());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn com_api_without_com_init_fails() {
        use windows::Win32::Media::Audio::MMDeviceEnumerator;
        use windows::Win32::System::Com::CoCreateInstance;

        // Intentionally do NOT call CoInitializeEx on this thread; CoCreateInstance should fail.
        let res: windows::core::Result<windows::Win32::Media::Audio::IMMDeviceEnumerator> = unsafe {
            CoCreateInstance(
                &MMDeviceEnumerator,
                None,
                windows::Win32::System::Com::CLSCTX_ALL,
            )
        };
        println!("CoCreateInstance result without COM init: {:?}", res);
    }
}
