//! Cross-process admission barrier for job starts, running limits, and Unapply.

use crate::control::paths::ControlPaths;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Exclusive admission guard. It does not serialize spool readers or writers.
pub(crate) struct AdmissionGuard {
    file: File,
    generation_path: PathBuf,
    generation: u64,
}

impl AdmissionGuard {
    pub(crate) fn acquire(paths: &ControlPaths) -> Result<Self, String> {
        let directory = crate::edit::private_storage::job_control_directory();
        crate::edit::private_storage::ensure_private_directory(
            &directory,
            "background job control",
        )?;
        let home_key = hex::encode(Sha256::digest(home_identity(&paths.home)));
        let lock_path = directory.join(format!("{home_key}.lock"));
        let generation_path = directory.join(format!("{home_key}.generation"));
        let file = crate::edit::private_storage::open_lock_file(
            &lock_path,
            "background job admission lock",
        )?;
        fs2::FileExt::lock_exclusive(&file).map_err(|error| {
            format!(
                "Cannot lock background job admission at {}: {error}. Retry after the other FastCtx process finishes.",
                crate::paths::display_path(&lock_path)
            )
        })?;
        let generation = read_generation(&generation_path)?;
        Ok(Self {
            file,
            generation_path,
            generation,
        })
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Invalidates every already-running server before Unapply starts terminating jobs.
    pub(crate) fn advance_generation(&mut self) -> Result<u64, String> {
        let next = self.generation.checked_add(1).ok_or_else(|| {
            "Cannot advance the background job admission generation because it overflowed."
                .to_string()
        })?;
        crate::control::transaction::atomic_replace(
            &self.generation_path,
            format!("{next}\n").as_bytes(),
            Some(0o600),
            false,
        )
        .map_err(|error| {
            format!(
                "Cannot publish the background job admission generation {}: {error}",
                crate::paths::display_path(&self.generation_path)
            )
        })?;
        self.generation = next;
        Ok(next)
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

pub(crate) fn observe_generation(paths: &ControlPaths) -> Result<u64, String> {
    Ok(AdmissionGuard::acquire(paths)?.generation())
}

fn read_generation(path: &Path) -> Result<u64, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(format!(
                "Cannot read the background job admission generation {}: {error}",
                crate::paths::display_path(path)
            ));
        }
    };
    source.trim().parse::<u64>().map_err(|error| {
        format!(
            "Cannot parse the background job admission generation {}: {error}. Remove this damaged runtime file and restart FastCtx.",
            crate::paths::display_path(path)
        )
    })
}

#[cfg(unix)]
fn home_identity(home: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    home.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn home_identity(home: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    home.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn home_identity(home: &Path) -> Vec<u8> {
    home.to_string_lossy().as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::{AdmissionGuard, observe_generation};
    use crate::control::paths::ControlPaths;

    #[test]
    fn generation_is_durable_and_scoped_to_the_control_home() {
        let temp = tempfile::tempdir().unwrap();
        let first = ControlPaths::for_home(temp.path().join("first"));
        let second = ControlPaths::for_home(temp.path().join("second"));

        assert_eq!(observe_generation(&first).unwrap(), 0);
        assert_eq!(observe_generation(&second).unwrap(), 0);
        let mut guard = AdmissionGuard::acquire(&first).unwrap();
        assert_eq!(guard.advance_generation().unwrap(), 1);
        drop(guard);

        assert_eq!(observe_generation(&first).unwrap(), 1);
        assert_eq!(observe_generation(&second).unwrap(), 0);
    }
}
