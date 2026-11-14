//! What the bench reaches of the Codex follower (sync-v2-impl-design §18):
//! the app-server client, started with the least of a host's pump — answers
//! handed to their requests, the end acknowledged, nothing else taken — and
//! a reading of a thread past a seam as the host makes it, no turn of its
//! own running and no owner to decorate pictures for.

pub use super::follow::{Restart, Seam, Tail};
pub use super::server::AppServer;
use super::server::Inbound;
use jiff::Timestamp;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

/// Starts `program` as the app-server; call within a Tokio runtime.
pub fn spawn(program: &Path, args: &[String], cwd: &Path, env: &[(OsString, OsString)], log: super::Log) -> std::io::Result<Arc<AppServer>> {
    let (server, mut inbound) = AppServer::spawn(program, args, cwd, env, log)?;
    tokio::spawn(async move {
        while let Some(message) = inbound.recv().await {
            match message {
                Inbound::Response { waiter, answer } => {
                    let _ = waiter.send(answer);
                }
                Inbound::Exited { drained, .. } => {
                    let _ = drained.send(());
                }
                Inbound::Notification { .. } | Inbound::Request { .. } => {}
            }
        }
    });
    Ok(server)
}

pub async fn tail(
    server: &AppServer,
    thread: &str,
    seam: &Seam,
    accept: &(dyn Fn(Timestamp) -> bool + Send + Sync),
) -> Result<Result<Tail, Restart>, String> {
    super::follow::tail(server, thread, None, None, seam, accept).await
}
