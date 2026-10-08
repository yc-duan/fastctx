//! Restores the World's tool directory after the machine's login profile.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, publish};

/// Writes a job's bash startup file. Bash reads BASH_ENV after login profiles,
/// without changing the user script's line numbers. Child shells inherit PATH
/// but must not inherit this startup file.
pub fn write(script: &Path, directory: &Path) -> io::Result<PathBuf> {
    let path = script.with_extension("bashenv");
    let mut text = b"unset BASH_ENV\nPATH=".to_vec();
    if cfg!(windows) {
        // Native drive colons are PATH separators in bash; use this Git
        // installation's converter even when a profile replaced PATH.
        text.extend_from_slice(b"$(/usr/bin/cygpath -u -- ");
    }
    text.push(b'\'');
    for byte in path_bytes(directory)? {
        if byte == b'\'' {
            text.extend_from_slice(b"'\\''");
        } else {
            text.push(byte);
        }
    }
    text.push(b'\'');
    if cfg!(windows) {
        text.push(b')');
    }
    text.extend_from_slice(b":\"$PATH\" || exit\nexport PATH\n");
    publish(&path, &text, Permissions::Private)?;
    Ok(path)
}

fn path_bytes(path: &Path) -> io::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        Ok(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(windows)]
    {
        let path = path.to_str().ok_or_else(|| io::Error::other("the World's tool directory is not Unicode"))?;
        Ok(path.replace('\\', "/").into_bytes())
    }
}

/// The startup path as Git Bash or Unix bash reads it.
pub fn environment_value(path: &Path) -> io::Result<OsString> {
    #[cfg(windows)]
    {
        let path = path.to_str().ok_or_else(|| io::Error::other("the bash startup path is not Unicode"))?;
        Ok(path.replace('\\', "/").into())
    }
    #[cfg(unix)]
    Ok(path.as_os_str().to_owned())
}
