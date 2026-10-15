//! Removing the macOS application, a `.app` bundle the person dragged into
//! place: once the application has exited, from a copy of fastctx outside it.

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use fastctx_install::uninstall::Installation;
use fastctx_types::uninstall::Event;

/// The private directory a removal helper runs from begins with this.
pub const TEMPORARY_PREFIX: &str = "fastctx-bundle-uninstall-";

/// Waits for `process`, the application, to exit, then removes `bundle` and,
/// for a complete removal, the application's `data` directories.
pub async fn remove_bundle(
    bundle: &Path,
    process: &fastctx_types::ProcessIdentity,
    temporary: &Path,
    data: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let mut installation = Installation::here()?;
    installation.application_dirs = data;
    installation.validate()?;
    let metadata = std::fs::symlink_metadata(bundle)?;
    anyhow::ensure!(
        bundle.is_absolute()
            && metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && bundle.extension().is_some_and(|extension| extension == "app")
            && bundle.join("Contents/Info.plist").is_file()
            && metadata.uid() == installation.uid,
        "the application is not an owned macOS bundle"
    );
    anyhow::ensure!(
        std::env::current_exe()?.parent() == Some(temporary)
            && temporary.file_name().is_some_and(|name| name.to_string_lossy().starts_with(TEMPORARY_PREFIX)),
        "the removal helper does not own this temporary directory"
    );
    fastctx_platform::host_process::exited(process).await?;
    let current = std::fs::symlink_metadata(bundle)?;
    anyhow::ensure!(
        current.dev() == metadata.dev() && current.ino() == metadata.ino(),
        "the application changed before its removal"
    );
    std::fs::remove_dir_all(bundle)?;
    let left = crate::execute::application_data(&installation.application_dirs)?;
    if !left.is_empty() {
        println!("{}", serde_json::to_string(&Event::Finished { left })?);
    }
    // The helper's own copy goes once it has exited.
    let script = format!("while kill -0 {} 2>/dev/null; do sleep 0.05; done; rm -rf -- \"$0\"", std::process::id());
    std::process::Command::new("/bin/sh")
        .args(["-c", &script])
        .arg(temporary)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}
