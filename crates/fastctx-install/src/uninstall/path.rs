use std::io;
use std::path::Path;

/// The lines that put ~/.local/bin on a macOS login shell's PATH, exactly as
/// written into ~/.zprofile and taken out again.
pub const ZPROFILE_LINES: &str =
    "# Added by Fastctx: ~/.local/bin on the PATH.\nexport PATH=\"$HOME/.local/bin:$PATH\"\n";

/// Removes exactly matching directory entries, preserving every other entry
/// and its spelling. Separator ownership belongs to the removed entry.
pub fn path_entry(value: &str, directory: &str) -> String {
    let normal = |text: &str| text.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let wanted = normal(directory);
    value.split(';').filter(|entry| normal(&expand(entry.trim())) != wanted).collect::<Vec<_>>().join(";")
}

#[cfg(windows)]
fn expand(entry: &str) -> String {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    let source: Vec<u16> = std::ffi::OsStr::new(entry).encode_wide().chain([0]).collect();
    let size = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0) };
    if size == 0 {
        return entry.to_owned();
    }
    let mut target = vec![0u16; size as usize];
    let written = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), target.as_mut_ptr(), size) };
    if written == 0 || written > size {
        return entry.to_owned();
    }
    std::ffi::OsString::from_wide(&target[..written as usize - 1]).to_string_lossy().into_owned()
}

#[cfg(not(windows))]
fn expand(entry: &str) -> String {
    entry.to_owned()
}

/// Withdraws this user's World bin from the persistent Windows PATH.
#[cfg(windows)]
pub fn remove_path(_home: &Path, directory: &Path) -> io::Result<bool> {
    use windows_registry::{CURRENT_USER, Type};
    let key = match CURRENT_USER.open("Environment") {
        Ok(key) => key,
        Err(error) if error.code().0 == 0x80070002u32 as i32 => return Ok(false),
        Err(error) => return Err(io::Error::other(error.to_string())),
    };
    let kind = match key.get_type("Path") {
        Ok(kind @ (Type::String | Type::ExpandString)) => kind,
        Ok(_) => return Err(io::Error::other("the user's Path value is not a string")),
        Err(error) if error.code().0 == 0x80070002u32 as i32 => return Ok(false),
        Err(error) => return Err(io::Error::other(error.to_string())),
    };
    let before = key.get_string("Path").map_err(|error| io::Error::other(error.to_string()))?;
    let directory = directory.to_str().ok_or_else(|| io::Error::other("World bin is not Unicode"))?;
    let after = path_entry(&before, directory);
    if after == before {
        return Ok(false);
    }
    let key = CURRENT_USER.create("Environment").map_err(|error| io::Error::other(error.to_string()))?;
    if kind == Type::String { key.set_string("Path", &after) } else { key.set_expand_string("Path", &after) }
        .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(true)
}

#[cfg(unix)]
pub fn remove_path(home: &Path, directory: &Path) -> io::Result<bool> {
    #[cfg(target_os = "macos")]
    off_login_path(home)?;
    let link = home.join(".local/bin/fastctx");
    match std::fs::read_link(&link) {
        Ok(target) if target == directory.join("fastctx") => {
            super::delete_file(&link)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::InvalidInput) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Takes out of ~/.zprofile exactly the lines installing put there.
#[cfg(target_os = "macos")]
fn off_login_path(home: &Path) -> io::Result<()> {
    let profile = home.join(".zprofile");
    let text = match std::fs::read_to_string(&profile) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if text.contains(ZPROFILE_LINES) {
        std::fs::write(&profile, text.replacen(ZPROFILE_LINES, "", 1))?;
    }
    Ok(())
}
