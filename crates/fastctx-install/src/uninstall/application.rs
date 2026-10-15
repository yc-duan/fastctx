//! Application data uses the same platform directories as Tauri. A terminal
//! removal also retires data left by a former graphical installation.

use std::path::{Path, PathBuf};
use std::{fs, io};

/// Captures this user's Tauri data and development notification directories
/// before elevation. Windows uses Known Folders; Linux honors absolute XDG paths.
pub fn application_directories(home: &Path) -> io::Result<Vec<PathBuf>> {
    #[cfg(windows)]
    let roots = {
        use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, FOLDERID_RoamingAppData};
        let _ = home;
        vec![known_folder(&FOLDERID_RoamingAppData)?, known_folder(&FOLDERID_LocalAppData)?]
    };
    #[cfg(target_os = "linux")]
    let roots = [("XDG_DATA_HOME", ".local/share"), ("XDG_CONFIG_HOME", ".config"), ("XDG_CACHE_HOME", ".cache")]
        .into_iter()
        .map(|(variable, default)| {
            std::env::var_os(variable)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join(default))
        })
        .collect::<Vec<_>>();
    // macOS keeps an application's data, caches and its web view's storage
    // under ~/Library, each by the application's identifier.
    #[cfg(target_os = "macos")]
    let roots: Vec<PathBuf> = ["Library/Application Support", "Library/Caches", "Library/WebKit"]
        .into_iter()
        .map(|place| home.join(place))
        .collect();
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let roots: Vec<PathBuf> = {
        let _ = home;
        Vec::new()
    };
    #[cfg(windows)]
    let development = roots[1].join("dev.fastctx.world.development");
    let mut paths = roots.into_iter().map(|root| root.join("dev.fastctx.world")).collect::<Vec<_>>();
    #[cfg(windows)]
    paths.push(development);
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub(super) fn is_application_directory(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "dev.fastctx.world" || name == "dev.fastctx.world.development")
}

#[cfg(windows)]
fn known_folder(folder: &windows_sys::core::GUID) -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::SHGetKnownFolderPath};
    let mut pointer = std::ptr::null_mut();
    // The system allocates a NUL-terminated path owned by the COM allocator.
    let result = unsafe { SHGetKnownFolderPath(folder, 0, std::ptr::null_mut(), &mut pointer) };
    if result < 0 {
        return Err(io::Error::other(format!("resolving application data directory: HRESULT {result:#x}")));
    }
    let mut length = 0;
    unsafe {
        while *pointer.add(length) != 0 {
            length += 1;
        }
    }
    let path = unsafe { std::ffi::OsString::from_wide(std::slice::from_raw_parts(pointer, length)) };
    unsafe {
        CoTaskMemFree(pointer.cast());
    }
    Ok(path.into())
}

/// Removes application-owned entries and reports arbitrary additions. Each
/// entry is tried independently; links never grant ownership of their targets.
pub fn remove_application_data(directory: &Path) -> super::Programs {
    let mut result = super::Programs { left: Vec::new(), failed: Vec::new() };
    if let Err(error) = retire(directory, &mut result) {
        result.failed.push((directory.into(), error));
    }
    result
}

fn retire(directory: &Path, result: &mut super::Programs) -> io::Result<()> {
    super::checked_path(directory)?;
    if !is_application_directory(directory) {
        return Err(io::Error::other("application data must name the World application directory"));
    }
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(metadata) if crate::disk::is_link(&metadata) => {
            result.left.push(directory.into());
            return Ok(());
        }
        Ok(_) => {}
    }
    let development = directory.file_name().is_some_and(|name| name == "dev.fastctx.world.development");
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if crate::disk::is_link(&fs::symlink_metadata(entry.path())?) {
            result.left.push(entry.path());
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let owned = if development {
            name == "notifications"
        } else {
            matches!(
                name.as_ref(),
                "connection.json" | ".window-state.json" | "device" | "notifications" | "page-sandbox-webview"
            ) || (cfg!(windows) && name == "EBWebView")
                || (cfg!(target_os = "linux")
                    && matches!(
                        name.as_ref(),
                        "cookies"
                            | "localstorage"
                            | "databases"
                            | "itp"
                            | "serviceworkers"
                            | "applications"
                            | "CacheStorage"
                            | "WebKitCache"
                            | "storage"
                            | "hsts-storage.sqlite"
                            | "MediaCache"
                            | "AlternativeServices"
                            | "isolatedsites"
                            | "mediakeys"
                            | "deviceidhashsalts"
                            | "mediakeyshashsalts"
                            | "resourcemonitorthrottler"
                    ))
        };
        if owned {
            if let Err(error) = super::delete_tree(&entry.path()) {
                result.failed.push((entry.path(), error));
            }
        } else {
            result.left.push(entry.path());
        }
    }
    if fs::read_dir(directory)?.next().is_none() {
        fs::remove_dir(directory)?;
    }
    Ok(())
}
