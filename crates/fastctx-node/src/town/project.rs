//! `fastctx project`: opening a town, writing the versions of its record,
//! and a resident entering one (`world-projects.md`, 开一个项目).

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::project::{Record, RecordFields, Stamp, TownSummary, record_path};
use fastctx_types::{MachineName, ProjectCall, Reply, TownName};
use jiff::Timestamp;

use crate::Node;
use crate::residents::Heard;
use crate::town::caller::Caller;
use crate::town::{map, read};

pub async fn call(node: &Arc<Node>, caller: &Caller, call: ProjectCall) -> Result<Reply, Failure> {
    match call {
        ProjectCall::New { town, fields } => new(node, caller, town, fields).await,
        ProjectCall::Edit { town, fields } => {
            let town = caller.town_or(node, town)?;
            edit(node, caller, town, fields).await
        }
        ProjectCall::List => list(node).await.map(|towns| Reply::Towns { towns }),
        ProjectCall::Show { town } => {
            let town = caller.town_or(node, town)?;
            map::catch_up(node, &town).await?;
            let records = blocking(node, &town, read::records).await?;
            let (Some(first), Some(record)) = (records.first(), records.last()) else {
                return Err(map::no_ledger(node, &town));
            };
            let opened = Stamp { by: first.by.clone(), on: first.on.clone(), at: first.at };
            Ok(Reply::Record { town, record: Box::new(record.clone()), opened })
        }
        ProjectCall::Map { town } => {
            let town = caller.town_or(node, town)?;
            map::view(node, &town).await.map(|map| Reply::Map { map: Box::new(map) })
        }
        ProjectCall::Enter { town } => enter(node, caller, town).await,
    }
}

/// Opens a town: the first version of its record.
async fn new(node: &Arc<Node>, caller: &Caller, town: TownName, fields: RecordFields) -> Result<Reply, Failure> {
    let _writing = node.writing.lock().await;
    map::catch_up(node, &town).await?;
    if blocking(node, &town, read::latest).await?.is_some() {
        return Err(Failure::new(format!("the town {town} exists already; fastctx project edit {town} changes it")));
    }
    let name = fields.name.ok_or_else(|| Failure::new("a new town needs its name (--name)"))?;
    let what = fields.what.ok_or_else(|| Failure::new("a new town needs to say what it is (--what)"))?;
    let record = Record {
        version: 1,
        name,
        what,
        machines: fields.machines.unwrap_or_else(|| vec![node.machine()]),
        places: fields.places.unwrap_or_default(),
        canon: fields.canon,
        by: caller.writer(),
        on: node.machine(),
        at: Timestamp::now(),
    };
    let record = checked(node, record).await?;
    write(node, &town, &record).await.map_err(|failure| {
        if failure.message.contains("already") {
            Failure::new(format!("a town named {town} is in the World already ({failure})"))
        } else {
            failure
        }
    })?;
    let opened = Stamp { by: record.by.clone(), on: record.on.clone(), at: record.at };
    Ok(Reply::Record { town, record: Box::new(record), opened })
}

/// Writes the next version of a town's record: what `fields` gives
/// replaces what the latest version says; the rest stays.
async fn edit(node: &Arc<Node>, caller: &Caller, town: TownName, fields: RecordFields) -> Result<Reply, Failure> {
    let _writing = node.writing.lock().await;
    if fields == RecordFields::default() {
        return Err(Failure::new("nothing to change: give what the next version says"));
    }
    map::catch_up(node, &town).await?;
    let records = blocking(node, &town, read::records).await?;
    let (Some(first), Some(latest)) = (records.first(), records.last()) else {
        return Err(map::no_ledger(node, &town));
    };
    let record = Record {
        version: latest.version + 1,
        name: fields.name.unwrap_or_else(|| latest.name.clone()),
        what: fields.what.unwrap_or_else(|| latest.what.clone()),
        machines: fields.machines.unwrap_or_else(|| latest.machines.clone()),
        places: fields.places.unwrap_or_else(|| latest.places.clone()),
        canon: fields.canon.or_else(|| latest.canon.clone()),
        by: caller.writer(),
        on: node.machine(),
        at: Timestamp::now(),
    };
    let record = checked(node, record).await?;
    write(node, &town, &record).await.map_err(|failure| {
        if failure.message.contains("already") {
            Failure::new(format!(
                "another version {} of {town}'s record was written first; see it with fastctx project show {town} and edit from there",
                record.version
            ))
        } else {
            failure
        }
    })?;
    let opened = Stamp { by: first.by.clone(), on: first.on.clone(), at: first.at };
    Ok(Reply::Record { town, record: Box::new(record), opened })
}

/// The record with its machines checked against the World's, and its paths
/// written the way their machines write them.
async fn checked(node: &Arc<Node>, mut record: Record) -> Result<Record, Failure> {
    if record.machines.is_empty() {
        return Err(Failure::new("a town spans at least one machine"));
    }
    let known = machines(node).await;
    let unknown = |machine: &MachineName| !known.contains(machine);
    let named = record
        .machines
        .iter()
        .chain(record.places.iter().map(|place| &place.location.machine))
        .chain(record.canon.iter().map(|canon| &canon.machine));
    if let Some(stranger) = named.into_iter().find(|machine| unknown(machine)) {
        return Err(Failure::new(format!(
            "{stranger} is not a machine of this World; its machines are {}",
            known.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )));
    }
    for place in &mut record.places {
        place.location.path = absolute(&place.location.path).map_err(Failure::new)?;
        if place.what.trim().is_empty() {
            return Err(Failure::new(format!("the place {} says nothing of what is there", place.location.path)));
        }
    }
    if let Some(canon) = &mut record.canon {
        canon.path = absolute(&canon.path).map_err(Failure::new)?;
    }
    Ok(record)
}

/// The machines of the World as this machine knows them now.
async fn machines(node: &Arc<Node>) -> Vec<MachineName> {
    match node.link() {
        Some(link) => {
            // Asked afresh, or as last heard when the hub cannot be asked.
            let _ = link.refresh().await;
            link.members().0.members().map(|member| member.name.clone()).collect()
        }
        None => vec![node.machine()],
    }
}

/// A path as the World writes it: absolute, and on Windows with forward
/// slashes. A path is a machine's own; nothing else is done to it.
fn absolute(path: &str) -> Result<String, String> {
    let bytes = path.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\');
    let unc = path.starts_with("\\\\") || path.starts_with("//");
    if path.starts_with('/') && !unc {
        Ok(path.to_string())
    } else if drive || unc {
        Ok(path.replace('\\', "/"))
    } else {
        Err(format!("{path:?} is not an absolute path; a place is a machine and a full path on it"))
    }
}

async fn write(node: &Arc<Node>, town: &TownName, record: &Record) -> Result<(), Failure> {
    let mut bytes = serde_json::to_vec_pretty(record).map_err(|error| Failure::new(error.to_string()))?;
    bytes.push(b'\n');
    crate::ledger::write(node, town, vec![(record_path(record.version), bytes)], Some(record.machines.clone())).await
}

/// Refuses unless `town` spans this machine: a resident lives in a town on
/// the machine its session runs on, and the town's `w` asks only the
/// machines it spans.
pub async fn spans_here(node: &Arc<Node>, town: &TownName) -> Result<(), Failure> {
    let here = node.machine();
    map::catch_up(node, town).await?;
    let record = blocking(node, town, read::latest).await?.ok_or_else(|| map::no_ledger(node, town))?;
    if record.machines.contains(&here) {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "{town} does not span {here}; its machines are {}",
            record.machines.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )))
    }
}

/// The towns this machine keeps a ledger for and that span it.
pub async fn list(node: &Arc<Node>) -> Result<Vec<TownSummary>, Failure> {
    let (world, here) = (node.world.clone(), node.machine());
    tokio::task::spawn_blocking(move || {
        let mut towns = Vec::new();
        for town in world.towns()? {
            let Some(record) = read::latest(&world, &town)? else { continue };
            if record.machines.contains(&here) {
                let journals = read::journal_names(&world, &town)?.len();
                towns.push(TownSummary { town, record, journals });
            }
        }
        std::io::Result::Ok(towns)
    })
    .await
    .map_err(|error| Failure::new(format!("reading the towns: {error}")))?
    .map_err(|error| Failure::new(format!("reading the towns: {error}")))
}

/// The calling resident lives in `town` from now on, and is given its map.
async fn enter(node: &Arc<Node>, caller: &Caller, town: TownName) -> Result<Reply, Failure> {
    let here = node.machine();
    let id = match caller {
        Caller::Resident(address) if address.machine == here => address.resident.clone(),
        Caller::Resident(address) => {
            return Err(Failure::new(format!(
                "{} lives on {}; enter the town there, running the command without a node",
                address.resident, address.machine
            )));
        }
        Caller::User => {
            return Err(Failure::new(
                "only a resident enters a town: an agent's session, which runs this through the World's tools",
            ));
        }
        Caller::Session(agent) => {
            return Err(Failure::new(format!(
                "the World does not know this {agent} session yet; it becomes a resident at its first call of a World tool, and enters a town after that"
            )));
        }
    };
    map::catch_up(node, &town).await?;
    let record = blocking(node, &town, read::latest).await?.ok_or_else(|| map::no_ledger(node, &town))?;
    if !record.machines.contains(&here) {
        return Err(Failure::new(format!(
            "{town} does not span {here}, where {id} lives; its machines are {}",
            record.machines.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )));
    }
    let (world, resident, name) = (node.world.clone(), id.clone(), town.clone());
    let moved = {
        let node = node.clone();
        tokio::task::spawn_blocking(move || node.residents.enter(&world, &resident, &name))
            .await
            .map_err(|error| Failure::new(format!("moving {id}: {error}")))?
            .map_err(|error| Failure::new(format!("moving {id}: {error}")))?
    };
    if moved {
        crate::codex::moved(node, &id).await;
    }
    let map = map::view(node, &town).await?;
    let ledger = crate::ledger::seq(&node.world, &town).map_err(|error| Failure::new(error.to_string()))?;
    node.residents.set_heard(&id, Heard { town: Some(town.clone()), ledger });
    Ok(Reply::Entered { town, map: Box::new(map) })
}

async fn blocking<T: Send + 'static>(
    node: &Arc<Node>,
    town: &TownName,
    read: fn(&fastctx_types::layout::WorldDir, &TownName) -> std::io::Result<T>,
) -> Result<T, Failure> {
    let (world, name) = (node.world.clone(), town.clone());
    tokio::task::spawn_blocking(move || read(&world, &name))
        .await
        .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))?
        .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))
}
