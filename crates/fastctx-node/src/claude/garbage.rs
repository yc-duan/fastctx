//! Exchange leftovers retire when their writer and request helper have ended.

use std::io;
use std::path::Path;

use fastctx_hosts::claude::{control, exchange};
use fastctx_types::layout::WorldDir;

pub(crate) fn collect(world: &WorldDir) -> io::Result<()> {
    let mut sessions = crate::retention::entries(&exchange::sessions(world.path()))?;
    for kept in world.kept()? {
        sessions.extend(crate::retention::entries(&kept.directory.path().join("claude"))?);
    }
    for session in sessions {
        if fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&session)?) {
            continue;
        }
        let alive = exchange::registration(&session)?
            .and_then(|registration| registration.process)
            .map(|process| fastctx_platform::is_alive(&process))
            .transpose()?
            .unwrap_or(false);
        if !alive {
            for directory in [&session, &session.join("events"), &session.join("control")] {
                parts(directory, &session.join("damaged"))?;
            }
            let retired =
                session.parent().and_then(Path::parent).is_some_and(|home| home.join("books-retired").exists());
            if retired {
                for name in ["applied", "receipts", "attempts", "closed", "calls", "inbox-parts"] {
                    fastctx_platform::owned_directory::delete(&session.join(name))?;
                }
            }
        }
        let directory = session.join("control");
        if let Some(host) = control::host(&directory)?
            && !fastctx_platform::is_alive(&host.process)?
        {
            for side in ["inbox", "replies"] {
                fastctx_platform::owned_directory::delete(&directory.join(side))?;
            }
        }
        for path in crate::retention::entries(&directory.join("requests"))? {
            if let Some(waiting) = control::read::<control::Waiting>(&path.join("request.json"))?
                && !fastctx_platform::is_alive(&waiting.process)?
                && path.join("resolved.json").try_exists()?
            {
                fastctx_platform::owned_directory::delete(&path)?;
            }
        }
    }
    Ok(())
}

fn parts(directory: &Path, damaged: &Path) -> io::Result<()> {
    for path in crate::retention::entries(directory)? {
        if path.extension().is_none_or(|extension| extension != "parts") {
            continue;
        }
        let manifest = path.with_extension("json");
        let count = match std::fs::read(&manifest) {
            Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                .map_err(io::Error::other)?
                .get("parts")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if let Some(name) = manifest.file_name()
                    && damaged.join(name).try_exists()?
                {
                    std::fs::rename(&path, damaged.join(path.file_name().expect("parts name")))?;
                    continue;
                }
                0
            }
            Err(error) => return Err(error),
        };
        if count == 0 {
            fastctx_platform::owned_directory::delete(&path)?;
        } else {
            for piece in crate::retention::entries(&path)? {
                if piece
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<u64>().ok())
                    .is_some_and(|index| index >= count)
                {
                    fastctx_platform::owned_directory::delete(&piece)?;
                }
            }
        }
    }
    Ok(())
}
