//! Earlier guardians, running as SYSTEM, kept their program digests under
//! SYSTEM's own profile (`renewal::digest`), in a fastctx directory nothing
//! else knows of. It goes with the last service.

use std::path::PathBuf;

use anyhow::{Context as _, bail};

/// SYSTEM's profile, as Windows records it for the account S-1-5-18.
fn system_profile() -> anyhow::Result<PathBuf> {
    let recorded = windows_registry::LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\S-1-5-18")
        .and_then(|key| key.get_string("ProfileImagePath"))
        .context("reading SYSTEM's profile directory")?;
    let wide: Vec<u16> = recorded.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the first call reports the expanded length including its NUL;
    // the second writes at most that many units into a buffer of that size.
    unsafe {
        use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
        let length = ExpandEnvironmentStringsW(wide.as_ptr(), std::ptr::null_mut(), 0);
        if length == 0 {
            bail!("expanding SYSTEM's profile directory: {}", std::io::Error::last_os_error());
        }
        let mut expanded = vec![0u16; length as usize];
        let written = ExpandEnvironmentStringsW(wide.as_ptr(), expanded.as_mut_ptr(), length);
        if written == 0 || written > length {
            bail!("expanding SYSTEM's profile directory: {}", std::io::Error::last_os_error());
        }
        expanded.truncate(written as usize - 1);
        Ok(PathBuf::from(String::from_utf16(&expanded).context("SYSTEM's profile directory is not Unicode")?))
    }
}

/// Removes the digests a guardian kept under SYSTEM's profile, and the
/// directories that held only them. Returns what is there that fastctx did
/// not write; it stays, and so do the directories holding it.
pub(in crate::service) fn retire() -> anyhow::Result<Vec<PathBuf>> {
    let root = system_profile()?.join(".fastctx");
    let digests = root.join("cache").join("digests");
    let mut left = Vec::new();
    let entries = match std::fs::read_dir(&digests) {
        Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", digests.display())),
    };
    for entry in entries {
        let name = entry.file_name();
        let digest =
            name.to_str().is_some_and(|name| name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()));
        if digest && entry.file_type()?.is_file() {
            crate::uninstall::delete_file(&entry.path())?;
        } else {
            left.push(entry.path());
        }
    }
    for directory in [digests, root.join("cache"), root] {
        match std::fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                if left.is_empty() {
                    left.push(directory);
                }
                break;
            }
            Err(error) => return Err(error).with_context(|| format!("removing {}", directory.display())),
        }
    }
    Ok(left)
}
