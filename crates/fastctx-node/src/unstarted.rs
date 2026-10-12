//! Preparations without a published job have no supervisor or reader.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_types::JobId;
use fastctx_types::layout::WorldDir;

pub(crate) fn collect(world: &WorldDir) -> io::Result<()> {
    let mut directories = Vec::new();
    for parent in world.job_parents()? {
        for directory in crate::retention::entries(&parent)? {
            if fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&directory)?) {
                continue;
            }
            directories.push(directory);
        }
    }
    let mut referenced = HashSet::new();
    for directory in &directories {
        for path in crate::retention::entries(&directory.join("jobs"))? {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let record = fastctx_hands::run::record::read(&path)?;
            for path in [record.stdin.as_deref(), record.on.as_ref().map(|on| on.state.as_str())].into_iter().flatten()
            {
                match std::fs::canonicalize(path) {
                    Ok(path) => {
                        referenced.insert(path);
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    for directory in directories {
        let jobs = directory.join("jobs");
        for path in crate::retention::entries(&jobs)? {
            let Some(job) = path.file_stem().and_then(|stem| stem.to_str()).and_then(|stem| stem.parse::<JobId>().ok())
            else {
                continue;
            };
            if jobs.join(format!("{job}.json")).try_exists()? {
                continue;
            }
            if path.extension().is_some_and(|extension| {
                ["sh", "lock", "owed", "delivered", "notice", "waited", "consumed", "exit-receipt", "writes"]
                    .iter()
                    .any(|name| extension == *name)
            }) {
                fastctx_platform::owned_directory::delete(&path)?;
            }
        }
        for path in crate::retention::entries(&directory.join("out"))? {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
            if path.is_dir() || fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&path)?) {
                continue;
            }
            let job = name
                .strip_suffix(".stderr.log")
                .or_else(|| name.strip_suffix(".log"))
                .and_then(|stem| stem.parse::<JobId>().ok());
            let package = name.starts_with("on-")
                && ["state", "in"].iter().any(|extension| path.extension().is_some_and(|found| found == *extension));
            if job.as_ref().is_some_and(|job| jobs.join(format!("{job}.json")).exists())
                || protected(&referenced, &path)?
            {
                continue;
            }
            if job.is_some() || package {
                fastctx_platform::owned_directory::delete(&path)?;
            }
        }
    }
    Ok(())
}

fn protected(referenced: &HashSet<PathBuf>, path: &Path) -> io::Result<bool> {
    Ok(referenced.contains(&std::fs::canonicalize(path)?))
}
