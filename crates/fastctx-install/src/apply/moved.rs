//! A login moved to another disk (`fastctx login move`): its directory set
//! aside, copied there, and a link left where it was, so every path written
//! to it holds. What an apply keeps by the directory as it resolves — the
//! Claude receipts, named after it, and the value the entry's state was
//! found by — goes by where it resolves to now. The move holds the apply
//! lock throughout, and waits for no apply left unfinished.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::Error;

/// Nothing where every move of a login's directory recorded in the World
/// at `world` is settled; why not otherwise (`fastctx_node::login_moves`,
/// read here by the phase alone).
pub fn settled(world: &Path) -> Result<(), Error> {
    #[derive(serde::Deserialize)]
    struct Phase {
        phase: String,
    }
    let directory = world.join("login-moves");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::Io(format!("reading {}: {error}", directory.display()))),
    };
    for entry in entries {
        let path = entry.map_err(|error| Error::Io(format!("reading {}: {error}", directory.display())))?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let read = fastctx_platform::disk::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice::<Phase>(&bytes).map_err(|error| error.to_string()));
        match read {
            Ok(Phase { phase }) if phase == "done" || phase == "aborted" => {}
            Ok(_) => {
                return Err(Error::Io(format!(
                    "a login's directory is being moved, or its move was cut short ({}); finish it with `fastctx move-login --resume` first",
                    path.display()
                )));
            }
            Err(error) => return Err(Error::Io(format!("reading {}: {error}", path.display()))),
        }
    }
    Ok(())
}

/// What an apply under `root` left unfinished, each by its record: a move
/// takes place only with none.
pub fn unfinished(root: &Path) -> Result<Vec<String>, Error> {
    let mut left = Vec::new();
    for name in ["applied-files-pending.json", "applied-claude-entry-undo.json"] {
        let path = root.join(name);
        if path.try_exists().map_err(|error| Error::Io(format!("checking {}: {error}", path.display())))? {
            left.push(path.display().to_string());
        }
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(left),
        Err(error) => return Err(Error::Io(format!("reading {}: {error}", root.display()))),
    };
    for entry in entries {
        let entry = entry.map_err(|error| Error::Io(format!("reading {}: {error}", root.display())))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("applied-claude-plugin") && name.ends_with(".json")) {
            continue;
        }
        if super::load_plugin_record(&entry.path())?.is_some_and(|record| record.operation.is_some()) {
            left.push(entry.path().display().to_string());
        }
    }
    left.sort();
    Ok(left)
}

/// The Claude receipts of the home that resolved to `from` follow it to
/// `to`, where it resolves now: renamed after it, and the value its entry's
/// state was found by, where that was the home itself, made `to`. Under the
/// apply lock, held by the caller; done again, it changes nothing more.
pub fn moved(root: &Path, from: &Path, to: &Path) -> Result<Vec<String>, Error> {
    let mut done = Vec::new();
    for kind in ["plugin", "entry"] {
        let old = root.join(format!("applied-claude-{kind}-{:x}.json", Sha256::digest(spelled(from)?.as_bytes())));
        let new = root.join(format!("applied-claude-{kind}-{:x}.json", Sha256::digest(spelled(to)?.as_bytes())));
        let there = |path: &Path| {
            path.try_exists().map_err(|error| Error::Io(format!("checking {}: {error}", path.display())))
        };
        if !there(&old)? {
            continue;
        }
        if there(&new)? {
            return Err(Error::Io(format!(
                "{} and {} both name the moved home's receipt; neither is taken for the other",
                old.display(),
                new.display()
            )));
        }
        fastctx_platform::disk::rename(&old, &new)
            .map_err(|error| Error::Io(format!("renaming {}: {error}", old.display())))?;
        fastctx_platform::durable_file::sync_directory(new.parent().expect("a receipt's directory"))
            .map_err(|error| Error::Io(format!("keeping {}: {error}", new.display())))?;
        done.push(format!("{} -> {}", old.display(), new.display()));
    }
    let entry = root.join(format!("applied-claude-entry-{:x}.json", Sha256::digest(spelled(to)?.as_bytes())));
    if super::state::moved(&entry, from, to)? {
        done.push(format!("{}: found by {}", entry.display(), to.display()));
    }
    Ok(done)
}

/// Why the receipts of a home resolving to `from` could not follow it to
/// `to`: a receipt named after `to` is there already.
pub fn conflicts(root: &Path, from: &Path, to: &Path) -> Result<(), Error> {
    for kind in ["plugin", "entry"] {
        let old = root.join(format!("applied-claude-{kind}-{:x}.json", Sha256::digest(spelled(from)?.as_bytes())));
        let new = root.join(format!("applied-claude-{kind}-{:x}.json", Sha256::digest(spelled(to)?.as_bytes())));
        if old.exists() && new.exists() {
            return Err(Error::Io(format!(
                "{} is there already, for a home at {}; the moved home's receipt could not follow it",
                new.display(),
                to.display()
            )));
        }
    }
    Ok(())
}

/// A home's spelling as the receipts are named by it (`homes::identity`),
/// of a path already resolved.
fn spelled(home: &Path) -> Result<String, Error> {
    let text = home.to_str().ok_or_else(|| Error::Io("host home must be a UTF-8 path".into()))?;
    Ok(if cfg!(windows) { text.replace('\\', "/").trim_start_matches("//?/").to_owned() } else { text.to_owned() })
}
