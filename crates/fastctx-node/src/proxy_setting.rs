//! The HTTP proxy this computer reaches its hubs through, chosen in Fastctx
//! itself: a variable set in a shell never reaches a node its system
//! service starts. Kept in the World's `net/proxy.json`, taken as the node
//! starts, and tried before the system's proxy (`fastctx_net::proxy`).

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;

use crate::Node;

#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    url: String,
}

fn file(world: &WorldDir) -> PathBuf {
    world.net().join("proxy.json")
}

/// The proxy kept for this World, if one is.
pub fn kept(world: &WorldDir) -> io::Result<Option<String>> {
    match std::fs::read(file(world)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice::<Kept>(&bytes).map_err(io::Error::other)?.url)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Takes the proxy kept for this World, as the node starts.
pub fn take_kept(world: &WorldDir) -> Result<(), String> {
    let url = kept(world).map_err(|error| format!("reading the proxy kept: {error}"))?;
    fastctx_net::proxy::choose(url.as_deref()).map(|_| ())
}

/// One setting at a time, so the file kept and the proxy in use agree.
static SETTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Sets the proxy to `url`, or no longer any with none, keeps it, and dials
/// every hub again by it at once. Returns the proxy as it is now.
pub async fn set(node: &Arc<Node>, url: Option<String>) -> Result<Option<String>, Failure> {
    let _setting = SETTING.lock().await;
    let url = url.map(|url| url.trim().to_string()).filter(|url| !url.is_empty());
    if let Some(url) = &url {
        fastctx_net::proxy::check(url).map_err(Failure::new)?;
    }
    let world = node.world.clone();
    let keeping = url.clone();
    tokio::task::spawn_blocking(move || -> io::Result<()> {
        match keeping {
            Some(url) => {
                fastctx_platform::ensure_private_dir(&world.net())?;
                let bytes = serde_json::to_vec_pretty(&Kept { url }).map_err(io::Error::other)?;
                fastctx_platform::atomic::publish(&file(&world), &bytes, fastctx_platform::Permissions::Private)
            }
            None => match std::fs::remove_file(file(&world)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            },
        }
    })
    .await
    .map_err(|error| Failure::new(format!("keeping the proxy: {error}")))?
    .map_err(|error| Failure::new(format!("keeping the proxy: {error}")))?;
    fastctx_net::proxy::choose(url.as_deref()).map_err(Failure::new)?;
    let said = url.as_deref().map_or_else(|| "the system's".to_string(), fastctx_net::proxy::shown);
    node.log.line(format_args!("the proxy to the hubs is {said}"));
    for link in node.links() {
        link.reconnect();
    }
    Ok(url)
}
