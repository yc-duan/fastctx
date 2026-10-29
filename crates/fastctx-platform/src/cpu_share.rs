//! The processors shared evenly among the accounts whose jobs run on a
//! machine (`world-network.md`, 共享机器与智能体协助): a busy machine gives
//! each account an even share, an idle one lets any use it all.
//!
//! On Linux, under a service systemd delegated its cgroup to and started in
//! a leaf of it (`Delegate=yes`, `DelegateSubgroup=node`, systemd 254 and
//! later), each account's jobs go in a cgroup of their own below the
//! service's, all of the same weight. The node, and what it starts for its
//! conversations, share the machine with the jobs as one more.
//!
//! On Windows each account's jobs go in a job object of their own, all of
//! the same weight under the processor's weight-based rate control; the job
//! object each job's processes are ended by nests inside its account's.
//!
//! Elsewhere nothing is done.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

/// The variable that tells a job's supervisor the share it joins: a cgroup
/// on Linux, a job object's name on Windows.
pub const VARIABLE: &str = "FASTCTX_JOB_CGROUP";

/// What the accounts' jobs are shared out in: the service's cgroup, or the
/// accounts' job objects.
pub struct Shares {
    #[cfg(target_os = "linux")]
    jobs: PathBuf,
    #[cfg(windows)]
    jobs: windows::Jobs,
    #[cfg(not(any(target_os = "linux", windows)))]
    never: std::convert::Infallible,
}

impl Shares {
    /// Takes up what the accounts' jobs of the world at `world` share the
    /// processors in. On Linux it is the cgroup systemd delegated to this
    /// process's service: what else runs at its top (jobs an earlier
    /// version of the service left running there) moves to a leaf of its
    /// own, and the processors are shared among what is below; none where
    /// this is no such service.
    pub fn take_up(world: &Path) -> io::Result<Option<Shares>> {
        #[cfg(target_os = "linux")]
        {
            let _ = world;
            linux::take_up()
        }
        #[cfg(windows)]
        {
            Ok(Some(Shares { jobs: windows::Jobs::new(world) }))
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = world;
            Ok(None)
        }
    }

    /// The share of the jobs of the account `key` names, made when missing,
    /// as the job's supervisor is told it (`VARIABLE`); `key` is safe as a
    /// file name.
    pub fn of(&self, key: &str) -> io::Result<OsString> {
        #[cfg(target_os = "linux")]
        {
            let dir = self.jobs.join(key);
            match std::fs::create_dir(&dir) {
                Ok(()) => Ok(dir.into_os_string()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(dir.into_os_string()),
                Err(error) => Err(error),
            }
        }
        #[cfg(windows)]
        {
            self.jobs.of(key)
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = key;
            match self.never {}
        }
    }
}

/// Moves this process into the share `share` names, as a job's supervisor
/// does before its job starts; what it starts then is in it too.
pub fn join(share: &OsStr) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::join(share)
    }
    #[cfg(not(windows))]
    {
        std::fs::write(Path::new(share).join("cgroup.procs"), std::process::id().to_string())
    }
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

#[cfg(windows)]
mod windows {
    use std::collections::HashMap;
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::mem::zeroed;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use std::path::Path;
    use std::ptr;
    use std::sync::{Mutex, PoisonError};

    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::READ_CONTROL;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
        JOB_OBJECT_CPU_RATE_CONTROL_WEIGHT_BASED, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
        JobObjectCpuRateControlInformation, OpenJobObjectW,
    };
    use windows_sys::Win32::System::SystemServices::JOB_OBJECT_ASSIGN_PROCESS;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    use crate::windows_job::set;
    use crate::windows_user::{current_owner_sid, object_owner_sid};

    /// Every account's job object has this weight, the one Windows gives
    /// a job by default: what matters is that all are the same.
    const WEIGHT: u32 = 5;

    /// The job objects of the accounts' jobs of one world, by account, held
    /// while the node runs.
    pub(super) struct Jobs {
        /// The names begin with it: one world's are not another's, so a
        /// development world beside the released one shares nothing with it.
        prefix: String,
        held: Mutex<HashMap<String, OwnedHandle>>,
    }

    impl Jobs {
        pub(super) fn new(world: &Path) -> Self {
            // FNV-1a: the same world gives the same names in every release.
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for unit in world.as_os_str().encode_wide() {
                for byte in unit.to_le_bytes() {
                    hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            Self { prefix: format!("Local\\fastctx-cpu-{hash:016x}-"), held: Mutex::default() }
        }

        pub(super) fn of(&self, key: &str) -> io::Result<OsString> {
            let name = format!("{}{key}", self.prefix);
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            if !held.contains_key(key) {
                held.insert(key.to_string(), account_job(&name)?);
            }
            Ok(name.into())
        }
    }

    /// The account's job object named `name`, made when missing, of the
    /// common weight. One by that name made by someone else is refused.
    fn account_job(name: &str) -> io::Result<OwnedHandle> {
        let wide: Vec<u16> = OsStr::new(name).encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated name and default security.
        let raw = unsafe { CreateJobObjectW(ptr::null(), wide.as_ptr()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: read right after the call that set it.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        // SAFETY: a new handle, owned from here.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        if existed {
            ours(&job)?;
        }
        // SAFETY: zeroed is a valid empty rate control structure.
        let mut rate: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { zeroed() };
        rate.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_WEIGHT_BASED;
        rate.Anonymous.Weight = WEIGHT;
        set(&job, JobObjectCpuRateControlInformation, &rate)?;
        Ok(job)
    }

    /// Refuses a job object this process's user did not make.
    fn ours(job: &OwnedHandle) -> io::Result<()> {
        if object_owner_sid(job.as_raw_handle())? != current_owner_sid()? {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the account's job object is not this user's"));
        }
        Ok(())
    }

    pub(super) fn join(name: &OsStr) -> io::Result<()> {
        let wide: Vec<u16> = name.encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated name.
        let raw = unsafe { OpenJobObjectW(JOB_OBJECT_ASSIGN_PROCESS | READ_CONTROL, 0, wide.as_ptr()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new handle, owned from here.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        ours(&job)?;
        // SAFETY: both handles are live.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // Held while this process lives: the name lasts as long as anything
        // holds it, so a node started again meanwhile finds the same job.
        std::mem::forget(job);
        Ok(())
    }
}
