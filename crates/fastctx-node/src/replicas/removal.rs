//! Withdrawal never follows a redirected directory into someone else's files.

use fastctx_types::layout::WorldDir;
use std::{
    io,
    path::{Component, Path, PathBuf},
};

/// Removes the first redirected parent below the World, without reading its
/// target. The World and the directories that hold it stay as they were
/// placed, links or not.
pub(crate) fn redirected(world: &WorldDir, path: &Path) -> io::Result<bool> {
    let root = super::state::path_key(world.path());
    let below = |path: &Path| super::state::path_key(path).starts_with(&format!("{root}/"));
    if !path.is_absolute() || !below(path) || path.components().any(|part| part == Component::ParentDir) {
        return Err(io::Error::other("a replica cleanup path is outside its World"));
    }
    let mut parent = PathBuf::new();
    for component in path.parent().unwrap_or(path).components() {
        parent.push(component);
        if !below(&parent) {
            continue;
        }
        match fastctx_platform::disk::symlink_metadata(&parent) {
            Ok(metadata) if fastctx_platform::owned_directory::is_link(&metadata) => {
                fastctx_platform::owned_directory::delete(&parent)?;
                return Ok(true);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub(crate) fn delete(world: &WorldDir, path: &Path) -> io::Result<()> {
    if redirected(world, path)? {
        return Ok(());
    }
    super::files::delete(path)
}
