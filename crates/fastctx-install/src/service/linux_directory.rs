//! Prepares a node's marker directory before installing any system units.

use std::fs;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
use std::os::unix::process::CommandExt as _;
use std::process::Command;

use anyhow::{Context as _, ensure};

pub(super) fn prepare(service: &super::Service) -> anyhow::Result<()> {
    if service.kind != super::Kind::Node {
        return Ok(());
    }
    let directory = super::keep::marker(&service.directory).parent().expect("node marker parent").to_owned();
    if let Ok(metadata) = fs::symlink_metadata(&directory) {
        ensure!(metadata.is_dir(), "{} is not a plain directory", directory.display());
        return Ok(());
    }
    // 2026-10-20: creating as the service user avoids a root-owned private directory
    // and never grants root's chown authority to a path in a user's directory.
    if service.uid == unsafe { libc::getuid() } {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .or_else(|error| if error.kind() == std::io::ErrorKind::AlreadyExists { Ok(()) } else { Err(error) })?;
    } else {
        let account = fastctx_platform::unix_account::lookup(service.uid)?.with_context(|| {
            format!("user {} has no account record; cannot prepare its World directory", service.uid)
        })?;
        super::checked(
            Command::new("mkdir").args(["-p", "-m", "700", "--"]).arg(&directory).uid(service.uid).gid(account.gid),
        )
        .with_context(|| format!("preparing {} for user {}", directory.display(), service.uid))?;
    }
    let metadata = fs::symlink_metadata(&directory)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == service.uid,
        "{} must be a plain directory owned by user {}",
        directory.display(),
        service.uid
    );
    Ok(())
}
