//! The hub's chain head kept outside its directory (design §17, item 2): a
//! small file in the service account's state, in no backup of the hub,
//! holding the latest (`hub_seq`, `hub_head`). A control fact reaches it,
//! and the disk, after its commit and before anyone is answered or told; a
//! checkpoint after its commit, at most a second late. An ordinary crash
//! leaves it older than the hub's databases, never newer, so a file newer
//! than them on opening means the hub's directory was put back.
//!
//! The owner libraries' heads are not kept here: each checkpoint is in the
//! chain, so the hub's database, once its chain holds this head, is what
//! an owner library is checked against when it opens.
//!
//! It only moves forward. Once the hub finds it was put back, the file is
//! left as it is, the evidence of the history that was lost.

use fastctx_platform::Permissions;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// What the file holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    pub seq: u64,
    pub head: String,
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

/// The file and what it holds now.
pub(crate) struct Witness {
    path: PathBuf,
    kept: Mutex<Kept>,
    frozen: AtomicBool,
}

/// An absolute directory named by the variable `name`, if it names one.
fn absolute(name: &str) -> io::Result<Option<PathBuf>> {
    let Some(set) = std::env::var_os(name) else { return Ok(None) };
    let path = PathBuf::from(set);
    if !path.is_absolute() {
        return Err(io::Error::other(format!("{name} names {}, not an absolute directory", path.display())));
    }
    Ok(Some(path))
}

/// Where the file of the hub `id` kept in `dir` lives: `FASTCTX_HUB_WITNESS_DIR`
/// when set; beside a hub kept in the system's temporary directory (a
/// drill's); otherwise the service account's state directory. Never inside
/// the hub's directory, where a copy of it would come back with the hub.
fn location(dir: &Path, id: &str) -> io::Result<PathBuf> {
    let base = if let Some(set) = absolute("FASTCTX_HUB_WITNESS_DIR")? {
        set
    } else if dir.starts_with(std::env::temp_dir()) {
        dir.parent().unwrap_or(dir).join("hub-witness")
    } else if cfg!(windows) {
        let state = match absolute("LOCALAPPDATA")? {
            Some(state) => Some(state),
            None => absolute("ProgramData")?,
        };
        state
            .ok_or_else(|| io::Error::other("neither LOCALAPPDATA nor ProgramData names where the hub keeps its witness"))?
            .join("fastctx")
            .join("hub-witness")
    } else {
        let state = match absolute("XDG_STATE_HOME")? {
            Some(state) => Some(state),
            None => absolute("HOME")?.map(|home| home.join(".local").join("state")),
        };
        state.unwrap_or_else(|| PathBuf::from("/var/lib")).join("fastctx").join("hub-witness")
    };
    let path = base.join(format!("{id}.json"));
    if inside(&path, dir)? {
        return Err(io::Error::other(format!(
            "the hub's witness would be kept in its own directory, as {}: set FASTCTX_HUB_WITNESS_DIR to a directory outside {}",
            path.display(),
            dir.display()
        )));
    }
    Ok(path)
}

/// `path` as the system resolves it: the longest part of it that exists,
/// resolved, with the rest after it.
fn resolved(path: &Path) -> io::Result<PathBuf> {
    let mut rest = Vec::new();
    let mut at = path.to_path_buf();
    loop {
        match fastctx_platform::disk::canonicalize(&at) {
            Ok(found) => {
                let mut whole = found;
                for part in rest.iter().rev() {
                    whole.push(part);
                }
                return Ok(whole);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let Some(name) = at.file_name().map(|name| name.to_os_string()) else { return Ok(path.to_path_buf()) };
                rest.push(name);
                if !at.pop() {
                    return Ok(path.to_path_buf());
                }
            }
            Err(error) => return Err(error),
        }
    }
}

/// Whether `path` lies within the directory `dir`.
fn inside(path: &Path, dir: &Path) -> io::Result<bool> {
    let (path, dir) = (resolved(path)?, resolved(dir)?);
    let parts = |path: &Path| -> Vec<String> {
        path.components()
            .filter(|part| !matches!(part, Component::CurDir))
            .map(|part| {
                let text = part.as_os_str().to_string_lossy().into_owned();
                if cfg!(windows) { text.to_lowercase() } else { text }
            })
            .collect()
    };
    let (path, dir) = (parts(&path), parts(&dir));
    Ok(path.len() > dir.len() && path[..dir.len()] == dir[..])
}

impl Witness {
    /// The witness of the hub `id` kept in `dir`, as last written.
    pub(crate) fn open(dir: &Path, id: &str) -> io::Result<Self> {
        let path = location(dir, id)?;
        let kept = match fastctx_platform::disk::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Kept::default(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, kept: Mutex::new(kept), frozen: AtomicBool::new(false) })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn kept(&self) -> Kept {
        self.kept.lock().expect("witness").clone()
    }

    /// The chain head the file holds: what the hub's words are bound to.
    pub(crate) fn current(&self) -> (u64, [u8; 32]) {
        let kept = self.kept.lock().expect("witness");
        (kept.seq, unhex(&kept.head).unwrap_or([0; 32]))
    }

    /// Leaves the file as it is from now on.
    pub(crate) fn freeze(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }

    /// The hub recovered and put its old history aside (design §17): the
    /// file holds the chain head as it is now, older or not.
    pub(crate) fn reset(&self, seq: u64, head: &[u8; 32]) -> io::Result<()> {
        let mut kept = self.kept.lock().expect("witness");
        let next = Kept { seq, head: hex(head) };
        if let Some(parent) = self.path.parent() {
            fastctx_platform::ensure_private_dir(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&next).map_err(io::Error::other)?;
        fastctx_platform::publish(&self.path, &bytes, Permissions::Private)?;
        *kept = next;
        Ok(())
    }

    /// The chain reached (`seq`, `head`): written and on the disk before it
    /// returns. An older head than the file's is left out.
    pub(crate) fn advanced(&self, seq: u64, head: &[u8; 32]) -> io::Result<()> {
        if self.frozen.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut kept = self.kept.lock().expect("witness");
        if seq <= kept.seq {
            return Ok(());
        }
        let next = Kept { seq, head: hex(head) };
        if let Some(parent) = self.path.parent() {
            fastctx_platform::ensure_private_dir(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(&next).map_err(io::Error::other)?;
        fastctx_platform::publish(&self.path, &bytes, Permissions::Private)?;
        *kept = next;
        Ok(())
    }
}
