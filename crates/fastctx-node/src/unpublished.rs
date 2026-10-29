//! Publication scratch whose writer no longer exists has no reader.

use std::io;
use std::path::Path;

pub(crate) fn collect(directory: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if kind.is_dir() {
            if fastctx_platform::owned_directory::is_link(&entry.metadata()?) {
                continue;
            }
            if name == "attachments" {
                uploads(&path)?;
            }
            // Attachments and command artifacts may legitimately have any name.
            if !matches!(name, "attachments" | "out" | "claude") {
                collect(&path)?;
            }
        } else if let Some((pid, sequence)) = name
            .strip_prefix(".fastctx-")
            .and_then(|name| name.strip_suffix(".tmp"))
            .and_then(|name| name.split_once('-'))
            && sequence.parse::<u64>().is_ok()
            && let Ok(pid) = pid.parse::<u32>()
            && pid != 0
            && fastctx_platform::identity_of(pid)?.is_none()
        {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn uploads(directory: &Path) -> io::Result<()> {
    for slot in crate::retention::entries(directory)? {
        let metadata = std::fs::symlink_metadata(&slot)?;
        if !metadata.is_dir() || fastctx_platform::owned_directory::is_link(&metadata) {
            continue;
        }
        fastctx_platform::owned_directory::delete(&slot.join("upload.part"))?;
        if !slot.join("receipt.json").try_exists()?
            && slot.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("attachment-"))
        {
            for name in ["content", "content.png", "content.jpg", "content.webp", "content.gif"] {
                fastctx_platform::owned_directory::delete(&slot.join(name))?;
            }
        }
        if std::fs::read_dir(&slot)?.next().transpose()?.is_none() {
            std::fs::remove_dir(slot)?;
        }
    }
    Ok(())
}
