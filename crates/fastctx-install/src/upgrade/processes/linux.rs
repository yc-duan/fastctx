use super::Process;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) fn enumerate() -> io::Result<(Vec<Process>, Vec<String>)> {
    let mut processes = Vec::new();
    let mut unavailable = Vec::new();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                unavailable.push(format!("PID {pid}: {error}"));
                continue;
            }
        };
        if metadata.uid() != uid || pid == std::process::id() {
            continue;
        }
        match fastctx_platform::identity_of(pid) {
            Ok(None) => continue,
            Ok(Some(_)) => {}
            Err(error) => {
                unavailable.push(format!("PID {pid}: {error}"));
                continue;
            }
        }
        let image_source = entry.path().join("exe");
        match fs::read_link(&image_source) {
            Ok(image) => processes.push(Process { pid, image, image_source, legacy: false }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => unavailable.push(format!("PID {pid}: {error}")),
        }
    }
    Ok((processes, unavailable))
}

pub(super) fn references(process: &Process, path: &Path) -> io::Result<bool> {
    let pid = process.pid;
    if let Ok(cwd) = fs::read_link(format!("/proc/{pid}/cwd"))
        && super::within(&cwd, path)
    {
        return Ok(true);
    }
    match fs::read(PathBuf::from(format!("/proc/{pid}/cmdline"))) {
        Ok(bytes) => Ok(super::command_references(&String::from_utf8_lossy(&bytes), path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
