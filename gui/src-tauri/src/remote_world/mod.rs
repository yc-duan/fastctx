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

/// `machine` is the name the server takes in the World, made from its host
/// name.
#[derive(Serialize)]
pub(crate) struct Made {
    session: u64,
    address: String,
    machine: String,
}

/// Connects far enough to see the server's key, and says whether this
/// computer's SSH records know it.
#[fastctx_gui_command::command]
pub(crate) async fn remote_probe(host: String, port: u16) -> Result<Probed, String> {
    ssh::probe(&host, port).await
}

/// What the server showed once signed in to: the name it would take in the
/// World, made from its host name; the ports taken there; the hub of a
/// World it keeps already, if it does; and whether it is a machine of one.
#[derive(Serialize)]
pub(crate) struct Looked {
    machine: String,
    busy: Vec<u16>,
    hub: Option<ssh::Held>,
    joined: bool,
}

/// Signs in to the server whose key was confirmed and looks at it.
#[fastctx_gui_command::command]
pub(crate) async fn remote_look(server: Server) -> Result<Looked, String> {
    let seen = ssh::look(&server).await?;
    Ok(Looked {
        machine: fastctx_types::MachineName::from_host(&seen.hostname).to_string(),
        busy: seen.busy,
        hub: seen.hub,
        joined: seen.joined,
    })
}

/// Takes the World a server keeps off it, as for a hub made and not wanted:
/// signs in, checks it, uploads the program that does it, and leaves
/// nothing behind.
#[fastctx_gui_command::command]
pub(crate) async fn remote_withdraw(
    app: tauri::AppHandle,
    server: Server,
    steps: Channel<&'static str>,
) -> Result<(), String> {
    let step = |name| steps.send(name).map_err(|error| error.to_string());
    let remote = ssh::open(&server, &carried(&app)?, &step).await?;
    step("withdraw")?;
    let withdrawn = remote.withdraw().await;
    let closed = remote.close().await;
    withdrawn.and(closed)
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
    let (remote, made) =
        ssh::make_hub(&request, &carried(&app)?, |name| steps.send(name).map_err(|error| error.to_string())).await?;
    let machine = fastctx_types::MachineName::from_host(&made.hostname).to_string();
    Ok(Made { session: keep(&sessions, remote), address: made.address, machine })
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
    let remote =
        ssh::open(&server, &carried(&app)?, &|name| steps.send(name).map_err(|error| error.to_string())).await?;
    let machine = fastctx_types::MachineName::from_host(remote.hostname()).to_string();
    Ok(Opened { session: keep(&sessions, remote), machine })
}

#[derive(Serialize)]
pub(crate) struct Opened {
    session: u64,
    machine: String,
}

fn keep(sessions: &Sessions, remote: ssh::Remote) -> u64 {
    let session = sessions.next.fetch_add(1, Ordering::Relaxed);
    sessions.open.lock().expect("never poisoned").insert(session, Arc::new(remote));
    session
}

/// Installs the server as one of the World's machines with `token`, then
/// takes the temporary directory away and closes the session; with
/// `keep_open`, the session stays, as for the hub to move to the server
/// next (`remote_arrive`).
#[fastctx_gui_command::command]
pub(crate) async fn remote_join(
    sessions: tauri::State<'_, Sessions>,
    session: u64,
    token: String,
    keep_open: Option<bool>,
    proxy: Option<String>,
    steps: Channel<&'static str>,
) -> Result<(), String> {
    steps.send("join").map_err(|error| error.to_string())?;
    let remote = sessions
        .open
        .lock()
        .expect("never poisoned")
        .get(&session)
        .cloned()
        .ok_or("the connection to the server is gone")?;
    remote.join(&token, proxy.as_deref()).await?;
    if keep_open == Some(true) {
        return Ok(());
    }
    close(&sessions, session).await
}

/// Installs the system service of the hub moving to the server, with the
/// sudo the session has (`fastctx hub arrive` there); the session stays
/// open.
#[fastctx_gui_command::command]
pub(crate) async fn remote_arrive(
    sessions: tauri::State<'_, Sessions>,
    session: u64,
    steps: Channel<&'static str>,
) -> Result<(), String> {
    steps.send("arrive").map_err(|error| error.to_string())?;
    let remote = sessions
        .open
        .lock()
        .expect("never poisoned")
        .get(&session)
        .cloned()
        .ok_or("the connection to the server is gone")?;
    remote.arrive().await
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
