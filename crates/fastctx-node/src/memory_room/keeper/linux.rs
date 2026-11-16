//! The node's service cgroup, whose processes — the agents' and their
//! hosts' — are ranked for the kernel's OOM killer above the node itself.

use fastctx_platform::disk;
use std::io;
use std::path::{Path, PathBuf};

pub(super) struct Keeper {
    group: PathBuf,
    /// The node could lower its own score: the others are ranked above it.
    standing: bool,
}

impl Keeper {
    pub(super) fn new() -> io::Result<Self> {
        let group = service_group()?;
        let standing = disk::write("/proc/self/oom_score_adj", "-500").is_ok();
        Ok(Self { group, standing })
    }

    pub(super) fn description(&self) -> String {
        format!(
            "{}; {}",
            self.group.display(),
            if self.standing {
                "node OOM priority -500; service descendants 300"
            } else {
                "OOM priority unchanged: the account cannot lower its own score"
            }
        )
    }

    pub(super) fn standing(&self) -> bool {
        self.standing
    }

    pub(super) fn rank(&self) -> io::Result<()> {
        rank_work(&self.group)
    }
}

fn service_group() -> io::Result<PathBuf> {
    let text = disk::kernel_text("/proc/self/cgroup")?;
    let own = text
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| io::Error::other("no unified cgroup hierarchy"))?;
    let own = Path::new(own);
    let service = if own.file_name().is_some_and(|name| name == "node") { own.parent() } else { Some(own) }
        .ok_or_else(|| io::Error::other("no service cgroup"))?;
    let name = service.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    if !name.starts_with("fastctx") || !name.ends_with(".service") {
        return Err(io::Error::other("the node is not in its own fastctx service"));
    }
    Ok(Path::new("/sys/fs/cgroup").join(service.strip_prefix("/").map_err(io::Error::other)?))
}

fn rank_work(group: &Path) -> io::Result<()> {
    let mut groups = vec![group.to_path_buf()];
    let me = std::process::id();
    while let Some(at) = groups.pop() {
        let text = match disk::kernel_text(at.join("cgroup.procs")) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for pid in text.lines().filter_map(|pid| pid.parse::<u32>().ok()).filter(|pid| *pid != me) {
            let path = PathBuf::from(format!("/proc/{pid}/oom_score_adj"));
            if disk::kernel_text(&path)
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
                .is_some_and(|score| score < 300)
            {
                // One gone meanwhile, or one this account may not rank (another
                // user's, or one that is not dumpable), is passed over: it
                // holds back neither the rest nor the reclaiming after them.
                match disk::write(path, "300") {
                    Ok(()) => {}
                    Err(error)
                        if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied)
                            || matches!(error.raw_os_error(), Some(1 | 3)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let entries = match disk::read_dir(&at) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                groups.push(entry.path());
            }
        }
    }
    Ok(())
}
