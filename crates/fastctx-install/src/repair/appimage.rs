//! An AppImage's old mount path is not an installed program path. Read the
//! carried kernel from its registered image without launching the application.

use anyhow::{Context as _, ensure};
use std::path::Path;

pub(super) fn program(
    root: &Path,
    app: &super::application::Registration,
) -> anyhow::Result<fastctx_types::renewal::Build> {
    let image = app.executable()?;
    let directory = tempfile::Builder::new()
        .prefix("appimage-check-")
        .tempdir_in(root.join("updates"))
        .with_context(|| format!("preparing AppImage inspection for {}", image.display()))?;
    let mut command = crate::program::command(&image);
    command
        .args(["--appimage-extract", "usr/bin/fastctx"])
        .current_dir(directory.path())
        .env_remove("APPDIR")
        .env_remove("APPIMAGE")
        .env_remove("APPIMAGE_EXTRACT_AND_RUN");
    let output = crate::program::answer::answer(&mut command, &image)
        .with_context(|| format!("reading carried kernel from AppImage {}", image.display()))?;
    ensure!(
        output.status.success(),
        "extracting carried kernel from {} exited {}: {}",
        image.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let kernel = directory.path().join("squashfs-root/usr/bin/fastctx");
    let build = crate::program::inspect(&kernel)
        .with_context(|| format!("identifying carried kernel in {}", image.display()))?;
    directory.close().with_context(|| format!("removing AppImage inspection for {}", image.display()))?;
    Ok(build)
}
