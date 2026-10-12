//! Installation can restore an enrolled computer without starting a node
//! merely to find whether a standalone computer needs a boot service.

use std::io;

use fastctx_net::ids::{CertPin, HubId};
use fastctx_net::link::{Membership, state};
use fastctx_types::layout::WorldDir;
use serde::Deserialize;

pub(super) fn any(world: &WorldDir) -> io::Result<bool> {
    let net = world.net();
    match state::read::<Legacy>(&net.join("membership.json")) {
        Ok(_) => return Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let entries = match std::fs::read_dir(&net) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        let Some(id) =
            path.file_name().and_then(|name| name.to_str()).and_then(|name| HubId::try_from(name.to_owned()).ok())
        else {
            continue;
        };
        match state::read::<Membership>(&path.join("membership.json")) {
            Ok(membership) if membership.id == id => return Ok(true),
            Ok(membership) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} holds the membership of {}", path.display(), membership.id),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

// The installer only recognizes the old membership; the node owns migration.
#[derive(Deserialize)]
struct Legacy {
    #[serde(rename = "world")]
    _world: HubId,
    #[serde(rename = "name")]
    _name: fastctx_types::MachineName,
    #[serde(rename = "hub")]
    _hub: Vec<std::net::SocketAddr>,
    #[serde(rename = "cert")]
    _cert: CertPin,
}
