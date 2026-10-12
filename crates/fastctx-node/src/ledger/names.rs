//! Which hub keeps each project's ledger, and the project's id there
//! (`fastctx_types::access::TownId`), kept beside the project's copy as
//! `project.json`. A project is known locally by its short name; on the
//! wire and at the hub by its id, chosen at random when it is opened.

use std::io;

use fastctx_net::ids::HubId;
use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::TownName;
use fastctx_types::access::TownId;
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};

use crate::Node;

/// Where a project's ledger is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// The hub; for a project of a computer on no hub, the id it keeps for
    /// itself (`super::alone`).
    pub hub: HubId,
    pub town: TownId,
}

fn path(world: &WorldDir, town: &TownName) -> std::path::PathBuf {
    world.town(town).path().join("project.json")
}

/// Where `town`'s ledger is kept, if this machine has it.
pub fn place(world: &WorldDir, town: &TownName) -> io::Result<Option<Place>> {
    match std::fs::read(path(world, town)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map(Some).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn set_place(world: &WorldDir, town: &TownName, place: &Place) -> io::Result<()> {
    ensure_private_dir(world.town(town).path())?;
    let mut bytes = serde_json::to_vec_pretty(place).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(&path(world, town), &bytes, Permissions::Private)
}

/// The id of the project `town` at its hub.
pub fn town_id(node: &Node, town: &TownName) -> Option<TownId> {
    place(&node.world, town).ok().flatten().map(|place| place.town)
}
