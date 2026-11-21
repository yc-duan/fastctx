//! The records of the moves of logins' directories to other disks, as
//! `fastctx move-login` keeps them in `<world>/login-moves/<op>.json`: the
//! directory copied there, the original set aside, a link left where it was
//! (`specs`: the user, 2026-11-21, "他要搬迁过去，但是允许你在原位置有一些残留").
//! A move is made with the node stopped and its lock held; the node starts on
//! a World none is unsettled in, and a login a finished move took keeps its
//! id (`super::ids`).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the records are, in the World.
pub const DIRECTORY: &str = "login-moves";

/// How far a move went. Until `Done` or `Aborted` the directory may stand
/// between its two places, and nothing uses the login.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Recorded; the node's service being stopped.
    Stopping,
    /// Who uses the directory is being looked at.
    Checking,
    /// Copied into the staging directory beside the target.
    Copying,
    /// The copy checked against the original.
    Verifying,
    /// The copy is in the target's place; the original untouched.
    Published,
    /// The original being set aside and the link put in its place: from the
    /// first rename on, the move goes forward.
    Swapping,
    /// The link stands; the receipts kept by the directory's real place
    /// follow it.
    Linked,
    /// Moved whole.
    Done,
    /// Given up before the original was touched: it stands as it was.
    Aborted,
}

impl Phase {
    /// Whether the directory stands whole in one place: untouched, or moved.
    pub fn settled(self) -> bool {
        matches!(self, Self::Done | Self::Aborted)
    }
}

/// How the node's service stood before the move stopped it, to be put back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceBefore {
    pub name: String,
    pub enabled: bool,
    pub active: bool,
}

/// A directory, as a filesystem knows it: its device and inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub device: u64,
    pub inode: u64,
}

/// What was copied, to check the original against before it is deleted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copied {
    pub entries: u64,
    pub bytes: u64,
    /// The digest of every entry's path, kind, owner, mode, time, size and
    /// content, in order.
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub op: String,
    /// `claude` or `codex`.
    pub agent: String,
    /// The login as the node knows it.
    pub login: PathBuf,
    /// The directory it resolved to: moved, and where the link is left.
    pub source: PathBuf,
    pub source_identity: Identity,
    /// Where it goes.
    pub target: PathBuf,
    /// The copy in the target's place, once published.
    #[serde(default)]
    pub target_identity: Option<Identity>,
    /// Beside the target, while it is copied.
    pub staging: PathBuf,
    /// Beside the source: the original, set aside.
    pub backup: PathBuf,
    /// The service that kept the node, as it stood; none where none did.
    #[serde(default)]
    pub service: Option<ServiceBefore>,
    /// The service is as it was before the move.
    #[serde(default)]
    pub service_restored: bool,
    pub phase: Phase,
    #[serde(default)]
    pub copied: Option<Copied>,
    /// Why it was given up, or what was left to the operator.
    #[serde(default)]
    pub why: Option<String>,
    pub at: jiff::Timestamp,
    /// The backup was deleted.
    #[serde(default)]
    pub backup_deleted: bool,
}

/// The records kept, each with its file; one that cannot be read is an
/// error, never passed over.
pub fn records(world: &Path) -> io::Result<Vec<(PathBuf, Record)>> {
    let directory = world.join(DIRECTORY);
    let entries = match fastctx_platform::disk::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io::Error::new(error.kind(), format!("reading {}: {error}", directory.display()))),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let bytes = fastctx_platform::durable_file::read_bounded(&path, 1 << 20)?;
        let record: Record = serde_json::from_slice(&bytes).map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("reading {}: {error}", path.display()))
        })?;
        found.push((path, record));
    }
    found.sort_by(|a, b| a.1.at.cmp(&b.1.at).then_with(|| a.0.cmp(&b.0)));
    Ok(found)
}

/// Whether `record` leaves something to do: the directory between its two
/// places, or the service not put back as it was.
pub fn pending(record: &Record) -> bool {
    !record.phase.settled() || !record.service_restored
}

/// The record of a move not settled, if any; a record that cannot be read
/// counts as one.
pub fn unsettled(world: &Path) -> Option<String> {
    match records(world) {
        Ok(records) => records.into_iter().find(|(_, record)| !record.phase.settled()).map(|(path, record)| {
            format!(
                "the move of the login {} to {} is unfinished ({}); finish it with `fastctx move-login --resume`",
                record.login.display(),
                record.target.display(),
                path.display()
            )
        }),
        Err(error) => Some(format!("the records of login moves cannot be read: {error}")),
    }
}

/// The most moves one login is followed through.
pub const MOST_MOVES: usize = 1024;

/// A finished move: the directory it took away, as it was, a link left
/// there, and where it put the copy, as that directory was once published.
pub struct Finished {
    pub source: PathBuf,
    pub source_identity: Identity,
    pub target: PathBuf,
    pub target_identity: Option<Identity>,
}

/// The moves finished on this World; an error where the records cannot be read.
pub fn finished(world: &Path) -> io::Result<Vec<Finished>> {
    Ok(records(world)?
        .into_iter()
        .filter(|(_, record)| record.phase == Phase::Done)
        .map(|(_, record)| Finished {
            source: record.source,
            source_identity: record.source_identity,
            target: record.target,
            target_identity: record.target_identity,
        })
        .collect())
}

/// Whether the directory `from` was moved, by one finished move or one after
/// another, to `real`, the very directory a move published there: each move
/// after the first took the very directory the one before it published.
pub fn led_to(finished: &[Finished], from: &Path, real: &Path) -> bool {
    let mut at = from.to_path_buf();
    let mut published: Option<Identity> = None;
    for _ in 0..MOST_MOVES {
        let Some(move_) = finished
            .iter()
            .find(|move_| move_.source == at && published.is_none_or(|published| published == move_.source_identity))
        else {
            return false;
        };
        if move_.target == real {
            return move_.target_identity.is_none_or(|identity| identity_of(real).is_ok_and(|now| now == identity));
        }
        let Some(identity) = move_.target_identity else { return false };
        published = Some(identity);
        at = move_.target.clone();
    }
    false
}

/// A directory's identity, not followed through a link.
pub fn identity_of(path: &Path) -> io::Result<Identity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fastctx_platform::disk::symlink_metadata(path)?;
        Ok(Identity { device: metadata.dev(), inode: metadata.ino() })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(io::Error::new(io::ErrorKind::Unsupported, "a directory's identity is read on Unix"))
    }
}

/// Keeps `record` in its file, on disk before it is answered.
pub fn keep(path: &Path, record: &Record) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if let Some(parent) = path.parent() {
        fastctx_platform::ensure_private_dir(parent)?;
    }
    fastctx_platform::publish(path, &bytes, fastctx_platform::Permissions::Private)?;
    if let Some(parent) = path.parent() {
        fastctx_platform::durable_file::sync_directory(parent)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
pub(crate) fn finished_for_test(world: &Path, source: &Path, target: &Path) {
    let op = format!("test-{}", records(world).map(|records| records.len()).unwrap_or(0));
    let record = Record {
        op: op.clone(),
        agent: "claude".into(),
        login: source.to_path_buf(),
        source: source.to_path_buf(),
        source_identity: finished(world)
            .ok()
            .and_then(|done| done.into_iter().find(|move_| move_.target == source).and_then(|move_| move_.target_identity))
            .unwrap_or(Identity { device: 0, inode: 0 }),
        target: fastctx_platform::disk::canonicalize(target).expect("a target"),
        target_identity: Some(identity_of(target).expect("a target")),
        staging: target.with_extension("staging"),
        backup: source.with_extension("moved"),
        service: None,
        service_restored: true,
        phase: Phase::Done,
        copied: None,
        why: None,
        at: jiff::Timestamp::now(),
        backup_deleted: false,
    };
    keep(&world.join(DIRECTORY).join(format!("{op}.json")), &record).expect("a record");
}
