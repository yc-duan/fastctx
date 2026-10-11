//! Interpret paths owned by this machine, after routing has selected it.

use std::io;
use std::path::{Path, PathBuf};

/// Accepts native paths and, on Windows, MSYS drive paths such as `/c/work`.
/// MSYS mount points require the caller's mount table, so callers must convert
/// those with `cygpath -m`. Remote paths must reach their owner unchanged.
/// This does not make relative paths absolute or resolve filesystem links.
pub fn local_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let units: Vec<u16> = path.as_os_str().encode_wide().collect();
        if units.first() == Some(&u16::from(b'/')) && units.get(1) != Some(&u16::from(b'/')) {
            if let Some(&drive) = units.get(1)
                && ((b'a' as u16..=b'z' as u16).contains(&drive) || (b'A' as u16..=b'Z' as u16).contains(&drive))
                && (units.len() == 2 || units.get(2) == Some(&u16::from(b'/')))
            {
                let mut native = vec![u16::from((drive as u8).to_ascii_uppercase()), u16::from(b':'), u16::from(b'/')];
                native.extend_from_slice(&units[units.len().min(3)..]);
                return Ok(PathBuf::from(OsString::from_wide(&native)));
            }
            // `/A:/work` is a drive path with a slash before it, not a
            // mount: `cygpath` would put it under Git's own folder.
            if let Some(&drive) = units.get(1)
                && ((b'a' as u16..=b'z' as u16).contains(&drive) || (b'A' as u16..=b'Z' as u16).contains(&drive))
                && units.get(2) == Some(&u16::from(b':'))
            {
                let native = OsString::from_wide(&units[1..]);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "{} has a slash before its drive; write it {}",
                        path.display(),
                        Path::new(&native).display()
                    ),
                ));
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} is an MSYS mount path; use cygpath -m to convert it to a native Windows path",
                    path.display()
                ),
            ));
        }
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_and_msys_path_contract() {
        for path in ["relative/file", "C:/work/file", r"C:\work\file", "//server/share/file", r"\\?\C:\work\file"] {
            assert_eq!(local_path(path).unwrap(), PathBuf::from(path));
        }
        for (path, native) in [("/c/work/file", "C:/work/file"), ("/Z/x", "Z:/x"), ("/d", "D:/")] {
            assert_eq!(local_path(path).unwrap(), PathBuf::from(if cfg!(windows) { native } else { path }));
        }
        for path in ["/tmp/file", "/usr/bin", "/home/user", "/", "/1/file"] {
            if cfg!(windows) {
                assert!(local_path(path).unwrap_err().to_string().contains("cygpath -m"));
            } else {
                assert_eq!(local_path(path).unwrap(), PathBuf::from(path));
            }
        }
        if cfg!(windows) {
            assert!(local_path("/A:/work/file").unwrap_err().to_string().ends_with("write it A:/work/file"));
        }
    }
}
