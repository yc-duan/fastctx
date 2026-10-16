//! The folders in a folder of this machine, for the user choosing one on it
//! from any device (`world-gui.md`, 新对话): names and paths only, never
//! what is in a file.

use std::path::Path;

use fastctx_types::MachineName;
use fastctx_types::focus::{Folder, Folders};
use fastctx_types::ipc::Failure;

use crate::run::present::display_path;

/// The folders in `path`, or with none the places a choice starts from: the
/// user's home, and each local disk (`/` on Unix).
pub fn list(machine: MachineName, path: Option<&str>) -> Result<Folders, Failure> {
    let home =
        fastctx_platform::user_home().map_err(|error| Failure::not_run(format!("finding the home folder: {error}")))?;
    let home = written(&home);
    let Some(path) = path else {
        let mut folders = vec![Folder { name: home.clone(), path: home.clone(), hidden: false }];
        for root in roots().map_err(Failure::not_run)? {
            if !folders.iter().any(|folder| same(&folder.path, &root)) {
                folders.push(Folder { name: root.clone(), path: root, hidden: false });
            }
        }
        return Ok(Folders { machine, path: None, parent: None, home, folders });
    };
    let folder = Path::new(path);
    if !folder.is_absolute() {
        return Err(Failure::not_run(format!("{path} is not an absolute path")));
    }
    // A folder macOS keeps private is said as that, with where to allow it.
    let unread = |error: std::io::Error| {
        if fastctx_platform::privacy::refused(path, &error) {
            Failure::mac_privacy(fastctx_platform::privacy::said(path))
        } else {
            Failure::not_run(format!("{path}: {error}"))
        }
    };
    let entries = std::fs::read_dir(folder).map_err(unread)?;
    let mut folders = Vec::new();
    for entry in entries {
        let entry = entry.map_err(unread)?;
        // A link to a folder is followed, as a file manager does.
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let hidden = name.starts_with('.') || hidden(&entry);
        folders.push(Folder { path: written(&entry.path()), name, hidden });
    }
    folders.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name)));
    let parent = folder.parent().map(written);
    Ok(Folders { machine, path: Some(written(folder)), parent, home, folders })
}

/// A path as this machine's folders are written to the model and the user:
/// absolute, with `/` on Windows too.
fn written(path: &Path) -> String {
    display_path(&path.to_string_lossy())
}

fn same(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.trim_end_matches('/').eq_ignore_ascii_case(b.trim_end_matches('/')) } else { a == b }
}

#[cfg(windows)]
fn roots() -> Result<Vec<String>, String> {
    let disks = fastctx_platform::disks::fixed_disks().map_err(|error| format!("listing the disks: {error}"))?;
    Ok(disks.iter().map(|disk| written(disk)).collect())
}

#[cfg(not(windows))]
fn roots() -> Result<Vec<String>, String> {
    Ok(vec!["/".to_string()])
}

#[cfg(windows)]
fn hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    /// `FILE_ATTRIBUTE_HIDDEN`.
    const HIDDEN: u32 = 0x2;
    entry.metadata().is_ok_and(|metadata| metadata.file_attributes() & HIDDEN != 0)
}

#[cfg(not(windows))]
fn hidden(_: &std::fs::DirEntry) -> bool {
    false
}
