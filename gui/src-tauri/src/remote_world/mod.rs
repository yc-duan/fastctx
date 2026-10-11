//! Making a World on a server, or adding a server to a World, from this
//! computer over SSH (`world-gui.md`, 新建 and 添加设备; walkthrough
//! 「走查准备时发现的」 4): the page's commands over the SSH work in
//! `ssh.rs`. The program that goes up is the Linux archive for the server's
//! architecture the installer carries next to the app; the session stays
//! open until the server has joined.

mod ssh;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::Manager as _;
use tauri::ipc::Channel;

pub(crate) use ssh::{Probed, Request, Server};

/// The sessions kept open until the server has joined.
#[derive(Default)]
pub(crate) struct Sessions {
    next: AtomicU64,
    open: Mutex<HashMap<u64, Arc<ssh::Remote>>>,
}

#[derive(Serialize)]
pub(crate) struct Made {
    session: u64,
    address: String,
    hostname: String,
}

/// Connects far enough to see the server's key, and says whether this
/// computer's SSH records know it.
#[fastctx_gui_command::command]
pub(crate) async fn remote_probe(host: String, port: u16) -> Result<Probed, String> {
    ssh::probe(&host, port).await
}

/// Connects, checks the server, uploads the program and makes the hub; the
/// session stays open for the server to join.
#[fastctx_gui_command::command]
pub(crate) async fn remote_hub(
    app: tauri::AppHandle,
    sessions: tauri::State<'_, Sessions>,
    request: Request,
    steps: Channel<&'static str>,
) -> Result<Made, String> {
    let (remote, made) = ssh::make_hub(&request, &carried(&app)?, |name| steps.send(name).map_err(|error| error.to_string())).await?;
    Ok(Made { session: keep(&sessions, remote), address: made.address, hostname: made.hostname })
}

/// A server for an existing World: signs in, checks it and uploads the
/// program; the session stays open for it to join with a code made next.
#[fastctx_gui_command::command]
pub(crate) async fn remote_open(
    app: tauri::AppHandle,
    sessions: tauri::State<'_, Sessions>,
    server: Server,
    steps: Channel<&'static str>,
) -> Result<Opened, String> {
    let remote = ssh::open(&server, &carried(&app)?, &|name| steps.send(name).map_err(|error| error.to_string())).await?;
    let hostname = remote.hostname().to_string();
    Ok(Opened { session: keep(&sessions, remote), hostname })
}

#[derive(Serialize)]
pub(crate) struct Opened {
    session: u64,
    hostname: String,
}

fn keep(sessions: &Sessions, remote: ssh::Remote) -> u64 {
    let session = sessions.next.fetch_add(1, Ordering::Relaxed);
    sessions.open.lock().expect("never poisoned").insert(session, Arc::new(remote));
    session
}

/// Installs the server as one of the World's machines with `token`, then
/// takes the temporary directory away and closes the session.
#[fastctx_gui_command::command]
pub(crate) async fn remote_join(
    sessions: tauri::State<'_, Sessions>,
    session: u64,
    token: String,
    steps: Channel<&'static str>,
) -> Result<(), String> {
    steps.send("join").map_err(|error| error.to_string())?;
    let remote = sessions.open.lock().expect("never poisoned").get(&session).cloned().ok_or("the connection to the server is gone")?;
    remote.join(&token).await?;
    close(&sessions, session).await
}

/// Takes the temporary directory away and closes the session.
#[fastctx_gui_command::command]
pub(crate) async fn remote_close(sessions: tauri::State<'_, Sessions>, session: u64) -> Result<(), String> {
    close(&sessions, session).await
}

async fn close(sessions: &Sessions, session: u64) -> Result<(), String> {
    let Some(remote) = sessions.open.lock().expect("never poisoned").remove(&session) else { return Ok(()) };
    remote.close().await
}

/// Where the Linux programs the installer carries are, one for each
/// architecture.
fn carried(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().resource_dir().map_err(|error| error.to_string())?.join("linux"))
}
