//! Claude Code keeps, beside each session's record, the original bytes of
//! every picture an MCP tool answered with
//! (`<session>/tool-results/mcp-<server>-blob-<ms>-<random>.<extension>`).
//! What the model saw is in the record itself, the picture block followed
//! by a text block naming the file (`[Image: source: <path>…]`), and neither
//! Claude Code resuming the session nor the World showing it reads these
//! files. Claude Code lets them go after its own retention, which it skips
//! for a session started with local settings alone, as an assisted one is.
//! Of the sessions the World runs, these files go here once five days old: a
//! few at a time, never while the conversation is at work, only on this
//! machine, and only those whose picture the record holds too. One the
//! record names with no picture before it (Claude Code could not shrink it,
//! or left it out of the answer) is the only copy, and stays.

use std::collections::{HashMap, HashSet};
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
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
/// What begins the text block naming a picture's file after its block.
const NAMED: &str = "[Image: source: ";
/// The most failures one look tells of.
const FAILURES_TOLD: usize = 4;

/// The pictures each record was found to hold, while it stays as it was
/// read: a record whose pictures stay is not read again every hour.
struct Read {
    length: u64,
    written: SystemTime,
    held: HashSet<String>,
}

static READ: Mutex<Option<HashMap<PathBuf, Read>>> = Mutex::new(None);
/// The most records remembered so.
const READ_KEPT: usize = 512;

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
            let (gone, mut failures) = look(&node).await;
            if gone > 0 {
                node.log.line(format_args!(
                    "let go of {gone} picture file(s) Claude Code kept beside its sessions' records, five days old"
                ));
            }
            if !failures.is_empty() {
                let more = failures.len().saturating_sub(FAILURES_TOLD);
                failures.truncate(FAILURES_TOLD);
                let more = if more > 0 { format!("; and {more} more") } else { String::new() };
                node.log
                    .line(format_args!("letting go of Claude Code's old picture files: {}{more}", failures.join("; ")));
            }
            drop(node);
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// One look at every Claude session the World knows here: how many files
/// went, and what could not be done, each session apart from the others.
async fn look(node: &Arc<Node>) -> (usize, Vec<String>) {
    let root = exchange::sessions(node.world.path());
    let sessions = match fastctx_platform::task::blocking(move || sessions(&root)).await {
        Ok(sessions) => sessions,
        Err(error) => return (0, vec![error.to_string()]),
    };
    let (mut gone, mut failures) = (0, Vec::new());
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
        match fastctx_platform::task::blocking(move || clear(&directory, most)).await {
            Ok((went, failed)) => {
                gone += went;
                failures.extend(failed.map(|failed| format!("{owner}: {failed}")));
            }
            Err(error) => failures.push(format!("{owner}: {error}")),
        }
    }
    (gone, failures)
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
/// session whose exchange directory is `directory`, of those the record
/// holds too; how many went, and the first that could not.
fn clear(directory: &Path, most: usize) -> std::io::Result<(usize, Option<String>)> {
    let Some(transcript) = exchange::transcript(directory)? else { return Ok((0, None)) };
    let folder = transcript.with_extension("").join("tool-results");
    let entries = match fastctx_platform::disk::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, None)),
        Err(error) => return Err(error),
    };
    let Some(cutoff) = SystemTime::now().checked_sub(KEPT) else { return Ok((0, None)) };
    let mut old = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().filter(|name| picture(name)).map(str::to_string) else {
            continue;
        };
        let Ok(metadata) = fastctx_platform::disk::symlink_metadata(entry.path()) else { continue };
        // Written within the five days, or at a time still to come.
        if metadata.is_file() && metadata.modified().is_ok_and(|written| written <= cutoff) {
            old.push((name, entry.path()));
        }
    }
    if old.is_empty() {
        return Ok((0, None));
    }
    let held = held_inline(&transcript)?;
    let (mut gone, mut failed) = (0, None);
    for (name, path) in old {
        if gone >= most {
            break;
        }
        if !held.contains(&name) {
            continue;
        }
        match fastctx_platform::disk::remove_file(&path) {
            Ok(()) => {
                gone += 1;
                if gone % BATCH == 0 {
                    std::thread::sleep(BREATH);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                failed.get_or_insert_with(|| format!("{}: {error}", path.display()));
            }
        }
    }
    Ok((gone, failed))
}

/// The files of the pictures the record `transcript` holds itself: each one
/// a text block names right after a picture block with its bytes. Read once
/// while the record stays as it was.
fn held_inline(transcript: &Path) -> std::io::Result<HashSet<String>> {
    let metadata = fastctx_platform::disk::metadata(transcript)?;
    let (length, written) = (metadata.len(), metadata.modified()?);
    {
        let read = READ.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(found) = read.as_ref().and_then(|read| read.get(transcript))
            && found.length == length
            && found.written == written
        {
            return Ok(found.held.clone());
        }
    }
    let mut held = HashSet::new();
    let lines = std::io::BufReader::new(fastctx_platform::disk::open(transcript)?).split(b'\n');
    for line in lines {
        let line = line?;
        // Only the lines naming a picture are read whole.
        if !line.windows(NAMED.len()).any(|window| window == NAMED.as_bytes()) {
            continue;
        }
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) {
            pictures_in(&value, &mut held);
        }
    }
    let mut read = READ.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let read = read.get_or_insert_with(HashMap::new);
    if read.len() >= READ_KEPT {
        read.clear();
    }
    read.insert(transcript.to_path_buf(), Read { length, written, held: held.clone() });
    Ok(held)
}

/// Adds to `held` the file of each picture `value` holds with its bytes, as
/// the text block right after its picture block names it.
fn pictures_in(value: &serde_json::Value, held: &mut HashSet<String>) {
    match value {
        serde_json::Value::Array(blocks) => {
            for pair in blocks.windows(2) {
                if with_bytes(&pair[0])
                    && let Some(name) = named(&pair[1])
                {
                    held.insert(name);
                }
            }
            blocks.iter().for_each(|block| pictures_in(block, held));
        }
        serde_json::Value::Object(fields) => fields.values().for_each(|field| pictures_in(field, held)),
        _ => {}
    }
}

/// Whether `block` is a picture with its bytes.
fn with_bytes(block: &serde_json::Value) -> bool {
    block.get("type").and_then(serde_json::Value::as_str) == Some("image")
        && block
            .get("source")
            .and_then(|source| source.get("data"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|data| !data.is_empty())
}

/// The file a text block names a picture by: `[Image: source: <path>]`, or
/// with what Claude Code did to it after a comma.
fn named(block: &serde_json::Value) -> Option<String> {
    if block.get("type").and_then(serde_json::Value::as_str) != Some("text") {
        return None;
    }
    let rest = block.get("text")?.as_str()?.strip_prefix(NAMED)?;
    let path = match rest.split_once(", ") {
        Some((path, _)) => path,
        None => rest.strip_suffix(']')?,
    };
    path.rsplit(['/', '\\']).next().filter(|name| picture(name)).map(str::to_string)
}

/// Whether `name` is a picture an MCP tool answered with, as Claude Code names
/// the copy it keeps: `mcp-<server>-blob-<ms>-<random>.<picture extension>`.
fn picture(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("mcp-") else { return false };
    let Some((stem, extension)) = rest.rsplit_once('.') else { return false };
    stem.contains("-blob-")
        && matches!(extension.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
}

#[cfg(test)]
#[path = "tool_results_tests.rs"]
mod tests;
