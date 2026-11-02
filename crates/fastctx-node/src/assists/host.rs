//! Host isolation is a property of a conversation's birth, not a live
//! directory view: a hub may not have sent that view when a node restarts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::{ResidentId, access::Acting, ipc::Failure};

use crate::Node;

/// How long a conversation found with no record of its host mode is taken
/// to have none before its directory is looked at again.
const ABSENT_FOR: Duration = Duration::from_secs(10);

/// Each conversation's host mode as last read or recorded, by its record's
/// path: fixed at its birth and asked for whenever its host is found, it is
/// read from its disk once.
static KEPT: LazyLock<Mutex<HashMap<PathBuf, (Option<bool>, Instant)>>> = LazyLock::new(Default::default);

fn path(node: &Node, resident: &ResidentId) -> PathBuf {
    node.world.conversation(resident).path().join("host-isolation.json")
}

pub(crate) fn kept(node: &Node, resident: &ResidentId) -> Result<Option<bool>, Failure> {
    let path = path(node, resident);
    if let Some((kept, at)) = KEPT.lock().expect("never poisoned").get(&path).copied()
        && (kept.is_some() || at.elapsed() < ABSENT_FOR)
    {
        return Ok(kept);
    }
    let kept = match fastctx_platform::disk::read(&path) {
        Ok(bytes) => Some(
            serde_json::from_slice(&bytes).map_err(|_| Failure::new("invalid conversation host-isolation record"))?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(Failure::new(format!("reading conversation host isolation: {error}"))),
    };
    KEPT.lock().expect("never poisoned").insert(path, (kept, Instant::now()));
    Ok(kept)
}

fn record(node: &Node, resident: &ResidentId, assisted: bool) -> std::io::Result<()> {
    let path = path(node, resident);
    fastctx_platform::ensure_private_dir(path.parent().expect("conversation directory"))?;
    fastctx_platform::publish(
        &path,
        if assisted { b"true" } else { b"false" },
        fastctx_platform::Permissions::Private,
    )?;
    KEPT.lock().expect("never poisoned").insert(path, (Some(assisted), Instant::now()));
    Ok(())
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
    let assisted = super::assisting_known(node, acting)?;
    record(node, resident, assisted)
        .map_err(|error| Failure::new(format!("recording conversation host isolation: {error}")))?;
    Ok(assisted)
}

/// A fork stays on the app-server that actually owns its parent thread.
pub(crate) fn inherit(node: &Node, assisted: bool, child: &ResidentId) -> Result<(), Failure> {
    record(node, child, assisted).map_err(|error| Failure::new(format!("recording the fork's host isolation: {error}")))
}

/// The host mode of a conversation an agent here works in for another
/// account. One Claude Code opened before host isolation existed goes on
/// isolated: Claude's isolation is the session's own settings in the same
/// login, so it resumes there with its whole history (`world-lifecycle.md`).
/// A Codex thread stays on the app-server that holds it.
pub(crate) fn assisted(node: &Node, resident: &ResidentId) -> Result<Option<bool>, Failure> {
    if let Some(assisted) = kept(node, resident)? {
        return Ok(Some(assisted));
    }
    let claude = node.residents.about(resident).is_some_and(|(about, _)| about.host.agent.as_str() == "claude");
    if !claude {
        return Ok(None);
    }
    if let Some(session) = node.residents.about(resident).and_then(|(about, _)| about.session) {
        crate::claude::isolation::begin(node, resident, &session)?;
    }
    record(node, resident, true)
        .map_err(|error| Failure::new(format!("recording conversation host isolation: {error}")))?;
    node.log.line(format_args!("{resident} began before host isolation; preparing an isolated continuation"));
    Ok(Some(true))
}

/// Whether such a conversation runs isolated, or will when it next runs
/// (`assisted`), recording nothing.
pub(crate) fn isolated(node: &Node, resident: &ResidentId) -> Result<bool, Failure> {
    Ok(match kept(node, resident)? {
        Some(assisted) => assisted,
        None => node.residents.about(resident).is_some_and(|(about, _)| about.host.agent.as_str() == "claude"),
    })
}

pub(crate) fn require(node: &Node, resident: &ResidentId) -> Result<(), Failure> {
    if assisted(node, resident)? == Some(true) {
        return Ok(());
    }
    Err(legacy())
}

pub(super) fn legacy() -> Failure {
    Failure::not_run(
        "This assisted conversation predates host isolation. Start a new conversation to continue without the host owner's instructions and extensions.",
    )
}
