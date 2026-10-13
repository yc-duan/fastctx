//! Explicit directory trust in Claude Code's global state (2.1.287–2.1.289).
//!
//! This is separate from tool permissions. The caller must supply a directory
//! owned by the World, normally its stable `world` parent. No ancestor or Git
//! root is granted trust on the caller's behalf.

mod lease;
mod state_file;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::value::{RawValue, to_raw_value};
use unicode_normalization::UnicodeNormalization as _;

type Object = BTreeMap<String, Box<RawValue>>;

/// Whether this call added the directory's explicit trust record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustStatus {
    AlreadyTrusted,
    Written,
}

/// Ensures an explicit `projects[path].hasTrustDialogAccepted = true` record.
///
/// `directory` must exist. Its physical path, with Claude's Windows prefix and
/// separator normalization, is the only key written. Ancestor records are not
/// used to skip this check: the World needs its own stable record even when a
/// broader record happens to exist today. Git roots inside it can stop Claude's
/// inheritance; callers must keep their launch directories outside such roots.
///
/// This blocking operation resolves `CLAUDE_CONFIG_DIR` and the legacy
/// `.config.json` exactly as Claude does, takes its mkdir/mtime lease, rereads
/// the file, and preserves unrelated JSON values. A live lock returns
/// `WouldBlock`; invalid JSON or object shapes return `InvalidData`. No unlocked
/// fallback, repair from defaults, or in-place truncation is performed.
///
/// Claude also has writers that ignore this lease. This function cannot prevent
/// those writers from overwriting a successful update later. An error during
/// publication verification or lock release can occur after trust was written;
/// the error says so. Callers must handle launch-time trust rejection separately.
pub fn ensure_trusted(directory: &Path, config_dir: Option<&std::ffi::OsStr>) -> io::Result<TrustStatus> {
    let key = directory_key(directory)?;
    let paths = StatePaths::resolve(config_dir)?;
    let before = state_file::read(&paths.file)?;
    if update(before.as_deref(), &key)?.is_none() {
        return Ok(TrustStatus::AlreadyTrusted);
    }
    fs::create_dir_all(paths.file.parent().expect("absolute global state path"))?;
    let lease = lease::Lease::acquire(&paths.file)?;
    let result = (|| {
        let before = state_file::read(&paths.file)?;
        let Some(after) = update(before.as_deref(), &key)? else {
            return Ok(TrustStatus::AlreadyTrusted);
        };
        state_file::backup(&paths.file, &paths.backups, before.is_some())?;
        state_file::publish(&paths.file, before.as_deref(), &after, &lease)?;
        let verified =
            state_file::read(&paths.file).and_then(|bytes| update(bytes.as_deref(), &key)).map_err(|error| {
                io::Error::new(error.kind(), format!("Claude trust was published, but verification failed: {error}"))
            })?;
        if verified.is_some() {
            return Err(io::Error::other("Claude trust was published but its record is no longer present"));
        }
        Ok(TrustStatus::Written)
    })();
    let released = lease.release();
    match (result, released) {
        (Ok(status), Ok(())) => Ok(status),
        (Err(error), Ok(())) => Err(error),
        (Ok(status), Err(error)) => Err(io::Error::new(
            error.kind(),
            format!("Claude trust result was {status:?}, but releasing its lock failed: {error}"),
        )),
        (Err(error), Err(release)) => Err(io::Error::new(
            error.kind(),
            format!("{error}; releasing the Claude state lock also failed: {release}"),
        )),
    }
}

struct StatePaths {
    file: PathBuf,
    backups: PathBuf,
}

impl StatePaths {
    /// Claude Code's state for the home `configured` names, the value of
    /// `CLAUDE_CONFIG_DIR` the session starts with; `None` is unset.
    fn resolve(configured: Option<&std::ffi::OsStr>) -> io::Result<Self> {
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map_or_else(fastctx_platform::user_home, |value| Ok(PathBuf::from(value)))?;
        #[cfg(not(windows))]
        let home = fastctx_platform::user_home()?;
        let configured = configured.map(std::ffi::OsStr::to_os_string);
        let config = configured.clone().map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
        // Claude normalizes its config directory to NFC, but its modern global
        // filename uses the raw, nonempty override. An empty override differs
        // from an unset one for the legacy lookup and backup directory.
        let config = PathBuf::from(unicode_path(&config)?.nfc().collect::<String>());
        let legacy = config_file(&config, ".config.json")?;
        let file = if legacy.try_exists()? {
            legacy
        } else {
            config_file(
                &configured.filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or(home),
                ".claude.json",
            )?
        };
        Ok(Self { file, backups: config_file(&config, "backups")? })
    }
}

fn config_file(directory: &Path, name: &str) -> io::Result<PathBuf> {
    let directory = unicode_path(directory)?;
    // Node's join inserts a separator after a bare Windows drive ("C:") too.
    // Collapse dot segments before IO, including when a parent is a symlink.
    let joined =
        if directory.is_empty() { name.into() } else { format!("{directory}{}{name}", std::path::MAIN_SEPARATOR) };
    let mut normalized = PathBuf::new();
    for component in std::path::absolute(joined)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    Ok(normalized)
}

pub fn uninstall_key(directory: &Path) -> io::Result<String> {
    directory_key(directory)
}

pub(super) fn directory_key(directory: &Path) -> io::Result<String> {
    let physical = fs::canonicalize(directory)?;
    if !physical.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotADirectory, "Claude trust requires an existing directory"));
    }
    let path = unicode_path(&physical)?;
    #[cfg(windows)]
    let path = if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("//{unc}")
    } else if path.starts_with("\\\\?\\") && path.len() >= 7 && path.as_bytes()[5] == b':' {
        path[4..].to_owned()
    } else {
        path.to_owned()
    }
    .replace('\\', "/");
    Ok(path.to_owned())
}

fn unicode_path(path: &Path) -> io::Result<&str> {
    path.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Claude paths must be Unicode"))
}

/// Whether `name`, a key of Claude's projects, is the directory `key` or one
/// inside it. Keys are physical paths with forward slashes; Windows paths
/// compare without case, as the file system does.
fn within(name: &str, key: &str) -> bool {
    let same = |a: &str, b: &str| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b };
    same(name, key)
        || (name.len() > key.len()
            && name.as_bytes()[key.len()] == b'/'
            && name.is_char_boundary(key.len())
            && same(&name[..key.len()], key))
}

/// The keys of the project entries of the World's directory `key` and of
/// every directory inside it, as Claude's state holds them now.
pub fn world_entries(config_dir: Option<&std::ffi::OsStr>, key: &str) -> io::Result<Vec<String>> {
    let paths = StatePaths::resolve(config_dir)?;
    let Some(bytes) = state_file::read(&paths.file)? else { return Ok(Vec::new()) };
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Claude state is not UTF-8"))?;
    let mut root = object(text.strip_prefix('\u{feff}').unwrap_or(text), "root")?;
    let projects = member(&mut root, "projects", "projects")?;
    Ok(projects.into_keys().filter(|name| within(name, key)).collect())
}

/// Removes the project entries of the World's directory `key` and of every
/// directory inside it, where its sessions ran, preserving every other byte.
/// The World made those directories, so no entry for them predates it. The
/// ordinary Claude lease and compare-before-publication apply here too.
pub fn remove_world_trust(config_dir: Option<&std::ffi::OsStr>, key: &str) -> io::Result<bool> {
    use jsonc_parser::ast::Value;
    use jsonc_parser::common::Ranged;
    let paths = StatePaths::resolve(config_dir)?;
    let Some(before) = state_file::read(&paths.file)? else { return Ok(false) };
    // One entry at a time, each removal parsed afresh.
    let remove_one = |bytes: &[u8]| -> io::Result<Option<Vec<u8>>> {
        let text = std::str::from_utf8(bytes).map_err(|_| io::Error::other("Claude state is not UTF-8"))?;
        let offset = if text.starts_with('\u{feff}') { 3 } else { 0 };
        let source = &text[offset..];
        let parsed = jsonc_parser::parse_to_ast(source, &Default::default(), &Default::default())
            .map_err(|_| io::Error::other("Claude state is invalid JSON"))?;
        let Some(Value::Object(root)) = parsed.value else {
            return Err(io::Error::other("Claude state is not an object"));
        };
        let Some(projects) = root.properties.iter().find(|property| property.name.as_str() == "projects") else {
            return Ok(None);
        };
        let Value::Object(projects) = &projects.value else {
            return Err(io::Error::other("Claude projects is not an object"));
        };
        let Some((index, property)) =
            projects.properties.iter().enumerate().find(|(_, property)| within(property.name.as_str(), key))
        else {
            return Ok(None);
        };
        let range = property.range();
        let mut start = range.start;
        let mut end = range.end;
        if let Some(next) = projects.properties.get(index + 1) {
            let comma = source[end..next.range().start]
                .find(',')
                .ok_or_else(|| io::Error::other("Claude project separator is absent"))?;
            end += comma + 1;
        } else if index > 0 {
            let previous = projects.properties[index - 1].range().end;
            let comma = source[previous..start]
                .rfind(',')
                .ok_or_else(|| io::Error::other("Claude project separator is absent"))?;
            start = previous + comma;
        }
        let mut after = bytes.to_vec();
        after.drain(offset + start..offset + end);
        let valid = std::str::from_utf8(&after).map_err(|_| io::Error::other("Claude state is not UTF-8"))?;
        serde_json::from_str::<serde_json::Value>(valid.strip_prefix('\u{feff}').unwrap_or(valid))
            .map_err(|_| io::Error::other("removing Claude trust would produce invalid JSON"))?;
        Ok(Some(after))
    };
    let remove = |bytes: &[u8]| -> io::Result<Option<Vec<u8>>> {
        let mut removed: Option<Vec<u8>> = None;
        while let Some(after) = remove_one(removed.as_deref().unwrap_or(bytes))? {
            removed = Some(after);
        }
        Ok(removed)
    };
    if remove(&before)?.is_none() {
        return Ok(false);
    }
    let lease = lease::Lease::acquire(&paths.file)?;
    let result = (|| {
        let before = state_file::read(&paths.file)?
            .ok_or_else(|| io::Error::other("Claude state disappeared before trust removal"))?;
        let Some(after) = remove(&before)? else { return Ok(false) };
        state_file::publish(&paths.file, Some(&before), &after, &lease)?;
        if remove(
            &state_file::read(&paths.file)?
                .ok_or_else(|| io::Error::other("Claude state disappeared after trust removal"))?,
        )?
        .is_some()
        {
            return Err(io::Error::other("World trust is still present after publication"));
        }
        Ok(true)
    })();
    let released = lease.release();
    match (result, released) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(release)) => Err(io::Error::other(format!("{error}; releasing Claude state lock: {release}"))),
    }
}

fn object(text: &str, location: &str) -> io::Result<Object> {
    serde_json::from_str(text).map_err(|error| {
        // Do not include parser messages: they can contain source values.
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Claude state {location} must be a JSON object (line {}, column {})", error.line(), error.column()),
        )
    })
}

fn member(parent: &mut Object, name: &str, location: &str) -> io::Result<Object> {
    parent.remove(name).map_or_else(|| Ok(Object::new()), |value| object(value.get(), location))
}

fn update(bytes: Option<&[u8]>, key: &str) -> io::Result<Option<Vec<u8>>> {
    let text = std::str::from_utf8(bytes.unwrap_or(b"{}"))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Claude state is not UTF-8"))?;
    let mut root = object(text.strip_prefix('\u{feff}').unwrap_or(text), "root")?;
    let mut projects = member(&mut root, "projects", "projects")?;
    let mut project = member(&mut projects, key, "target project")?;
    if let Some(value) = project.get("hasTrustDialogAccepted") {
        match serde_json::from_str::<bool>(value.get()) {
            Ok(true) => return Ok(None),
            Ok(false) => {}
            Err(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "Claude trust flag must be a boolean")),
        }
    }
    project.insert("hasTrustDialogAccepted".into(), to_raw_value(&true)?);
    projects.insert(key.into(), to_raw_value(&project)?);
    root.insert("projects".into(), to_raw_value(&projects)?);
    Ok(Some(serde_json::to_vec_pretty(&root)?))
}
