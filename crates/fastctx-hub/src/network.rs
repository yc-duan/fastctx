//! The reactor and workers reserved for established member connections.
//! Business state is never passed to this runtime's transport tasks.

use std::io;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::{Handle, Runtime};

pub(crate) struct Network {
    runtime: Option<Runtime>,
}

impl Network {
    pub(crate) fn new() -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("hub-network")
            .enable_all()
            .build()?;
        Ok(Self { runtime: Some(runtime) })
    }

    pub(crate) fn handle(&self) -> Handle {
        self.runtime.as_ref().expect("network running").handle().clone()
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
