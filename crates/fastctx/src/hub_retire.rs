//! Ending the hub of a dissolved network kept on this machine before every
//! machine that was on it was told (`world-network.md`, 解散网络): its system
//! service is removed, which stops it, and its data is deleted, all but its
//! log. A hub ends so by itself once all were told; this is for making
//! another network where it is kept.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fastctx_install::service::{Kind, Service};
use fastctx_types::HubName;

/// Ends the hub of the dissolved network kept in `dir` with this process's
/// rights, which must be an administrator's; its name, and how many
/// machines were not told.
pub fn retire(dir: &Path) -> anyhow::Result<(HubName, usize)> {
    let Some((name, _)) = fastctx_hub::dissolve::dissolved(dir)? else {
        anyhow::bail!("{} holds no hub of a dissolved network", dir.display());
    };
    anyhow::ensure!(
        fastctx_platform::elevation::is_elevated()?,
        "ending the hub of {name} removes its system service: run this as administrator"
    );
    let service = Service::new(Kind::Hub, program(dir)?, dir);
    if service.status()?.installed {
        service.uninstall()?;
    }
    let untold = fastctx_hub::dissolve::retire(dir)?;
    Ok((name, untold))
}

/// Ends it as `retire` does, asking for the rights first where this process
/// lacks them: through sudo, read from standard input without a terminal,
/// or the system's prompt.
pub fn retire_elevated(dir: &Path) -> anyhow::Result<()> {
    if fastctx_platform::elevation::is_elevated()? {
        return retire(dir).map(|_| ());
    }
    let program = std::env::current_exe()?;
    let args: Vec<OsString> = vec!["hub".into(), "retire".into(), "--dir".into(), dir.into()];
    #[cfg(unix)]
    {
        use std::io::IsTerminal as _;
        let mut sudo = std::process::Command::new("sudo");
        if !std::io::stdin().is_terminal() {
            sudo.args(["-S", "-p", ""]);
        }
        let status = sudo
            .arg(&program)
            .args(&args)
            .status()
            .map_err(|error| anyhow::anyhow!("starting sudo to end the dissolved network's hub: {error}"))?;
        anyhow::ensure!(status.success(), "ending the dissolved network's hub through sudo failed ({status})");
    }
    #[cfg(windows)]
    {
        match fastctx_platform::elevation::run_elevated(&program, &args) {
            Ok(0) => {}
            Ok(code) => {
                anyhow::bail!("`fastctx hub retire` failed (exit {code}); run it as administrator, it says why")
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                anyhow::bail!("administrator rights were declined; the dissolved network's hub still runs here")
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// The World program the hub's service in `dir` runs from: the installed
/// one of the fastctx root it is kept under, not the copy the service runs,
/// which its removal deletes; this one when there is none.
pub fn program(dir: &Path) -> anyhow::Result<PathBuf> {
    let installed = dir.parent().map(fastctx_install::executable::installed_path).filter(|path| path.is_file());
    match installed {
        Some(installed) => Ok(installed),
        None => Ok(std::env::current_exe()?),
    }
}
