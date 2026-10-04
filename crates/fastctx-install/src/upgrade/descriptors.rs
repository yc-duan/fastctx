//! Reserve descriptors for the observed locks without consuming node headroom.

use super::Error;

pub(super) fn with_capacity<T>(entries: usize, work: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    #[cfg(target_os = "linux")]
    {
        let mut budget = Budget::reserve(entries).map_err(super::io)?;
        let result = work();
        match budget.restore() {
            Ok(()) => result,
            Err(error) => Err(Error::Io(format!(
                "could not restore cleanup descriptor allowance: {error}{}",
                result.err().map(|error| format!("; cleanup: {error}")).unwrap_or_default()
            ))),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = entries;
        work()
    }
}

#[cfg(target_os = "linux")]
struct Budget {
    original: libc::rlimit,
    changed: bool,
}

#[cfg(target_os = "linux")]
impl Budget {
    fn reserve(entries: usize) -> std::io::Result<Self> {
        let mut original = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: the output points to an initialized rlimit.
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut original) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut budget = Self { original, changed: false };
        if entries == 0 || budget.original.rlim_cur == libc::RLIM_INFINITY {
            return Ok(budget);
        }
        let opened = std::fs::read_dir("/proc/self/fd")?.collect::<std::io::Result<Vec<_>>>()?.len();
        let needed =
            opened.checked_add(entries).ok_or_else(|| std::io::Error::other("cleanup descriptor count overflow"))?;
        if libc::rlim_t::try_from(needed).map_err(std::io::Error::other)? < budget.original.rlim_cur {
            return Ok(budget);
        }
        // One handle per observed entry bounds the retained runtime locks.
        // Preserve the node's pre-existing allowance, within the OS hard limit.
        let required = budget
            .original
            .rlim_cur
            .checked_add(entries.try_into().map_err(std::io::Error::other)?)
            .ok_or_else(|| std::io::Error::other("cleanup descriptor allowance overflow"))?;
        if libc::rlim_t::try_from(needed).map_err(std::io::Error::other)? >= budget.original.rlim_max {
            return Err(std::io::Error::other(format!(
                "cleanup needs {entries} additional file descriptors beside {opened} open descriptors; hard limit {} is insufficient; cleanup deferred",
                budget.original.rlim_max
            )));
        }
        let limit =
            libc::rlimit { rlim_cur: required.min(budget.original.rlim_max), rlim_max: budget.original.rlim_max };
        // SAFETY: only this process's soft limit changes, within its hard limit.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        budget.changed = true;
        Ok(budget)
    }

    fn restore(&mut self) -> std::io::Result<()> {
        if self.changed {
            // SAFETY: restore the exact limits captured before this cleanup.
            if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &self.original) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            self.changed = false;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for Budget {
    fn drop(&mut self) {
        // Normal completion reports restoration errors; this also covers unwind.
        let _ = self.restore();
    }
}
