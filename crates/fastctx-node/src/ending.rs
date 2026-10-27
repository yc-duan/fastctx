//! Preserve volatile call evidence before controlled exits and stop signals.

use std::sync::{Arc, OnceLock};
static LOG: OnceLock<Arc<crate::log::Log>> = OnceLock::new();

pub(crate) fn start(log: Arc<crate::log::Log>) -> std::io::Result<()> {
    let _ = LOG.set(log);
    std::thread::Builder::new().name("node-stop-signals".into()).spawn(|| {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(error) => {
                if let Some(log) = LOG.get() {
                    log.line(format_args!("starting stop signal listener: {error}"));
                }
                return;
            }
        };
        runtime.block_on(async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let (Ok(mut terminate), Ok(mut interrupt), Ok(mut hangup)) =
                    (signal(SignalKind::terminate()), signal(SignalKind::interrupt()), signal(SignalKind::hangup()))
                else {
                    if let Some(log) = LOG.get() {
                        log.line("could not listen for node stop signals");
                    }
                    return;
                };
                tokio::select! { _ = terminate.recv() => {}, _ = interrupt.recv() => {}, _ = hangup.recv() => {} }
            }
            #[cfg(windows)]
            {
                use tokio::signal::windows;
                let (Ok(mut close), Ok(mut shutdown), Ok(mut control)) =
                    (windows::ctrl_close(), windows::ctrl_shutdown(), windows::ctrl_c())
                else {
                    if let Some(log) = LOG.get() {
                        log.line("could not listen for node stop signals");
                    }
                    return;
                };
                tokio::select! { _ = close.recv() => {}, _ = shutdown.recv() => {}, _ = control.recv() => {} }
            }
            exit(0);
        });
    })?;
    Ok(())
}

pub(crate) fn exit(code: i32) -> ! {
    with_reason(code, "the node is ending".into())
}

/// The final exit has no lock or I/O dependency. Diagnostics are attempted
/// only after an independent thread is committed to ending the process.
pub(crate) fn with_reason(code: i32, reason: String) -> ! {
    if std::thread::Builder::new()
        .name("node-final-exit".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            std::process::exit(code);
        })
        .is_err()
    {
        std::process::exit(code);
    }
    if let Some(log) = LOG.get() {
        log.line(reason);
        crate::intake::dump(log);
    }
    std::process::exit(code)
}
