//! Paths handed straight to the Windows API. A long one goes in its verbatim
//! form, as the standard library hands its own: the plain form stops at 260
//! characters for a program that does not declare itself long-path aware.

use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

/// Below this length the plain form works everywhere, directory creation
/// included, which keeps twelve characters back for a short name.
const PLAIN: usize = 248;

/// `path` as a NUL-terminated wide string the Windows API accepts at any length.
pub fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let plain: Vec<u16> = path.as_os_str().encode_wide().collect();
    if plain.len() < PLAIN || starts(&plain, r"\\?\") || starts(&plain, r"\\.\") {
        return Ok(terminated(plain));
    }
    // Verbatim paths skip normalization, so separators and dots are resolved first.
    let absolute: Vec<u16> = std::path::absolute(path)?.as_os_str().encode_wide().collect();
    if starts(&absolute, r"\\?\") || starts(&absolute, r"\\.\") {
        return Ok(terminated(absolute));
    }
    let verbatim = match absolute.strip_prefix(&units(r"\\")[..]) {
        Some(share) => units(r"\\?\UNC\").into_iter().chain(share.iter().copied()).collect(),
        None => units(r"\\?\").into_iter().chain(absolute).collect(),
    };
    Ok(terminated(verbatim))
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn starts(path: &[u16], prefix: &str) -> bool {
    path.starts_with(&units(prefix))
}

fn terminated(mut path: Vec<u16>) -> Vec<u16> {
    path.push(0);
    path
}
