//! Host isolation is a property of a conversation's birth, not a live
//! directory view: a hub may not have sent that view when a node restarts.

use fastctx_types::{ResidentId, access::Acting, ipc::Failure};

use crate::Node;

fn path(node: &Node, resident: &ResidentId) -> std::path::PathBuf {
    node.world.conversation(resident).path().join("host-isolation.json")
}

pub(crate) fn kept(node: &Node, resident: &ResidentId) -> Result<Option<bool>, Failure> {
    match std::fs::read(path(node, resident)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| Failure::new("invalid conversation host-isolation record")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Failure::new(format!("reading conversation host isolation: {error}"))),
    }
}

/// Records a new conversation's host mode; an older stored session remains
/// in its original host until it is explicitly replaced.
pub(crate) fn starting(node: &Node, resident: &ResidentId, acting: &Acting) -> Result<bool, Failure> {
    if let Some(assisted) = kept(node, resident)? {
        return Ok(assisted);
    }
    if node.residents.about(resident).is_some_and(|(about, _)| about.session.is_some()) {
        return Ok(false);
    }
    let assisted = super::assisting(node, acting);
    let path = path(node, resident);
    fastctx_platform::ensure_private_dir(path.parent().expect("conversation directory"))
        .and_then(|()| {
            fastctx_platform::publish(
                &path,
                if assisted { b"true" } else { b"false" },
                fastctx_platform::Permissions::Private,
            )
        })
        .map_err(|error| Failure::new(format!("recording conversation host isolation: {error}")))?;
    Ok(assisted)
}

/// A fork stays on the app-server that actually owns its parent thread.
pub(crate) fn inherit(node: &Node, assisted: bool, child: &ResidentId) -> Result<(), Failure> {
    let path = path(node, child);
    fastctx_platform::ensure_private_dir(path.parent().expect("conversation directory"))
        .and_then(|()| {
            fastctx_platform::publish(
                &path,
                if assisted { b"true" } else { b"false" },
                fastctx_platform::Permissions::Private,
            )
        })
        .map_err(|error| Failure::new(format!("recording the fork's host isolation: {error}")))
}

pub(crate) fn require(node: &Node, resident: &ResidentId) -> Result<(), Failure> {
    if kept(node, resident)? == Some(true) {
        return Ok(());
    }
    Err(legacy())
}

pub(super) fn legacy() -> Failure {
    Failure::not_run(
        "This assisted conversation predates host isolation. Start a new conversation to continue without the host owner's instructions and extensions.",
    )
}
