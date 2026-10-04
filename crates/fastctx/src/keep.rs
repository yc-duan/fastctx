//! Keeping this machine connected once it is in a World (`world-network.md`,
//! 加入了就一直连着): a system service starts its node at boot, before
//! anyone signs in, and brings it back whenever it stops. Installing one
//! takes administrator rights, asked for once.

use anyhow::bail;
use fastctx_install::apply::{Places, place_executable};
use fastctx_types::Call;
use fastctx_types::layout::WorldDir;

pub fn keep_connected() -> anyhow::Result<()> {
    let root = fastctx_platform::fastctx_root()?;
    if WorldDir::under(&root).node_service().exists() {
        println!("A system service keeps this machine's node running, signed in or not.");
        return Ok(());
    }
    if !cfg!(any(windows, target_os = "linux")) {
        println!("This system has no service for the node: it runs while you are signed in.");
        return Ok(());
    }
    let mut places = Places::here()?;
    places.root = root.clone();
    let (executable, report) = place_executable(&places)?;
    for leftover in &report.leftovers {
        println!("could not clear: {leftover}");
    }
    if fastctx_platform::elevation::is_elevated()? {
        crate::service::install_node(&root, &executable)?;
    } else if !install_elevated(&root, &executable)? {
        return Ok(());
    }
    // The service's node waits for the World, which this one hands over
    // once it is idle.
    crate::ask::call(Call::HandOver)?;
    println!("A system service keeps this machine's node running from now on, signed in or not.");
    Ok(())
}

/// Installs the node's service with rights this process does not have;
/// whether it was installed.
#[cfg(windows)]
fn install_elevated(root: &std::path::Path, executable: &std::path::Path) -> anyhow::Result<bool> {
    println!(
        "Keeping this machine connected when no one is signed in takes a system service; \
         Windows asks for administrator rights once to install it."
    );
    let user = fastctx_platform::current_user_sid()?;
    let args: Vec<std::ffi::OsString> = vec![
        "service".into(),
        "node".into(),
        "install".into(),
        "--dir".into(),
        root.into(),
        "--executable".into(),
        executable.into(),
        "--user-sid".into(),
        user.into(),
    ];
    match fastctx_platform::elevation::run_elevated(executable, &args) {
        Ok(0) => Ok(true),
        Ok(code) => bail!(
            "installing the node's service failed (exit {code}); run `fastctx service node install` \
             as administrator to see why"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            println!(
                "Not installed: the node runs while you are signed in. Run `fastctx service node install` \
                 as administrator to install it later."
            );
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn install_elevated(root: &std::path::Path, executable: &std::path::Path) -> anyhow::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(root)?.uid();
    println!(
        "Keeping this machine connected when no one is signed in takes a system service. Install it with:\n  \
         sudo {} service node install --dir {} --executable {} --uid {uid}",
        executable.display(),
        root.display(),
        executable.display()
    );
    Ok(false)
}
