//! A node-owned diagnostic log, independent of a launcher's standard handles.

use crate::world::WorldPaths;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

pub(super) fn open(paths: &WorldPaths) -> Result<(), String> {
    paths.ensure()?;
    let path = paths.dir.join("node.log");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|error| {
        format!(
            "Cannot open the node log {}: {error}",
            crate::paths::display_path(&path)
        )
    })?;
    FILE.set(Mutex::new(file))
        .map_err(|_| "The node diagnostic log was already opened.".to_string())
}

pub(super) fn write(line: &str) {
    if let Some(file) = FILE.get() {
        let mut file = file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(error) = writeln!(file, "{line}") {
            eprintln!("Cannot append the node diagnostic log: {error}");
        }
    }
}
