//! Delivery commands finish from the selected stable World program.

use std::path::Path;

pub fn deliver(root: &Path) -> anyhow::Result<Option<anyhow::Result<()>>> {
    let installed = fastctx_install::placement::ensure(root, &std::env::current_exe()?)?;
    forward_if_other(root, &installed)
}

/// Ordinary World commands use the selected kernel, including while its
/// boot-service handover awaits approval. Only a newer delivery installs.
pub fn world(root: &Path) -> anyhow::Result<Option<anyhow::Result<()>>> {
    let installed = fastctx_install::renewal::installed(root);
    if installed.is_file() {
        let current = fastctx_install::program::inspect(&installed)?;
        let own = fastctx_install::renewal::build()?;
        if own.version == current.version || !fastctx_install::program::newer(&own, &current)? {
            return forward_if_other(root, &installed);
        }
    }
    deliver(root)
}

pub fn forward_if_other(root: &Path, installed: &Path) -> anyhow::Result<Option<anyhow::Result<()>>> {
    let source = match std::fs::canonicalize(std::env::current_exe()?) {
        Ok(source) => Some(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if source.as_deref() == Some(std::fs::canonicalize(installed)?.as_path())
        && fastctx_install::renewal::build()? == fastctx_install::program::inspect(installed)?
    {
        return Ok(None);
    }
    forward(root, installed).map(Some)
}

pub fn forward(root: &Path, installed: &Path) -> anyhow::Result<anyhow::Result<()>> {
    let mut command = std::process::Command::new(installed);
    command.args(std::env::args_os().skip(1));
    command.env("FASTCTX_HOME", root);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        Ok(Err(command.exec().into()))
    }
    #[cfg(windows)]
    {
        let status = command.status()?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(Ok(()))
    }
}
