//! Registration discovery reads durable descriptions only. It never loads a
//! unit, recovers a transaction, creates a directory, or takes a writer lock.

use super::{Kind, Service};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Registration {
    pub name: String,
    pub kind: Kind,
    pub executable: PathBuf,
    pub directory: PathBuf,
    pub artifacts: Vec<PathBuf>,
}

impl Registration {
    pub fn service(&self, installation: &crate::uninstall::Installation) -> Service {
        let mut service = Service::new(self.kind, &self.executable, &self.directory);
        #[cfg(windows)]
        if self.kind == Kind::Node {
            service.user_sid = Some(installation.user_sid.clone());
        }
        #[cfg(target_os = "linux")]
        if self.kind == Kind::Node {
            service.uid = installation.uid;
        }
        service
    }

    /// The registration is removed first. The backend's writer locks have
    /// been released before its remaining product files are retired.
    pub fn uninstall(&self, installation: &crate::uninstall::Installation) -> anyhow::Result<Vec<PathBuf>> {
        self.service(installation).uninstall_registration()?;
        let mut left = Vec::new();
        for artifact in &self.artifacts {
            match std::fs::symlink_metadata(artifact) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
                Ok(metadata) if crate::disk::is_link(&metadata) => left.push(artifact.clone()),
                Ok(metadata) if metadata.is_dir() => {
                    for entry in std::fs::read_dir(artifact)? {
                        let entry = entry?;
                        let name = entry.file_name();
                        let name = name.to_str().unwrap_or("");
                        if matches!(
                            name,
                            "guardian.exe"
                                | "payload.exe"
                                | "config.json"
                                | "guardian.log"
                                | "running.json"
                                | "manager.lock"
                                | "transaction.json"
                                // A renewal of the protected guardian leaves its receipt
                                // beside the image it replaced.
                                | "guardian.renewal-files.json"
                                | "payload.renewal-files.json"
                        ) {
                            crate::uninstall::delete_file(&entry.path())?;
                        } else if crate::executable::is_retired(&entry.path()) {
                            // The earlier image a renewal kept while it was mapped.
                            crate::executable::delete_retired(&entry.path())
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                        } else {
                            left.push(entry.path());
                        }
                    }
                    if std::fs::read_dir(artifact)?.next().is_none() {
                        std::fs::remove_dir(artifact)?;
                    }
                }
                Ok(_) => crate::uninstall::delete_file(artifact)?,
            }
        }
        Ok(left)
    }
}

pub fn registrations(installation: &crate::uninstall::Installation) -> anyhow::Result<Vec<Registration>> {
    installation.validate()?;
    #[cfg(windows)]
    return super::windows::registrations(installation);
    #[cfg(target_os = "linux")]
    return super::linux::registrations(installation);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = installation;
        anyhow::bail!("uninstall supports Windows and Linux")
    }
}

#[cfg(target_os = "linux")]
pub(super) fn entries(path: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
