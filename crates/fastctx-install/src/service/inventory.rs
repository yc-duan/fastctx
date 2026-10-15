//! Registration discovery reads durable descriptions only. It never loads a
//! unit, recovers a transaction, creates a directory, or takes a writer lock.

use super::{Kind, Service};
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "linux", target_os = "macos"))]
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
        #[cfg(any(target_os = "linux", target_os = "macos"))]
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
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    left.extend(retire_services_dir(artifact)?);
                    continue;
                }
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
                                | "guardian.log.1"
                                | "service-hub.log"
                                | "service-hub.log.1"
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
                        left.extend(retire_services_dir(artifact)?);
                    }
                }
                Ok(_) => crate::uninstall::delete_file(artifact)?,
            }
        }
        Ok(left)
    }
}

/// The protected directory every Windows service of fastctx lives under goes
/// with the last of them; while it holds anything but the lock they share, it
/// stays. With it go the digests their guardians kept under SYSTEM's profile;
/// what is there that fastctx did not write is returned and stays.
fn retire_services_dir(service: &std::path::Path) -> anyhow::Result<Vec<PathBuf>> {
    let Some(services) =
        service.parent().filter(|parent| parent.file_name().is_some_and(|name| name == "fastctx-services"))
    else {
        return Ok(Vec::new());
    };
    let names = match std::fs::read_dir(services) {
        Ok(entries) => entries.map(|entry| Ok(entry?.file_name())).collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    if !names.iter().all(|name| name == "policy.lock") {
        return Ok(Vec::new());
    }
    if !names.is_empty() {
        crate::uninstall::delete_file(&services.join("policy.lock"))?;
        std::fs::remove_dir(services)?;
    }
    #[cfg(windows)]
    return super::windows::retire_system_cache();
    #[cfg(not(windows))]
    Ok(Vec::new())
}

pub fn registrations(installation: &crate::uninstall::Installation) -> anyhow::Result<Vec<Registration>> {
    installation.validate()?;
    #[cfg(windows)]
    return super::windows::registrations(installation);
    #[cfg(target_os = "linux")]
    return super::linux::registrations(installation);
    #[cfg(target_os = "macos")]
    return super::macos::registrations(installation);
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = installation;
        anyhow::bail!("uninstall supports Windows, Linux and macOS")
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn entries(path: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
