//! How Claude Code names what it keeps for a working directory (2.1.296):
//! the sessions under its home's `projects`, and its caches and task output
//! outside it, each in a folder named by the directory's path.

use std::io;
use std::path::Path;

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
