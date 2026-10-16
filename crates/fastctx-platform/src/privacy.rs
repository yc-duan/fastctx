//! macOS's privacy protection on what a program reads and writes: the
//! Desktop, Documents and Downloads folders, iCloud Drive, other apps' data,
//! and removable and network volumes open to a program only once its user
//! allowed it, and are refused with EPERM until then. Fastctx's node, which
//! the agents' commands and file tools run under, is such a program
//! (`world-gui.md`, 设置). Where it is refused is said as that, with where to
//! allow it, and the last place refused is kept for the interface to say so
//! while it is refused still.

use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

/// The last place refused while this process runs, and when.
static REFUSED: Mutex<Option<(String, SystemTime)>> = Mutex::new(None);

/// The places in a user's home macOS keeps private from a program until its
/// user allows it: the folders it asks about, iCloud Drive, and the data of
/// Mail, Messages, Safari, Calendar, Contacts, Photos and other apps.
const PRIVATE: [&str; 13] = [
    "Desktop",
    "Documents",
    "Downloads",
    "Library/Mobile Documents",
    "Library/Mail",
    "Library/Messages",
    "Library/Safari",
    "Library/Calendars",
    "Library/Containers",
    "Library/Group Containers",
    "Library/Application Support/AddressBook",
    "Library/Application Support/com.apple.TCC",
    "Pictures/Photos Library.photoslibrary",
];

/// Whether `path` is in a place macOS keeps private: one of `PRIVATE` in this
/// user's home, or a removable or network volume.
fn private(path: &str) -> bool {
    if path.starts_with("/Volumes/") {
        return true;
    }
    let Ok(home) = crate::user_home() else { return false };
    let path = Path::new(path);
    PRIVATE.iter().any(|place| path.starts_with(home.join(place)))
}

/// Whether `error` reaching `path` is macOS's privacy protection refusing
/// this program: EPERM, where macOS keeps things private. Elsewhere EPERM
/// means something else, a file made unchangeable among them.
fn refusal(path: &str, error: &std::io::Error) -> bool {
    cfg!(target_os = "macos") && error.raw_os_error() == Some(1) && private(path)
}

/// Whether `error` reaching `path` is macOS's privacy protection refusing
/// this program; noted as the last place refused when it is.
pub fn refused(path: &str, error: &std::io::Error) -> bool {
    let refused = refusal(path, error);
    if refused {
        *REFUSED.lock().expect("never poisoned") = Some((path.to_string(), SystemTime::now()));
    }
    refused
}

/// Why a place is refused, as the model and the user read it.
pub const REFUSAL: &str = "macOS keeps it private and has not let Fastctx in; on that Mac, allow Fastctx in System Settings → Privacy & Security → Full Disk Access, then try again";

/// A place refused, with why.
pub fn said(path: &str) -> String {
    format!("{path}: {REFUSAL}")
}

/// The private places of this user's home macOS refuses this program now,
/// as they are written: looked at again when something it ran was refused.
pub fn refused_now() -> Vec<String> {
    let Ok(home) = crate::user_home() else { return Vec::new() };
    ["Desktop", "Documents", "Downloads"]
        .into_iter()
        .map(|name| home.join(name))
        .filter_map(|place| {
            let written = place.to_string_lossy().into_owned();
            match std::fs::read_dir(&place) {
                Err(error) if refused(&written, &error) => Some(written),
                _ => None,
            }
        })
        .collect()
}

/// The last place refused while this process runs, and when, while it is
/// refused still: it is looked at again, so a place let in since is said no
/// more.
pub fn last() -> Option<(String, SystemTime)> {
    let (path, at) = REFUSED.lock().expect("never poisoned").clone()?;
    let looked = match std::fs::read_dir(&path) {
        // A file is opened, as a folder is listed.
        Err(error) if error.raw_os_error() == Some(20) => std::fs::File::open(&path).map(drop),
        listed => listed.map(drop),
    };
    if let Err(error) = &looked
        && refusal(&path, error)
    {
        return Some((path, at));
    }
    let mut kept = REFUSED.lock().expect("never poisoned");
    if kept.as_ref().is_some_and(|(kept, _)| *kept == path) {
        *kept = None;
    }
    None
}
