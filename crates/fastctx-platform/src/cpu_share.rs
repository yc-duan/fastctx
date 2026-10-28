//! The processors shared evenly among the accounts whose jobs run on a
//! machine (`world-network.md`, 共享机器与智能体协助). On Linux, under a
//! service systemd delegated its cgroup to and started in a leaf of it
//! (`Delegate=yes`, `DelegateSubgroup=node`, systemd 254 and later), each
//! account's jobs go in a cgroup of their own below the service's, all of
//! the same weight: a busy machine gives each account an even share, an
//! idle one lets any use it all. The node, and what it starts for its
//! conversations, share the machine with the jobs as one more. Elsewhere
//! nothing is done.

use std::io;
use std::path::{Path, PathBuf};

/// The variable that tells a job's supervisor the cgroup it joins.
pub const VARIABLE: &str = "FASTCTX_JOB_CGROUP";

/// The service's cgroup, taken up for the jobs.
pub struct Shares {
    jobs: PathBuf,
}

impl Shares {
    /// Takes up the cgroup systemd delegated to this process's service:
    /// what else runs at its top (jobs an earlier version of the service
    /// left running there) moves to a leaf of its own, and the processors
    /// are shared among what is below. None where this is no such service.
    pub fn take_up() -> io::Result<Option<Shares>> {
        #[cfg(target_os = "linux")]
        {
            linux::take_up()
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(None)
        }
    }

    /// The cgroup of the jobs of the account `key` names, made when missing;
    /// `key` is safe as a file name.
    pub fn of(&self, key: &str) -> io::Result<PathBuf> {
        let dir = self.jobs.join(key);
        match std::fs::create_dir(&dir) {
            Ok(()) => Ok(dir),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(dir),
            Err(error) => Err(error),
        }
    }
}

/// Moves this process into the cgroup `dir`, as a job's supervisor does
/// before its job starts; what it starts then is in it too.
pub fn join(dir: &Path) -> io::Result<()> {
    std::fs::write(dir.join("cgroup.procs"), std::process::id().to_string())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::Shares;

    const MOUNT: &str = "/sys/fs/cgroup";

    pub(super) fn take_up() -> io::Result<Option<Shares>> {
        let Some(own) = own_cgroup()? else { return Ok(None) };
        let leaf = Path::new(MOUNT).join(own.trim_start_matches('/'));
        // Only a fastctx service's own cgroup, the node in the leaf systemd
        // started it in, with the processor controller to hand down.
        let Some(root) = leaf.parent().filter(|_| leaf.file_name().is_some_and(|name| name == "node")) else {
            return Ok(None);
        };
        let service = root.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if !(service.starts_with("fastctx") && service.ends_with(".service")) {
            return Ok(None);
        }
        let controllers = std::fs::read_to_string(root.join("cgroup.controllers")).unwrap_or_default();
        if !controllers.split_whitespace().any(|controller| controller == "cpu")
            || std::fs::OpenOptions::new().write(true).open(root.join("cgroup.subtree_control")).is_err()
        {
            return Ok(None);
        }
        let jobs = made(&root.join("jobs"))?;
        let earlier = made(&jobs.join("earlier"))?;
        // What an earlier version left at the top is moved out; something
        // that came meanwhile is moved on the next pass.
        for _ in 0..5 {
            for pid in std::fs::read_to_string(root.join("cgroup.procs"))?.lines() {
                // One that ended meanwhile is gone already.
                let _ = std::fs::write(earlier.join("cgroup.procs"), pid);
            }
            if std::fs::write(root.join("cgroup.subtree_control"), "+cpu").is_ok() {
                std::fs::write(jobs.join("cgroup.subtree_control"), "+cpu")?;
                return Ok(Some(Shares { jobs }));
            }
        }
        Err(io::Error::other(format!("{} still holds processes", root.display())))
    }

    /// This process's cgroup in the unified hierarchy, where there is one.
    fn own_cgroup() -> io::Result<Option<String>> {
        let text = match std::fs::read_to_string("/proc/self/cgroup") {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(text.lines().find_map(|line| line.strip_prefix("0::")).map(str::to_string))
    }

    fn made(dir: &Path) -> io::Result<PathBuf> {
        match std::fs::create_dir(dir) {
            Ok(()) => Ok(dir.to_path_buf()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(dir.to_path_buf()),
            Err(error) => Err(error),
        }
    }
}
