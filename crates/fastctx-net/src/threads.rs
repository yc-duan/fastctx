//! The threads a machine's lines run on (`specs/world-engine.md`, 连接与判活).
//!
//! A line is read, written and kept alive on a thread of its own: frames in
//! and out, heartbeats, the silence that ends a dead line, dialing again.
//! What the frames mean is handled on the node's own runtime, so however
//! busy the node is, its lines still say it is there and hear the other end.
//! The control lines, the line to the hub and the direct links, share one
//! thread; data connections, which carry transfers of hundreds of megabytes,
//! have another, so a transfer never holds back a heartbeat.
//!
//! A socket is woken by the runtime it was made on, so a line is dialed,
//! or its listener bound, on the thread that reads it.

use std::future::Future;
use std::sync::OnceLock;

use fastctx_platform::roles::{self, Role};
use tokio::runtime::Handle;

static LINES: OnceLock<Handle> = OnceLock::new();
static DATA: OnceLock<Handle> = OnceLock::new();

/// The runtime of the control lines' thread, started the first time it is
/// asked for.
pub(crate) fn lines() -> Handle {
    LINES.get_or_init(|| start("fastctx-link", Role::Link, "link")).clone()
}

/// The runtime of the data connections' thread, started the first time it
/// is asked for.
pub(crate) fn data() -> Handle {
    DATA.get_or_init(|| start("fastctx-data", Role::Data, "data")).clone()
}

/// Starts `work` on `runtime`, and waits for it. Dropping the wait stops it,
/// as dropping `work` itself would; a panic in it goes on in the waiter.
/// The wait holds only the task's handle, never `work`, so a caller's
/// future does not grow by what `work` holds.
pub(crate) fn run_on<F>(runtime: &Handle, work: F) -> impl Future<Output = Result<F::Output, String>> + Send + 'static
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    struct Stop(tokio::task::AbortHandle);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let task = fastctx_platform::task::spawn_on(runtime, work);
    let stop = Stop(task.abort_handle());
    async move {
        let _stop = stop;
        match task.await {
            Ok(output) => Ok(output),
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(_) => Err("this process is stopping".to_string()),
        }
    }
}

fn start(name: &str, role: Role, watched: &'static str) -> Handle {
    let (started, handle) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            roles::set(role);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .on_thread_unpark(move || roles::set(role))
                .enable_all()
                .build()
                .expect("a lines' runtime starts");
            let _ = started.send(runtime.handle().clone());
            runtime.block_on(async {
                fastctx_platform::task::watch(watched);
                std::future::pending::<()>().await
            });
        })
        .expect("a lines' thread starts");
    handle.recv().expect("a lines' runtime is handed over")
}
