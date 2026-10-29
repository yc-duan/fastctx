//! How Claude Code names what it keeps for a working directory (2.1.296):
//! the sessions under its home's `projects`, and its caches and task output
//! outside it, each in a folder named by the directory's path.

use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};

/// How much of a record's start is read to learn whose it is.
const HEAD: u64 = 256 * 1024;

/// Where Claude Code under `home` keeps the record of session `session`, as
/// it names it: `<session>.jsonl` in the folder of the directory the session
/// runs in, or failing that in any folder of `projects`. A file is the
/// session's only when its records say so.
pub fn record(home: &Path, cwd: Option<&Path>, session: &str) -> io::Result<Option<PathBuf>> {
    let file = format!("{session}.jsonl");
    let projects = home.join("projects");
    let named = cwd.and_then(|cwd| cwd_text(cwd).ok()).map(|cwd| projects.join(slug(&cwd)).join(&file));
    if let Some(path) = named.filter(|path| path.is_file())
        && says_session(&path, session)?
    {
        return Ok(Some(path));
    }
    let entries = match std::fs::read_dir(&projects) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path().join(&file);
        if path.is_file() && says_session(&path, session)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Whether the first of the record's rows that names a session names
/// `session`.
fn says_session(path: &Path, session: &str) -> io::Result<bool> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for line in io::BufReader::new(file.take(HEAD)).split(b'\n') {
        // A row cut short by the end of what is read is not one.
        let Ok(row) = serde_json::from_slice::<serde_json::Value>(&line?) else { continue };
        if let Some(named) = row["sessionId"].as_str() {
            return Ok(named == session);
        }
    }
    Ok(false)
}

/// The working directory's path as Claude Code holds it.
pub fn cwd_text(directory: &Path) -> io::Result<String> {
    // Claude resolves originalCwd with realpathSync. Rust's Windows
    // canonicalize adds an extended-length prefix to that physical path.
    let directory = match std::fs::canonicalize(directory) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => directory.to_path_buf(),
        Err(error) => return Err(error),
    };
    let text = directory
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Claude working directory is not Unicode"))?;
    let text = if cfg!(windows) {
        text.strip_prefix(r"\\?\UNC\")
            .map(|path| format!(r"\\{path}"))
            .unwrap_or_else(|| text.strip_prefix(r"\\?\").unwrap_or(text).replace('/', "\\"))
    } else {
        text.to_owned()
    };
    #[cfg(target_os = "macos")]
    let text = {
        use unicode_normalization::UnicodeNormalization as _;
        text.nfc().collect()
    };
    Ok(text)
}

/// Claude's ASCII replacement, 200 UTF-16 unit prefix and signed 32-bit hash.
pub fn slug(text: &str) -> String {
    let mut key = String::new();
    let mut hash = 0i32;
    let mut length = 0;
    for unit in text.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
        if length < 200 {
            key.push(if unit <= 127 && (unit as u8).is_ascii_alphanumeric() { unit as u8 as char } else { '-' });
        }
        length += 1;
    }
    if length > 200 {
        key.push('-');
        let mut number = hash.unsigned_abs();
        let mut suffix = Vec::new();
        loop {
            let digit = (number % 36) as u8;
            suffix.push(if digit < 10 { b'0' + digit } else { b'a' + digit - 10 });
            number /= 36;
            if number == 0 {
                break;
            }
        }
        key.extend(suffix.into_iter().rev().map(char::from));
    }
    key
}
