//! Whether a file on this computer runs as a program when opened. A link in
//! a conversation opens what it names with the system's own handler, and
//! for a program that is running it: the page asks first, showing the file
//! in its folder as the plain way (issue list 17: links to programs ran at a
//! click).

use std::path::Path;

/// Kinds Windows runs, or hands to something that runs them, when opened.
#[cfg(windows)]
const RUNS: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "ws",
    "hta",
    "msi",
    "msp",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "scr",
    "pif",
    "lnk",
    "reg",
    "cpl",
    "jar",
    "application",
    "appref-ms",
    "msc",
    "gadget",
    "sct",
    "url",
    "inf",
    "settingcontent-ms",
    // Scripts an installed interpreter runs when opened: Python's own
    // launcher, Git's bash and the like register these to run, not to show.
    "py",
    "pyw",
    "pyc",
    "pyo",
    "pyz",
    "pyzw",
    "sh",
    "bash",
    "rb",
    "rbw",
    "pl",
    "scf",
    "jnlp",
    "msu",
    "diagcab",
    "chm",
];

/// Kinds macOS runs, or installs, when opened.
#[cfg(target_os = "macos")]
const RUNS: &[&str] =
    &["app", "command", "tool", "pkg", "mpkg", "sh", "terminal", "workflow", "scpt", "applescript", "jar"];

/// Kinds a Linux desktop may run when opened.
#[cfg(all(unix, not(target_os = "macos")))]
const RUNS: &[&str] = &["sh", "run", "bin", "appimage", "desktop", "jar"];

/// Whether opening `path` would run it.
pub fn runs(path: &Path) -> bool {
    let named = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| RUNS.iter().any(|kind| kind.eq_ignore_ascii_case(extension)));
    named || marked_executable(path)
}

/// A Unix file anyone may execute, whatever its name.
#[cfg(unix)]
fn marked_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn marked_executable(_path: &Path) -> bool {
    false
}
