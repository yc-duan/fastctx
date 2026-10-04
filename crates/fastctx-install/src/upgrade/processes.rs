//! Read-only process observations. Command lines never enter reports.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use super::archive;

#[cfg(windows)]
#[path = "processes/windows.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "processes/linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "processes/macos.rs"]
mod imp;

pub(super) struct Process {
    pub pid: u32,
    pub image: PathBuf,
    pub image_source: PathBuf,
    pub legacy: bool,
    #[cfg(windows)]
    pub handle: Option<std::os::windows::io::OwnedHandle>,
}

pub(super) struct Inventory {
    pub processes: Vec<Process>,
    pub unavailable: Vec<String>,
}

impl Inventory {
    pub fn read() -> io::Result<Self> {
        let (processes, mut unavailable) = imp::enumerate()?;
        let mut cache = HashMap::new();
        let mut result = Vec::new();
        for mut process in processes {
            if process.pid == std::process::id() {
                continue;
            }
            let classified = if let Some(legacy) = cache.get(&process.image_source) {
                Ok(*legacy)
            } else {
                classify(&process.image_source)
            };
            #[cfg(windows)]
            let classified =
                classified.and_then(|legacy| if legacy { imp::confirmed_legacy(&process) } else { Ok(false) });
            match classified {
                Ok(legacy) => {
                    cache.insert(process.image_source.clone(), legacy);
                    process.legacy = legacy;
                    result.push(process);
                }
                Err(error) => match exited(&process) {
                    Ok(true) => {}
                    _ => unavailable.push(format!(
                        "PID {} ({}): cannot inspect executable: {error}",
                        process.pid,
                        process.image.display()
                    )),
                },
            }
        }
        let image = std::env::current_exe()?;
        result.push(Process {
            pid: std::process::id(),
            image_source: image.clone(),
            image,
            legacy: false,
            #[cfg(windows)]
            handle: None,
        });
        Ok(Self { processes: result, unavailable })
    }

    pub fn legacy_blockers(&self) -> Vec<String> {
        let mut result = self.unavailable.clone();
        for process in &self.processes {
            if process.pid == std::process::id() {
                continue;
            }
            // Both generations use the stable bin path. Only a released image
            // proves legacy identity; names and locations cannot distinguish them.
            if process.legacy {
                result.push(format!(
                    "legacy process PID {} is still running from {}",
                    process.pid,
                    process.image.display()
                ));
            }
        }
        result
    }

    pub fn users_of(&self, source: &Path, command_lines: bool) -> Vec<String> {
        let mut result = self.unavailable.clone();
        for process in &self.processes {
            if within(&process.image, source) {
                result.push(format!("PID {} is running from {}", process.pid, process.image.display()));
            } else if command_lines {
                match imp::references(process, source) {
                    Ok(true) => result.push(format!(
                        "PID {} ({}) still references this path",
                        process.pid,
                        process.image.display()
                    )),
                    Ok(false) => {}
                    Err(error) => result.push(format!("PID {}: cannot inspect path use: {error}", process.pid)),
                }
            }
        }
        result
    }
}

pub(super) fn within(path: &Path, root: &Path) -> bool {
    if cfg!(windows) {
        let normalize =
            |path: &Path| path.to_string_lossy().replace('/', "\\").trim_start_matches("\\\\?\\").to_lowercase();
        let (path, root) = (normalize(path), normalize(root));
        path == root || path.strip_prefix(&root).is_some_and(|rest| rest.starts_with('\\'))
    } else {
        path.starts_with(root)
    }
}

pub(super) fn command_references(command: &str, path: &Path) -> bool {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        command.replace('\\', "/").to_lowercase().contains(&path.replace('\\', "/").to_lowercase())
    } else {
        command.contains(path.as_ref())
    }
}

pub(super) fn known_binary(hash: &str) -> bool {
    include_str!("released-sha256.txt").lines().any(|line| line.split_whitespace().next() == Some(hash))
}

pub(super) fn classify(path: &Path) -> io::Result<bool> {
    let metadata = std::fs::metadata(path)?;
    let size_matches = include_str!("released-sha256.txt")
        .lines()
        .any(|line| line.split_whitespace().nth(1).and_then(|value| value.parse::<u64>().ok()) == Some(metadata.len()));
    if !size_matches {
        return Ok(false);
    }
    #[cfg(target_os = "linux")]
    if path.starts_with("/proc") {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        // /proc/PID/exe opens the actual image, even after unlink or replacement.
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        return Ok(known_binary(&format!("{:x}", hasher.finalize())));
    }
    Ok(known_binary(&archive::hash_file(path)?))
}

pub(super) fn dead(pid: u32) -> io::Result<bool> {
    Ok(legacy_token(pid)?.is_none())
}

fn exited(process: &Process) -> io::Result<bool> {
    #[cfg(windows)]
    {
        imp::exited(process)
    }
    #[cfg(not(windows))]
    {
        dead(process.pid)
    }
}

pub(super) fn legacy_token(pid: u32) -> io::Result<Option<String>> {
    #[cfg(target_os = "macos")]
    {
        imp::token(pid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(fastctx_platform::identity_of(pid)?.map(|identity| {
            // Released Linux jobs used start ticks without boot_id. A tick
            // collision after reboot is retained rather than mistaken for death.
            if cfg!(target_os = "linux") {
                identity.token.rsplit(':').next().expect("nonempty token").to_string()
            } else {
                identity.token
            }
        }))
    }
}

pub(super) fn nonce_pid(name: &str, prefixes: &[&str]) -> Option<u32> {
    let suffix = prefixes.iter().find_map(|prefix| name.strip_prefix(prefix))?;
    let suffix = suffix.strip_suffix(".exe").or_else(|| suffix.strip_suffix(".json")).unwrap_or(suffix);
    let fields: Vec<_> = suffix.split('-').collect();
    if fields.len() != 3
        || fields.iter().any(|field| field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    fields[1].parse::<u128>().ok()?;
    fields[2].parse::<u64>().ok()?;
    fields[0].parse::<u32>().ok().filter(|pid| *pid != 0)
}
