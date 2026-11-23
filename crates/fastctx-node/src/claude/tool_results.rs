//! Claude Code keeps, beside each session's record, the original bytes of
//! every picture an MCP tool answered with
//! (`<session>/tool-results/mcp-<server>-blob-<ms>-<random>.<extension>`).
//! What the model saw is in the record itself, and neither Claude Code
//! resuming the session nor the World showing it reads these files. Claude
//! Code lets them go after its own retention, which it skips for a session
//! started with local settings alone, as an assisted one is. Of the sessions
//! the World runs, these files go here once five days old: a few at a time,
//! never while the conversation is at work, and only on this machine.

use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use fastctx_hosts::claude::exchange;
use fastctx_types::ResidentId;

use crate::Node;

/// How long such a picture is kept after Claude Code wrote it.
const KEPT: Duration = Duration::from_secs(5 * 24 * 60 * 60);
/// How often the sessions are looked at.
const EVERY: Duration = Duration::from_secs(60 * 60);
/// The most files let go of in one look.
const AT_ONCE: usize = 500;
/// After so many files, a breath, so that the disk is not kept busy.
const BATCH: usize = 50;
const BREATH: Duration = Duration::from_millis(200);

/// Starts looking at the sessions here every hour.
pub(crate) fn start(node: &Arc<Node>) {
    let weak: Weak<Node> = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move {
        // Machines updated together do not all begin at the same moment.
        let spread = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| u64::from(since.subsec_nanos()) % EVERY.as_secs());
        tokio::time::sleep(Duration::from_secs(spread)).await;
        loop {
            let Some(node) = weak.upgrade() else { return };
            match look(&node).await {
                Ok(0) => {}
                Ok(gone) => node.log.line(format_args!(
                    "let go of {gone} picture file(s) Claude Code kept beside its sessions' records, five days old"
                )),
                Err(error) => node.log.line(format_args!("letting go of Claude Code's old picture files: {error}")),
            }
            drop(node);
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// One look at every Claude session the World knows here; how many files went.
async fn look(node: &Arc<Node>) -> std::io::Result<usize> {
    let root = exchange::sessions(node.world.path());
    let sessions = fastctx_platform::task::blocking(move || sessions(&root)).await?;
    let mut gone = 0;
    for (directory, owner) in sessions {
        if gone >= AT_ONCE {
            break;
        }
        let Some((about, _)) = node.residents.about(&owner) else { continue };
        if !about.world_managed() || about.host.agent.as_str() != "claude" {
            continue;
        }
        // A conversation at work, or one whose state cannot be read, is left
        // alone this time.
        if crate::lifecycle::held(node, &owner).await.unwrap_or(true) {
            continue;
        }
        let most = AT_ONCE - gone;
        gone += fastctx_platform::task::blocking(move || clear(&directory, most)).await?;
    }
    Ok(gone)
}

/// Each session directory under `root` with the resident it belongs to.
fn sessions(root: &Path) -> std::io::Result<Vec<(std::path::PathBuf, ResidentId)>> {
    let entries = match fastctx_platform::disk::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let directory = entry.path();
        let Ok(bytes) = fastctx_platform::disk::read(directory.join("resident.json")) else { continue };
        let Ok(owner) = serde_json::from_slice::<ResidentId>(&bytes) else { continue };
        found.push((directory, owner));
    }
    Ok(found)
}

/// Lets go of at most `most` pictures five days old beside the record of the
/// session whose exchange directory is `directory`; how many went.
fn clear(directory: &Path, most: usize) -> std::io::Result<usize> {
    let Some(transcript) = exchange::transcript(directory)? else { return Ok(0) };
    let folder = transcript.with_extension("").join("tool-results");
    let entries = match fastctx_platform::disk::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let Some(cutoff) = SystemTime::now().checked_sub(KEPT) else { return Ok(0) };
    let mut gone = 0;
    for entry in entries.flatten() {
        if gone >= most {
            break;
        }
        if !entry.file_name().to_str().is_some_and(picture) {
            continue;
        }
        let Ok(metadata) = fastctx_platform::disk::symlink_metadata(entry.path()) else { continue };
        // Written within the five days, or at a time still to come.
        if !metadata.is_file() || !metadata.modified().is_ok_and(|written| written <= cutoff) {
            continue;
        }
        match fastctx_platform::disk::remove_file(entry.path()) {
            Ok(()) => gone += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if gone % BATCH == 0 {
            std::thread::sleep(BREATH);
        }
    }
    Ok(gone)
}

/// Whether `name` is a picture an MCP tool answered with, as Claude Code names
/// the copy it keeps: `mcp-<server>-blob-<ms>-<random>.<picture extension>`.
fn picture(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("mcp-") else { return false };
    let Some((stem, extension)) = rest.rsplit_once('.') else { return false };
    stem.contains("-blob-")
        && matches!(extension.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
}
