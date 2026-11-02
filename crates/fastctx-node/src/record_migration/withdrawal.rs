//! Removing explicitly withdrawn source data is separate from migration success.
//! Both verified copies and damaged originals follow the original file's owner.
use fastctx_platform::{disk, readings};
use std::{io, path::Path};

/// The caller holds the conversation's file gate and has durably recorded the
/// withdrawal. No source outside that exact logical path is removed.
pub(crate) fn source(world: &Path, conversation: &str, source: &Path) -> io::Result<()> {
    let relative = super::archive::relative(world, conversation, source)?;
    let filename = relative.file_name().ok_or_else(|| io::Error::other("a withdrawn source needs a filename"))?;
    let prefix = format!("{}.", filename.to_string_lossy());
    let versions = match disk::read_dir(world.join("archive")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for version in versions {
        let version = version?;
        if !version.file_type()?.is_dir() {
            continue;
        }
        for original in [
            version.path().join(conversation).join(&relative),
            version.path().join("damaged").join(conversation).join(&relative),
        ] {
            let parent = original.parent().expect("archive source parent");
            let entries = match disk::read_dir(parent) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                if entry.file_type()?.is_file() && entry.file_name().to_string_lossy().starts_with(&prefix) {
                    match disk::remove_file(entry.path()) {
                        Ok(()) => {
                            readings::counter("migration.withdrawn_files").add(1);
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }
    Ok(())
}
