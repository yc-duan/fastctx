//! The reactor and workers reserved for established member connections, on
//! threads that keep network lines alive (`specs/world-engine.md`, 线程的三种
//! 角色). Business state is never passed to this runtime's transport tasks.

use std::io;
use std::net::SocketAddr;

use fastctx_platform::roles::{self, Role};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::{Handle, Runtime};

pub(crate) struct Network {
    runtime: Option<Runtime>,
    /// How far its tasks ran (`crate::watchdog`).
    watched: fastctx_platform::task::Watched,
}

impl Network {
    pub(crate) fn new() -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("hub-network")
            // Its threads may block until they first wake to run tasks, which
            // only the scheduler's threads do.
            .on_thread_start(|| roles::set(Role::Blocking))
            .on_thread_unpark(|| roles::set(Role::Link))
            .enable_all()
            .build()?;
        let watched = {
            let _entered = runtime.enter();
            fastctx_platform::task::watch("hub-network")
        };
        Ok(Self { runtime: Some(runtime), watched })
    }

    pub(crate) fn handle(&self) -> Handle {
        self.runtime.as_ref().expect("network running").handle().clone()
    }

    pub(crate) fn watched(&self) -> fastctx_platform::task::Watched {
        self.watched.clone()
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Registers accepted sockets with the transport reactor, even when the
/// accept coordinator runs on the business runtime.
pub(crate) async fn accept(handle: &Handle, listener: &TcpListener) -> io::Result<(TcpStream, SocketAddr)> {
    std::future::poll_fn(|cx| {
        let _entered = handle.enter();
        listener.poll_accept(cx)
    })
    .await
}
