//! The projects a node of the network's third version kept. Each one's
//! ledger, written by the World's machines and checked as it came in, is
//! kept from now on as a project of this computer on no hub (`super::alone`),
//! so that logging in carries it into the account the computer is
//! registered under (`super::carry`). Every machine that kept a project
//! names it alike, from its World's id and its name, so however many of
//! them carry it, they carry one project.
//!
//! ```text
//! towns/<name>/signed-third/   the third version's entries, kept as they were
//! ```

use std::io;

use fastctx_net::ledger::{self, Entry, File};
use fastctx_net::sign::{PublicKey, Signed};
use fastctx_types::access::TownId;
use fastctx_types::layout::WorldDir;
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::names::{self, Place};

/// An entry as the third version wrote it.
#[derive(Deserialize)]
struct ThirdEntry {
    world: String,
    town: TownName,
    author: MachineName,
    at: Timestamp,
    machines: Option<Vec<MachineName>>,
    files: Vec<File>,
}

/// Keeps each project of the third version this computer has as a project
/// of its own, its entries in their order; `here` is the name it wrote its
/// own entries under. Returns the projects kept so, and what could not be.
pub fn migrate(world: &WorldDir, here: &MachineName) -> io::Result<(Vec<TownName>, Vec<String>)> {
    let mut kept = Vec::new();
    let mut problems = Vec::new();
    for town in world.towns()? {
        if names::place(world, &town)?.is_some() {
            continue;
        }
        match migrate_town(world, here, &town) {
            Ok(true) => kept.push(town),
            Ok(false) => {}
            Err(error) => problems.push(format!("{town}: {error}")),
        }
    }
    Ok((kept, problems))
}

fn migrate_town(world: &WorldDir, here: &MachineName, town: &TownName) -> io::Result<bool> {
    let directory = world.town(town);
    let signed = directory.signed();
    let mut entries = Vec::new();
    for (_, path) in super::numbered(&signed)? {
        let wrapped: Signed = serde_json::from_slice(&std::fs::read(&path)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
        match wrapped.claim::<ThirdEntry>() {
            Ok(entry) if entry.town == *town => entries.push(entry),
            Ok(entry) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is an entry of {}", path.display(), entry.town),
                ));
            }
            // Not the third version's: nothing of it is changed.
            Err(_) => return Ok(false),
        }
    }
    let Some(first) = entries.first() else { return Ok(false) };
    let id = town_id(&first.world, town);
    let place = Place { hub: super::alone(world)?, town: id.clone() };
    let key = super::own_key(world)?;
    // An entry another machine wrote names a key that is not this
    // computer's, so carrying it into a hub leaves its author as it is.
    let theirs = PublicKey::from_bytes([0; 32]);
    let aside = directory.path().join("signed-third");
    std::fs::rename(&signed, &aside)?;
    for (index, entry) in entries.into_iter().enumerate() {
        let entry = Entry {
            hub: place.hub.clone(),
            town: id.clone(),
            key: if entry.author == *here { key.public() } else { theirs },
            author: entry.author,
            at: entry.at,
            machines: entry.machines,
            files: entry.files,
        };
        let signed = ledger::sign(&key, &entry);
        super::apply(world, town, index as u64 + 1, &entry, &signed)?;
    }
    let refused = directory.path().join("refused.json");
    if refused.exists() {
        std::fs::rename(&refused, directory.path().join("refused-third.json"))?;
    }
    names::set_place(world, town, &place)?;
    Ok(true)
}

/// The id every machine of the World `world` gives its project `town`.
fn town_id(world: &str, town: &TownName) -> TownId {
    let digest = Sha256::new()
        .chain_update(b"fastctx third version project\0")
        .chain_update(world.as_bytes())
        .chain_update([0])
        .chain_update(town.as_str().as_bytes())
        .finalize();
    TownId::from_bytes(digest[..8].try_into().expect("eight bytes"))
}
