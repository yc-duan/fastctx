//! Where fastctx keeps its things for this user on this machine.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// Whether this program was built by the release scripts, which set
/// `FASTCTX_RELEASE` (`scripts/kernel.mjs`, `gui/scripts/release.mjs`,
/// `gui/scripts/android.mjs`). Any other build is a development build,
/// whose World is kept apart from the one the user's installed Fastctx
/// runs: in `.fastctx-dev`, off the PATH, looking for no new versions.
pub const RELEASED: bool = option_env!("FASTCTX_RELEASE").is_some();

/// Whether the features not offered in a release yet run here: sentinels,
/// project memory and goals. Their code is kept and a development build
/// runs them; a release offers none of them to the user or the model, and
/// runs them only where `FASTCTX_PREVIEW` is set (`world.md`, 这一版不发的).
pub fn preview() -> bool {
    !RELEASED || std::env::var_os("FASTCTX_PREVIEW").is_some()
}

/// Whether this program takes part in its networks' versions, looking for
/// new releases and telling its hubs what it runs: a release does, and a
/// development build only in a drill whose `FASTCTX_UPDATE_RELEASES` names
/// a list of releases of its own.
pub fn follows_releases() -> bool {
    RELEASED || std::env::var_os("FASTCTX_UPDATE_RELEASES").is_some()
}

/// The directory holding everything fastctx keeps for this user:
/// `FASTCTX_HOME` when it is set, which lets a test or a second World run
/// beside the real one, and `.fastctx` in the user's home otherwise, or
/// `.fastctx-dev` for a development build (`RELEASED`).
pub fn fastctx_root() -> io::Result<PathBuf> {
    match std::env::var_os("FASTCTX_HOME") {
        Some(root) => absolute("FASTCTX_HOME", root),
        None => Ok(user_home()?.join(if RELEASED { ".fastctx" } else { ".fastctx-dev" })),
    }
}

/// Where fastctx keeps everything for the user whose home is `home`, as
/// `fastctx_root` finds it for that user when `FASTCTX_HOME` is unset: for
/// an account another one installs a World for.
pub fn fastctx_root_in(home: &Path) -> PathBuf {
    home.join(if RELEASED { ".fastctx" } else { ".fastctx-dev" })
}

/// The user's home directory. On Unix it is `HOME`, as every program there
/// takes it, and the account database when `HOME` is unset. On Windows it
/// is the profile directory as the system records it, never a variable
/// that a shell may have respelled, except for SYSTEM, whose home is the
/// one Fastctx gives it (`crate::system_home`).
pub fn user_home() -> io::Result<PathBuf> {
    imp::user_home()
}

/// The temporary directory the commands this process starts are given:
/// for SYSTEM the one in the home Fastctx gives it (`crate::system_home`),
/// the system's for anyone else. `std::env::temp_dir` cannot say it for
/// SYSTEM: it asks Windows' `GetTempPath2`, which answers SYSTEM with
/// `C:\Windows\SystemTemp` whatever `TEMP` says.
pub fn temp_dir() -> io::Result<PathBuf> {
    Ok(crate::system_home::temporary()?.unwrap_or_else(std::env::temp_dir))
}

/// The numeric id of the user this process runs as; none on Windows, which
/// has no such id.
pub fn user_id() -> Option<u32> {
    #[cfg(unix)]
    {
        // SAFETY: getuid cannot fail.
        Some(unsafe { libc::getuid() })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn absolute(what: &str, path: OsString) -> io::Result<PathBuf> {
    let path = crate::local_path(PathBuf::from(path))?;
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{what} must be an absolute path, not {}", path.display()),
        ))
    }
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::path::PathBuf;

    pub fn user_home() -> io::Result<PathBuf> {
        if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
            return super::absolute("HOME", home);
        }
        // SAFETY: getuid has no preconditions.
        let account = crate::unix_account::lookup(unsafe { libc::getuid() })?
            .ok_or_else(|| io::Error::other("HOME is unset and this user has no entry in the account database"))?;
        super::absolute("the account's home", account.home)
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::OsStringExt as _;
    use std::path::PathBuf;

    use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;

    use crate::windows_user::with_current_token;

    pub fn user_home() -> io::Result<PathBuf> {
        if let Some(home) = crate::system_home::home()? {
            return Ok(home);
        }
        let profile = with_current_token(|token| {
            let mut length = 0u32;
            // SAFETY: a size query with no buffer.
            let sized = unsafe { GetUserProfileDirectoryW(token, std::ptr::null_mut(), &mut length) };
            let error = io::Error::last_os_error();
            if sized != 0 || error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
                return Err(error);
            }
            let mut buffer = vec![0u16; length as usize];
            // SAFETY: the buffer holds `length` units, as the system asked.
            if unsafe { GetUserProfileDirectoryW(token, buffer.as_mut_ptr(), &mut length) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
            Ok(OsString::from_wide(&buffer[..end]))
        })?;
        super::absolute("the profile directory", profile)
    }
}
