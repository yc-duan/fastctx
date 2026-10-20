//! Discover the portable command as Codex 0.160.0 does, without starting a server.

#[cfg(windows)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn available(home: &Path) -> Result<bool, String> {
    Ok(resolve(home)?.is_some())
}

pub(super) fn resolve(home: &Path) -> Result<Option<PathBuf>, String> {
    let path = std::env::var_os("PATH");
    #[cfg(windows)]
    {
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        let resolved = which::which_in(super::shared::COMMAND, path.as_ref(), &cwd)
            .unwrap_or_else(|_| PathBuf::from(super::shared::COMMAND));
        if resolved.parent().is_some_and(|parent| !parent.as_os_str().is_empty()) {
            return Ok(explicit_windows(&resolved));
        }
        let program =
            fastctx_hosts::codex::launch::discover(home, path.as_ref())?.map(|launch| launch.binary).filter(|binary| {
                // A shell wrapper's directory is not the native Codex image's
                // directory; PATH still searches that directory when explicit.
                !binary.extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
                })
            });
        Ok(native_windows(&resolved, path.as_deref(), program.as_deref()))
    }
    #[cfg(unix)]
    {
        let _ = home;
        let path = match path {
            Some(path) => path,
            None => default_unix_path()?,
        };
        Ok(std::env::split_paths(&path)
            .map(|directory| directory.join(super::shared::COMMAND))
            .find(|candidate| executable_unix(candidate)))
    }
}

#[cfg(windows)]
fn native_windows(name: &Path, path: Option<&OsStr>, program: Option<&Path>) -> Option<PathBuf> {
    use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW};

    // Rust resolves a bare name with the child's PATH first, then the
    // application's directory, system directories and the inherited PATH.
    // This is Codex's executable directory, never the running fastctx's.
    let paths = || path.into_iter().flat_map(std::env::split_paths).filter(|path| !path.as_os_str().is_empty());
    let directories = paths()
        .chain(program.and_then(Path::parent).map(Path::to_path_buf))
        .chain(system_directory(GetSystemDirectoryW))
        .chain(system_directory(GetWindowsDirectoryW))
        .chain(paths());
    for directory in directories {
        // which can return a bare relative .CMD for an empty PATH entry.
        // Rust still searches that name; it does not implicitly use cwd.
        let mut candidate = directory.join(name);
        if !name.as_os_str().as_encoded_bytes().contains(&b'.') {
            candidate.set_extension("exe");
        }
        // An earlier existing candidate shadows later ones in native lookup.
        if std::fs::symlink_metadata(&candidate).is_ok() {
            return candidate.is_file().then_some(candidate);
        }
    }
    None
}

#[cfg(windows)]
fn explicit_windows(path: &Path) -> Option<PathBuf> {
    if !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("exe")) {
        let mut appended = path.as_os_str().to_owned();
        appended.push(".exe");
        let appended = PathBuf::from(appended);
        if std::fs::symlink_metadata(&appended).is_ok() {
            return appended.is_file().then_some(appended);
        }
    }
    path.is_file().then(|| path.to_path_buf())
}

#[cfg(windows)]
fn system_directory(get: unsafe extern "system" fn(*mut u16, u32) -> u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    // A zero-length call returns the exact buffer size including its NUL.
    let size = unsafe { get(std::ptr::null_mut(), 0) };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: the buffer has the size returned by this same system API.
    let length = unsafe { get(buffer.as_mut_ptr(), size) };
    if length == 0 || length >= size {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buffer[..length as usize])))
}

#[cfg(target_os = "macos")]
fn default_unix_path() -> Result<OsString, String> {
    // Apple's execvp uses _PATH_DEFPATH; _CS_PATH additionally includes sbin.
    Ok(OsString::from("/usr/bin:/bin"))
}

#[cfg(target_os = "linux")]
fn default_unix_path() -> Result<OsString, String> {
    // Official Codex 0.160.0 Linux packages use musl's execvp default;
    // confstr(_CS_PATH) omits /usr/local/bin, even when fastctx uses musl.
    Ok(OsString::from("/usr/local/bin:/bin:/usr/bin"))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn default_unix_path() -> Result<OsString, String> {
    use std::os::unix::ffi::OsStringExt;

    // execvp uses the platform's default search path only when PATH is absent.
    let size = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
    if size == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut bytes = vec![0u8; size];
    // SAFETY: the buffer has the size returned by confstr for this value.
    let length = unsafe { libc::confstr(libc::_CS_PATH, bytes.as_mut_ptr().cast(), bytes.len()) };
    if length != size {
        return Err("the default executable search path changed while reading it".into());
    }
    bytes.pop();
    Ok(OsString::from_vec(bytes))
}

#[cfg(unix)]
fn executable_unix(path: &Path) -> bool {
    fastctx_platform::unix_executable::is_executable(path)
}
