//! How far this machine is toward the limits its system sets on what one
//! user's programs hold, the ones the World has run out of and might again:
//! inotify instances and watches, open files, tasks. Each is what the user
//! the node runs as holds, against the limit that bounds it; a limit the
//! system does not set, or does not hold the user to, is left out. Read when
//! asked.

/// One limit and how much of it is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Use {
    pub kind: Kind,
    pub used: u64,
    pub limit: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// inotify instances the user holds, against `fs.inotify.max_user_instances`.
    InotifyInstances,
    /// inotify watches the user holds, against `fs.inotify.max_user_watches`.
    InotifyWatches,
    /// Files the node itself holds open, against its own limit.
    OpenFiles,
    /// Tasks (threads) the user runs, against its limit.
    Tasks,
}

pub fn read() -> Vec<Use> {
    imp::read()
}

/// The open files a node or a hub raises its own limit to, at most.
#[cfg(unix)]
const OPEN_FILES: u64 = 65_536;

/// Raises this process's soft limit on open files toward its hard limit,
/// to at most `OPEN_FILES`: a node or a hub holds a descriptor for every
/// connection, watch and file it works with, and the soft limit a service
/// starts with (1024 under systemd, 256 under launchd) is reached long
/// before the machine runs short (`world-concurrency.md`: bounds follow the
/// machine's resources). What it starts inherits it, which stays well under
/// what a program that walks every descriptor finds slow.
pub fn raise_open_files() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: getrlimit writes one initialized structure.
        let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let ceiling = OPEN_FILES as libc::rlim_t;
        let wanted = if limit.rlim_max == libc::RLIM_INFINITY { ceiling } else { limit.rlim_max.min(ceiling) };
        if limit.rlim_cur == libc::RLIM_INFINITY || limit.rlim_cur >= wanted {
            return Ok(());
        }
        // macOS refuses more than OPEN_MAX (10240) whatever the hard limit.
        for target in [wanted, wanted.min(10_240)] {
            let raised = libc::rlimit { rlim_cur: target, rlim_max: limit.rlim_max };
            // SAFETY: setrlimit reads one initialized structure.
            if target > limit.rlim_cur && unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } == 0 {
                return Ok(());
            }
        }
        Err(std::io::Error::last_os_error())
    }
    #[cfg(not(unix))]
    Ok(())
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Kind, Use};

    pub fn read() -> Vec<Use> {
        let mut uses = Vec::new();
        // SAFETY: reads the calling process's real user id.
        let uid = unsafe { libc::getuid() };
        let held = held(uid);
        if let Some(limit) = setting("/proc/sys/fs/inotify/max_user_instances") {
            uses.push(Use { kind: Kind::InotifyInstances, used: held.instances, limit });
        }
        if let Some(limit) = setting("/proc/sys/fs/inotify/max_user_watches") {
            uses.push(Use { kind: Kind::InotifyWatches, used: held.watches, limit });
        }
        if let (Ok(open), Some(limit)) = (std::fs::read_dir("/proc/self/fd"), soft(libc::RLIMIT_NOFILE)) {
            uses.push(Use { kind: Kind::OpenFiles, used: open.count() as u64, limit });
        }
        // The superuser is not held to its task limit.
        if uid != 0
            && let Some(limit) = soft(libc::RLIMIT_NPROC)
        {
            uses.push(Use { kind: Kind::Tasks, used: held.tasks, limit });
        }
        uses
    }

    #[derive(Default)]
    struct Held {
        instances: u64,
        watches: u64,
        tasks: u64,
    }

    /// What the processes of `uid` hold. A process that ended meanwhile, or
    /// whose files may not be read, counts for nothing.
    fn held(uid: u32) -> Held {
        let mut held = Held::default();
        let Ok(processes) = std::fs::read_dir("/proc") else { return held };
        for process in processes.filter_map(Result::ok) {
            let name = process.file_name();
            let Some(pid) = name.to_str().filter(|name| name.bytes().all(|byte| byte.is_ascii_digit())) else {
                continue;
            };
            let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else { continue };
            let field = |name: &str| {
                status.lines().find_map(|line| line.strip_prefix(name)).and_then(|rest| rest.split_whitespace().next())
            };
            if field("Uid:").and_then(|real| real.parse::<u32>().ok()) != Some(uid) {
                continue;
            }
            held.tasks += field("Threads:").and_then(|count| count.parse::<u64>().ok()).unwrap_or(1);
            let Ok(files) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
            for file in files.filter_map(Result::ok) {
                if std::fs::read_link(file.path()).is_ok_and(|target| target.as_os_str() == "anon_inode:inotify") {
                    held.instances += 1;
                    let info =
                        std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{}", file.file_name().to_string_lossy()));
                    held.watches += info
                        .map_or(0, |info| info.lines().filter(|line| line.starts_with("inotify wd:")).count() as u64);
                }
            }
        }
        held
    }

    fn setting(path: &str) -> Option<u64> {
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    /// What `getrlimit` takes for a resource: glibc names its own type.
    #[cfg(target_env = "gnu")]
    type Resource = libc::__rlimit_resource_t;
    #[cfg(not(target_env = "gnu"))]
    type Resource = libc::c_int;

    /// The soft limit on `resource`, where there is one.
    fn soft(resource: Resource) -> Option<u64> {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: the structure is the one getrlimit fills.
        if unsafe { libc::getrlimit(resource, &mut limit) } != 0 || limit.rlim_cur == libc::RLIM_INFINITY {
            return None;
        }
        Some(limit.rlim_cur)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Kind, Use};

    pub fn read() -> Vec<Use> {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: the structure is the one getrlimit fills.
        let limited =
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 && limit.rlim_cur != libc::RLIM_INFINITY;
        match std::fs::read_dir("/dev/fd") {
            Ok(open) if limited => {
                vec![Use { kind: Kind::OpenFiles, used: open.count() as u64, limit: limit.rlim_cur }]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    /// Windows bounds none of these for one user's programs.
    pub fn read() -> Vec<super::Use> {
        Vec::new()
    }
}
