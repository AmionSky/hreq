//! pluggable runtimes

use crate::either::Either;
use crate::Error;
use crate::Stream;
use futures_util::future::poll_fn;
use once_cell::sync::Lazy;
use std::future::Future;
use std::sync::Mutex;
use std::task::Poll;
use std::time::Duration;

impl Stream for exec::TcpStream {}

static CURRENT_RUNTIME: Lazy<Mutex<AsyncRuntime>> = Lazy::new(|| {
    if cfg!(feature = "async-std") {
        #[cfg(feature = "async-std")]
        return Mutex::new(AsyncRuntime::AsyncStd);
    } else if cfg!(feature = "tokio") {
        #[cfg(feature = "tokio")]
        return Mutex::new(AsyncRuntime::TokioShared);
    }
    Mutex::new(AsyncRuntime::BuiltIn)
});

/// Switches between different async runtimes.
///
/// This is a global singleton.
///
/// hreq supports async-std and tokio. Tokio support is enabled by default and comes in some
/// different flavors.
///
///   * `AsyncStd`. Requires the feature `async-std`. Supports
///     `.block()`.
///   * `TokioDefault`. The default option. A minimal tokio `rt-core`
///     which executes calls in one single thread. It does nothing
///     until the current thread blocks on a future using `.block()`.
///   * `TokioShared`. Picks up on a globally shared runtime by using a
///     [`Handle`]. This runtime cannot use the `.block()` extension
///     trait since that requires having a direct connection to the
///     tokio [`Runtime`].
///   * `TokioOwned`. Uses a preconfigured tokio [`Runtime`] that is
///     "handed over" to hreq.
///
///
/// # Example using `AsyncStd`:
///
/// ```
/// use hreq::AsyncRuntime;
/// #[cfg(feature = "async-std")]
/// AsyncRuntime::set_default(AsyncRuntime::AsyncStd, None);
/// ```
///
/// # Example using a shared tokio.
///
/// ```
/// use hreq::AsyncRuntime;
///
/// AsyncRuntime::set_default(AsyncRuntime::TokioShared, None);
/// ```
///
/// # Example using an owned tokio.
///
/// ```
/// use hreq::AsyncRuntime;
/// // normally: use tokio::runtime::Builder;
/// use tokio_lib::runtime::Builder;
///
/// let runtime = Builder::new()
///   .enable_io()
///   .enable_time()
///   .build()
///   .expect("Failed to build tokio runtime");
///
/// AsyncRuntime::set_default(AsyncRuntime::TokioOwned, Some(runtime));
/// ```
///
///
/// [`Handle`]: https://docs.rs/tokio/0.2.11/tokio/runtime/struct.Handle.html
/// [`Runtime`]: https://docs.rs/tokio/0.2.11/tokio/runtime/struct.Runtime.html
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AsyncRuntime {
    BuiltIn,
    #[cfg(feature = "async-std")]
    AsyncStd,
    #[cfg(feature = "tokio")]
    TokioShared,
    #[cfg(feature = "tokio")]
    TokioOwned,
}

#[allow(unused)]
enum Inner {
    BuiltIn,
    AsyncStd,
    TokioShared,
    TokioOwned,
}

impl AsyncRuntime {
    fn inner(self) -> Inner {
        match self {
            AsyncRuntime::BuiltIn => Inner::BuiltIn,
            #[cfg(feature = "async-std")]
            AsyncRuntime::AsyncStd => Inner::AsyncStd,
            #[cfg(feature = "tokio")]
            AsyncRuntime::TokioShared => Inner::TokioShared,
            #[cfg(feature = "tokio")]
            AsyncRuntime::TokioOwned => Inner::TokioOwned,
        }
    }

    pub(crate) fn current() -> Self {
        *CURRENT_RUNTIME.lock().unwrap()
    }

    pub(crate) async fn connect_tcp(self, addr: &str) -> Result<impl Stream, Error> {
        let inner = self.inner();
        Ok(match inner {
            Inner::BuiltIn | Inner::AsyncStd => Either::A(match inner {
                Inner::BuiltIn => Either::A(exec::connect_tcp(addr)?),
                Inner::AsyncStd => Either::B(async_std::connect_tcp(addr).await?),
                _ => unreachable!(),
            }),
            Inner::TokioShared | Inner::TokioOwned => {
                Either::B(async_tokio::connect_tcp(addr).await?)
            }
        })
    }

    pub(crate) async fn timeout(self, duration: Duration) {
        match self.inner() {
            Inner::BuiltIn => exec::timeout(duration).await,
            Inner::AsyncStd => async_std::timeout(duration).await,
            Inner::TokioShared | Inner::TokioOwned => async_tokio::timeout(duration).await,
        }
    }

    pub(crate) fn spawn<T: Future + Send + 'static>(self, task: T) {
        match self.inner() {
            Inner::BuiltIn => exec::spawn(task),
            Inner::AsyncStd => async_std::spawn(task),
            Inner::TokioShared | Inner::TokioOwned => async_tokio::spawn(task),
        }
    }

    pub(crate) fn block_on<F: Future>(self, task: F) -> F::Output {
        match self.inner() {
            Inner::BuiltIn => exec::block_on(task),
            Inner::AsyncStd => async_std::block_on(task),
            Inner::TokioShared | Inner::TokioOwned => async_tokio::block_on(task),
        }
    }
}

#[cfg(not(feature = "async-std"))]
pub(crate) mod async_std {
    use super::*;
    pub(crate) async fn connect_tcp(addr: &str) -> Result<impl Stream, Error> {
        Ok(exec::TcpStream::connect(addr)?) // filler in for type
    }
    pub async fn timeout(_: Duration) {
        unreachable!()
    }
    pub fn spawn<T>(_: T) {
        unreachable!()
    }
    pub fn block_on<F: Future>(_: F) -> F::Output {
        unreachable!()
    }
}

#[cfg(feature = "async-std")]
pub(crate) mod async_std {
    use super::*;
    use async_std_lib::net::TcpStream;
    use async_std_lib::task;

    impl Stream for TcpStream {}

    pub(crate) async fn connect_tcp(addr: &str) -> Result<impl Stream, Error> {
        Ok(TcpStream::connect(addr).await?)
    }

    pub async fn timeout(duration: Duration) {
        async_std_lib::future::timeout(duration, never()).await.ok();
    }

    pub fn spawn<T>(task: T)
    where
        T: Future + Send + 'static,
    {
        async_std_lib::task::spawn(async move {
            task.await;
        });
    }

    pub fn block_on<F: Future>(task: F) -> F::Output {
        task::block_on(task)
    }
}

#[cfg(not(feature = "tokio"))]
pub(crate) mod async_tokio {
    use super::*;
    pub(crate) async fn connect_tcp(addr: &str) -> Result<impl Stream, Error> {
        Ok(exec::TcpStream::connect(addr)?) // filler in for type
    }
    pub async fn timeout(_: Duration) {
        unreachable!()
    }
    pub fn spawn<T>(_: T) {
        unreachable!()
    }
    pub fn block_on<F: Future>(_: F) -> F::Output {
        unreachable!()
    }
}

#[cfg(feature = "tokio")]
pub(crate) mod async_tokio {
    use super::*;
    use crate::tokio::from_tokio;
    use tokio_lib::net::TcpStream;
    pub use tokio_lib::runtime::Handle;

    pub(crate) async fn connect_tcp(addr: &str) -> Result<impl Stream, Error> {
        Ok(from_tokio(TcpStream::connect(addr).await?))
    }

    pub async fn timeout(duration: Duration) {
        tokio_lib::time::delay_for(duration).await;
    }

    pub fn spawn<T>(task: T) {
        unreachable!()
    }

    pub fn block_on<F: Future>(task: F) -> F::Output {
        unreachable!()
    }

    // pub fn default_spawn<T>(task: T)
    // where
    //     T: Future + Send + 'static,
    // {
    //     with_handle(|h| {
    //         h.spawn(async move {
    //             task.await;
    //         });
    //     });
    // }

    // pub fn default_block_on<F: Future>(future: F) -> F::Output {
    //     with_runtime(|rt| rt.block_on(future))
    // }

    // fn create_default_runtime() -> (Handle, Runtime) {
    //     let runtime = Builder::new()
    //         .basic_scheduler()
    //         .enable_io()
    //         .enable_time()
    //         .build()
    //         .expect("Failed to build tokio runtime");
    //     let handle = runtime.handle().clone();
    //     (handle, runtime)
    // }

    // fn with_runtime<F: FnOnce(&mut tokio_lib::runtime::Runtime) -> R, R>(f: F) -> R {
    //     let mut rt = RUNTIME
    //         .get_or_init(|| {
    //             let (h, rt) = create_default_runtime();
    //             HANDLE.set(h).expect("Failed to set HANDLE");
    //             Mutex::new(rt)
    //         })
    //         .lock()
    //         .unwrap();
    //     f(&mut rt)
    // }

    // fn with_handle<F: FnOnce(tokio_lib::runtime::Handle)>(f: F) {
    //     let h = {
    //         HANDLE
    //             .get_or_init(|| {
    //                 let (h, rt) = create_default_runtime();
    //                 RUNTIME.set(Mutex::new(rt)).expect("Failed to set RUNTIME");
    //                 h
    //             })
    //             .clone()
    //     };
    //     f(h)
    // }
}

// TODO does this cause memory leaks?
pub async fn never() {
    poll_fn::<(), _>(|_| Poll::Pending).await;
    unreachable!()
}
