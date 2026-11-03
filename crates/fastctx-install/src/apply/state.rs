//! Claude Code's global state for the selected Claude home, where a login
//! the workbench is not open in has Fastctx's server entered
//! (`crate::claude::entry`): which state is the home's, the entry's receipt
//! kept beside the apply record, and the entry written and taken out under
//! Claude Code's own lock. The state is Claude Code's own, busy, and may be a
//! link, so it is never part of the apply's file transaction: an entry put in
//! before a transaction that fails is taken out again, and one taken out is
//! put back.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use fastctx_hosts::claude::trust;
use serde::{Deserialize, Serialize};

use super::{Error, Places, homes, io};
use crate::Notice;
use crate::claude::entry::{self, Receipt};
use crate::transaction::{FileChange, Snapshot};

/// The receipt file as it is kept.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Kept {
    version: u32,
    entry: Receipt,
}

const VERSION: u32 = 1;

/// The value of `CLAUDE_CONFIG_DIR` the selected home's sessions start with:
/// the user's environment's for the home it opens by default, the home
/// itself for any other.
pub(super) fn configured(places: &Places) -> Result<Option<OsString>, Error> {
    let default = match &places.claude_config_dir {
        Some(dir) => dir.clone(),
        None => fastctx_platform::user_home().map_err(io("finding the home directory"))?.join(".claude"),
    };
    Ok(if homes::same_home(&places.claude_home, &default)? {
        places.claude_config_dir.clone().map(PathBuf::into_os_string)
    } else {
        // One spelling of the home, however it was given, so that every
        // change works out the same entry: a home handed to another build
        // arrives resolved, and on Windows in its verbatim form.
        let home = std::fs::canonicalize(&places.claude_home).unwrap_or_else(|_| places.claude_home.clone());
        Some(fastctx_hosts::inventory::plain(&home).into_os_string())
    })
}

/// The receipt of the selected home's entry as kept, and the file it is
/// kept in.
pub(super) fn receipt(places: &Places) -> Result<(Snapshot, Option<Receipt>), Error> {
    let file = Snapshot::read(homes::entry_record(places)?)?;
    let receipt = match file.bytes() {
        None => None,
        Some(bytes) => {
            let kept: Kept = serde_json::from_slice(bytes)
                .map_err(|error| Error::Io(format!("reading {}: {error}", file.path().display())))?;
            if kept.version != VERSION {
                return Err(Error::Io(format!(
                    "{}: unsupported entry record version {}",
                    file.path().display(),
                    kept.version
                )));
            }
            Some(kept.entry)
        }
    };
    Ok((file, receipt))
}

/// The change that keeps `receipt` in its file, or takes the file away
/// with none; nothing where there is no file to take away.
pub(super) fn keep(file: Snapshot, receipt: Option<&Receipt>) -> Result<Option<FileChange>, Error> {
    Ok(match receipt {
        Some(entry) => {
            let bytes = serde_json::to_vec_pretty(&Kept { version: VERSION, entry: entry.clone() })
                .map_err(|error| Error::Io(error.to_string()))?;
            Some(FileChange::write(file, bytes))
        }
        None if file.exists() => Some(FileChange::delete(file)),
        None => None,
    })
}

/// The selected home's state as it is, and as a change would leave it.
pub(super) struct Planned {
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

/// What entering the server would make of the selected home's state, with
/// `previous` the receipt of the entry there: for a preview, nothing written.
pub(super) fn plan_put(places: &Places, previous: Option<&Receipt>, take_back: bool) -> Result<Planned, Error> {
    let configured = configured(places)?;
    let wanted = wanted(places, configured.as_deref())?;
    let (path, before) = read(configured.as_deref())?;
    let previous = previous.filter(|receipt| receipt.configured() == configured.as_deref());
    let (next, _) = entry::place(before.as_deref(), &text(&path), &wanted, configured.as_deref(), previous, take_back)?;
    let after = next.or_else(|| before.clone());
    Ok(Planned { path, before, after })
}

/// What taking out the entry `receipt` records would make of its state, and
/// why it would be left where the user changed it.
pub(super) fn plan_take(receipt: &Receipt, take_back: bool) -> Result<(Planned, Option<Notice>), Error> {
    let (path, before) = read(receipt.configured())?;
    let (next, left) = entry::unplace(before.as_deref(), &text(&path), receipt, take_back)?;
    let after = next.or_else(|| before.clone());
    Ok((Planned { path, before, after }, left))
}

/// The selected home's entry as its state holds it now, for a fingerprint:
/// the state itself changes with every session Claude Code opens.
pub(super) fn print(places: &Places) -> Result<(PathBuf, Option<Vec<u8>>), Error> {
    let (path, bytes) = read(configured(places)?.as_deref())?;
    let current = entry::current(bytes.as_deref(), &text(&path))?;
    Ok((path, current.map(|value| value.to_string().into_bytes())))
}

/// Enters the server in the selected home's state, or brings its entry to
/// what this fastctx writes; `previous` is its receipt. Answers the new
/// receipt, and whether the state changed.
pub(super) fn put(places: &Places, previous: Option<&Receipt>, take_back: bool) -> Result<(Receipt, bool), Error> {
    let configured = configured(places)?;
    // An entry in the state a home's earlier setting named goes first.
    let previous = match previous {
        Some(receipt) if receipt.configured() != configured.as_deref() => {
            take(receipt, false)?;
            None
        }
        other => other,
    };
    let wanted = wanted(places, configured.as_deref())?;
    let (path, _) = read(configured.as_deref())?;
    let location = text(&path);
    let mut made = None;
    let mut refused = None;
    let changed = trust::edit_state(configured.as_deref(), |bytes| {
        match entry::place(bytes, &location, &wanted, configured.as_deref(), previous, take_back) {
            Ok((next, receipt)) => {
                made = Some(receipt);
                Ok(next)
            }
            Err(error) => {
                refused = Some(error);
                Err(std::io::Error::other("refused"))
            }
        }
    });
    if let Some(error) = refused {
        return Err(error.into());
    }
    let changed = changed.map_err(io("entering Fastctx's server in Claude Code's global state"))?;
    Ok((made.expect("the entry was worked out"), changed.is_some()))
}

/// Takes the entry `receipt` records out of the state it is in, however the
/// user changed it with `take_back`; answers why it was left where the user
/// changed it since.
pub(super) fn take(receipt: &Receipt, take_back: bool) -> Result<Option<Notice>, Error> {
    let (path, _) = read(receipt.configured())?;
    let location = text(&path);
    let mut notice = None;
    let mut refused = None;
    let taken =
        trust::edit_state(receipt.configured(), |bytes| match entry::unplace(bytes, &location, receipt, take_back) {
            Ok((next, left)) => {
                notice = left;
                Ok(next)
            }
            Err(error) => {
                refused = Some(error);
                Err(std::io::Error::other("refused"))
            }
        });
    if let Some(error) = refused {
        return Err(error.into());
    }
    taken.map_err(io("taking Fastctx's server out of Claude Code's global state"))?;
    Ok(notice)
}

/// Puts back the entry `receipt` records, taken out by a change that did
/// not go through.
pub(super) fn restore(receipt: &Receipt) -> Result<(), Error> {
    let (path, _) = read(receipt.configured())?;
    let location = text(&path);
    let written = entry::written(receipt);
    let mut refused = None;
    let restored = trust::edit_state(receipt.configured(), |bytes| {
        match entry::place(bytes, &location, &written, receipt.configured(), Some(receipt), true) {
            Ok((next, _)) => Ok(next),
            Err(error) => {
                refused = Some(error);
                Err(std::io::Error::other("refused"))
            }
        }
    });
    if let Some(error) = refused {
        return Err(error.into());
    }
    restored.map_err(io("putting Fastctx's server back in Claude Code's global state"))?;
    Ok(())
}

/// The entry for the selected home, as this fastctx writes it.
fn wanted(places: &Places, configured: Option<&OsStr>) -> Result<serde_json::Value, Error> {
    let (command, root) =
        fastctx_hosts::claude::world_server(&places.world).map_err(io("finding the World's server"))?;
    Ok(entry::wanted(&command, &root, configured))
}

fn read(configured: Option<&OsStr>) -> Result<(PathBuf, Option<Vec<u8>>), Error> {
    trust::read_state(configured).map_err(io("reading Claude Code's global state"))
}

fn text(path: &std::path::Path) -> String {
    path.display().to_string()
}
