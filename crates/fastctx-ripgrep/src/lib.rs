//! The official ripgrep image travels inside fastctx, so copying fastctx alone
//! preserves it through old upgraders, npm cache deletion and server installs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::Staged;

pub mod licenses;

pub const VERSION: &str = env!("FASTCTX_RIPGREP_VERSION");
pub const NAME: &str = if cfg!(windows) { "rg.exe" } else { "rg" };
const IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rg"));

/// Places this build's verified ripgrep beside the pinned fastctx before any
/// jobs start. `directory` must exist and belong to this version of the World.
/// Replacement is atomic; an unreadable existing image is an error.
pub fn place(directory: &Path) -> io::Result<PathBuf> {
    licenses::place(directory)?;
    let path = directory.join(NAME);
    let present = match fs::read(&path) {
        Ok(bytes) => bytes == IMAGE,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if !present {
        let mut staged = Staged::create(&path, Some(0o700))?;
        staged.write_all(IMAGE)?;
        staged.commit()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // Also completes placement after an interrupted older installation.
        if fs::metadata(&path)?.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(path)
}
