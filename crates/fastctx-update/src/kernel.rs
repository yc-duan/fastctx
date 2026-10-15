//! The command-line program of a release: the package `fastctx-<triple>`
//! for the machine it runs on, a `.tar.gz` with the program and its notices
//! side by side.

use std::path::{Path, PathBuf};

use fastctx_types::update::KERNEL_PREFIX;

/// The package this machine's World program comes in.
pub fn target() -> String {
    format!("{KERNEL_PREFIX}{}", triple())
}

/// The Rust target triple this program was built for, as releases name
/// their command-line programs: Linux's static programs keep the `gnu`
/// name they were published under.
pub fn triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => "unknown",
    }
}

/// Takes the program out of the checked `archive` into `into`, a directory
/// of the caller's, and says where it is.
pub fn unpack(archive: &Path, into: &Path) -> Result<PathBuf, String> {
    let name = if cfg!(windows) { "fastctx.exe" } else { "fastctx" };
    let file = std::fs::File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in entries.entries().map_err(|error| format!("reading {}: {error}", archive.display()))? {
        let mut entry = entry.map_err(|error| format!("reading {}: {error}", archive.display()))?;
        let path = entry.path().map_err(|error| error.to_string())?.into_owned();
        if path.file_name().is_some_and(|file| file == name) && path.components().count() == 1 {
            let out = into.join(name);
            entry.unpack(&out).map_err(|error| format!("unpacking {}: {error}", out.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755))
                    .map_err(|error| error.to_string())?;
            }
            return Ok(out);
        }
    }
    Err(format!("{} has no {name}", archive.display()))
}
