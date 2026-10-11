//! Making a World on a server, from this computer, over SSH (`world-gui.md`,
//! 新建; walkthrough 「走查准备时发现的」 4): the page's commands over the
//! SSH work in `ssh.rs`. The program that goes up is the x86_64 Linux archive
//! the installer carries next to the app; the session stays open between
//! making the hub and the server joining.

mod ssh;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fastctx_types::kernel::X86_64_ARCHIVE;
use serde::Serialize;
use tauri::Manager as _;
use tauri::ipc::Channel;

pub(crate) use ssh::{Probed, Request};

/// The sessions kept open between making the hub and the server joining.
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
    let archive = carried(&app)?;
    let (remote, made) = ssh::make_hub(&request, &archive, |name| steps.send(name).map_err(|error| error.to_string())).await?;
    let session = sessions.next.fetch_add(1, Ordering::Relaxed);
    sessions.open.lock().expect("never poisoned").insert(session, Arc::new(remote));
    Ok(Made { session, address: made.address, hostname: made.hostname })
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

/// The Linux program the installer carries next to the app.
fn carried(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let path = app.path().resource_dir().map_err(|error| error.to_string())?.join("linux").join(X86_64_ARCHIVE);
    if !path.is_file() {
        return Err(format!("this app carries no Linux program ({} is missing)", path.display()));
    }
    Ok(path)
}
