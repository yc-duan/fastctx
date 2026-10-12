//! Creating a local network needs both services. Their installation shares
//! one elevation; installing a hub on its own continues to install only it.

use std::ffi::OsString;
use std::path::Path;

use super::{Kept, Kind, Service};
use crate::apply::{Places, place_executable};

/// Installs the local hub and this user's node with one administrator
/// request. Returns the hub and node outcomes separately, so the caller
/// hands an existing node over only when its service was installed here.
pub fn keep_local_network(places: &Places, hub: &Path) -> anyhow::Result<(Kept, Kept)> {
    let (executable, _) = place_executable(places)?;
    super::hub_registry::remember(&places.root, hub)?;
    let node_exists = fastctx_types::layout::WorldDir::under(&places.root).node_service().try_exists()?;
    let user = fastctx_platform::current_user_sid()?;
    let args: Vec<OsString> = vec![
        "service-local-network".into(),
        "--dir".into(),
        places.root.clone().into(),
        "--hub-dir".into(),
        hub.into(),
        "--executable".into(),
        executable.clone().into(),
        "--user-sid".into(),
        user.clone().into(),
    ];
    let kept = if fastctx_platform::elevation::is_elevated()? {
        install_local_network(&places.root, hub, &executable, &user)?;
        Kept::Installed
    } else {
        match fastctx_platform::elevation::run_elevated(&executable, &args) {
            Ok(0) => Kept::Installed,
            Ok(code) => {
                anyhow::bail!("`fastctx service-local-network` failed (exit {code}); run as administrator, it says why")
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Kept::Declined,
            Err(error) => return Err(error.into()),
        }
    };
    let node = if node_exists { Kept::Already } else { kept.clone() };
    Ok((kept, node))
}

/// The elevated phase. Each existing service installer retains its own
/// rollback semantics; a node failure reports that the hub remains installed.
pub fn install_local_network(root: &Path, hub: &Path, executable: &Path, user: &str) -> anyhow::Result<()> {
    use anyhow::Context as _;
    anyhow::ensure!(
        fastctx_platform::elevation::is_elevated()?,
        "installing local network services requires administrator rights"
    );
    Service::new(Kind::Hub, executable, hub).install()?;
    if !fastctx_types::layout::WorldDir::under(root).node_service().try_exists()? {
        let node = Service { user_sid: Some(user.into()), ..Service::new(Kind::Node, executable, root) };
        node.install().context("the hub service is installed, but installing the node service failed")?;
    }
    Ok(())
}
