//! Non-secret coordinates retained by the original user before elevation.
//! Protected guardian descriptions cannot be enumerated by that user later.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Hubs {
    root: PathBuf,
    directories: Vec<PathBuf>,
}

pub fn registered_hubs(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let bytes = match std::fs::read(root.join("hub-services.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let hubs: Hubs = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        hubs.root == root && hubs.directories.iter().all(|directory| directory.is_absolute()),
        "hub registration coordinates belong to another installation"
    );
    Ok(hubs.directories)
}

pub(super) fn remember(root: &Path, directory: &Path) -> anyhow::Result<()> {
    let mut directories = registered_hubs(root)?;
    if directories.iter().any(|known| known == directory) {
        return Ok(());
    }
    directories.push(directory.to_path_buf());
    fastctx_platform::ensure_private_dir(root)?;
    fastctx_platform::publish(
        &root.join("hub-services.json"),
        &serde_json::to_vec(&Hubs { root: root.to_path_buf(), directories })?,
        fastctx_platform::Permissions::Private,
    )?;
    Ok(())
}
