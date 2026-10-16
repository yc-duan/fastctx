//! macOS's privacy protection on what a program reads and writes: the
//! Desktop, Documents and Downloads folders, iCloud Drive, and removable and
//! network volumes open to a program only once its user allowed it, and are
//! refused with EPERM until then. Fastctx's node, which the agents' commands
//! and file tools run under, is such a program (`world-gui.md`, 设置). Where
//! it is refused is said as that, with where to allow it, and the last place
//! refused is kept for the interface to say so.

use std::sync::Mutex;
use std::time::SystemTime;

/// The last place refused while this process runs, and when.
static REFUSED: Mutex<Option<(String, SystemTime)>> = Mutex::new(None);

/// Where macOS refuses even a program allowed everything: the system's own,
/// kept by System Integrity Protection.
const SYSTEM: [&str; 4] = ["/System/", "/bin/", "/sbin/", "/usr/"];

/// Whether `error` reaching `path` is macOS's privacy protection refusing
/// this program; noted as the last place refused when it is.
pub fn refused(path: &str, error: &std::io::Error) -> bool {
    let system = SYSTEM.iter().any(|prefix| path.starts_with(prefix) && !path.starts_with("/usr/local/"));
    let refused = cfg!(target_os = "macos") && error.raw_os_error() == Some(1) && !system;
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

/// The last place refused while this process runs, and when.
pub fn last() -> Option<(String, SystemTime)> {
    REFUSED.lock().expect("never poisoned").clone()
}
