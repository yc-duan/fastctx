//! `fastctx project`: opening a town, writing the versions of its record,
//! and a resident entering one (`world-projects.md`, 开一个项目).

use std::sync::Arc;

use fastctx_types::access::{HubAccount, HubId};
use fastctx_types::ipc::Failure;
use fastctx_types::project::{Record, RecordFields, Stamp, record_path};
use fastctx_types::{AccountName, HubName, MachineName, ProjectCall, Reply, TownName};
use jiff::Timestamp;

use crate::Node;

use crate::town::caller::Caller;
use crate::town::{map, read};

pub async fn call(node: &Arc<Node>, caller: &Caller, call: ProjectCall) -> Result<Reply, Failure> {
    if matches!(&call, ProjectCall::New { fields, .. } | ProjectCall::Edit { fields, .. } if fields.canon.is_some()) {
        return Err(Failure::not_run(fastctx_types::memory::LEGACY));
    }
    if matches!(call, ProjectCall::New { .. } | ProjectCall::Edit { .. }) {
        crate::memory::identity::author(node, caller).await?;
    }
    match call {
        ProjectCall::New { town, fields, account, hub } => new(node, caller, town, fields, account, hub).await,
        ProjectCall::Edit { town, fields } => {
            let town = caller.town_or(node, town)?;
            edit(node, caller, town, fields).await
        }
        ProjectCall::List => {
            let acting = crate::around::acting_of(node, caller);
            crate::ledger::query::list_for(node, &acting).await.map(|towns| Reply::Towns { towns })
        }
        ProjectCall::Show { town } => {
            let town = caller.town_or(node, town)?;
            map::query(node, &town).await?;
            let Some((mut record, opened)) = blocking(node, &town, read::current).await? else {
                return Err(map::no_ledger(node, &town));
            };
            seen(node, caller, &town, &mut record)?;
            Ok(Reply::Record { town, record: Box::new(record), opened })
        }
        ProjectCall::Map { town } => {
            let town = caller.town_or(node, town)?;
            map::query(node, &town).await?;
            let read_only = crate::memory::identity::read_only(node, caller).await?;
            let mut map = map::view_for(node, &town, &crate::around::acting_of(node, caller), read_only).await?;
            seen(node, caller, &town, &mut map.record)?;
            Ok(Reply::Map { map: Box::new(map) })
        }
        ProjectCall::Enter { town } => super::enter::call(node, caller, town).await,
    }
}

/// Opens a town: the first version of its record, in the account it is
/// opened in (`owner`).
async fn new(
    node: &Arc<Node>,
    caller: &Caller,
    town: TownName,
    fields: RecordFields,
    account: Option<AccountName>,
    hub: Option<HubName>,
) -> Result<Reply, Failure> {
    let owner = owner(node, caller, account, hub)?;
    crate::ledger::query(node, Some(&town)).await.map_err(Failure::new)?;
    if blocking(node, &town, read::latest).await?.is_some() {
        return Err(Failure::new(format!(
            "the project {town} exists already; `fastctx project edit {town}` changes it"
        )));
    }
    let name = fields.name.ok_or_else(|| Failure::new("a new project needs its name (--name)"))?;
    let what = fields.what.ok_or_else(|| Failure::new("a new project needs to say what it is (--what)"))?;
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
    let record = checked(node, caller, owner.as_ref().map(|owner| &owner.hub), record).await?;
    crate::ledger::open(node, &town, owner).await?;
    write(node, &town, &record).await.map_err(|failure| {
        if failure.message.contains("already") {
            Failure::new(format!("a project named {town} exists already ({failure})"))
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
    if fields == RecordFields::default() {
        return Err(Failure::new("nothing to change: give what the next version says"));
    }
    map::query(node, &town).await?;
    let Some((latest, opened)) = blocking(node, &town, read::current).await? else {
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
    let hub = crate::ledger::names::place(&node.world, &town).ok().flatten().map(|place| place.hub);
    let record = checked(node, caller, hub.as_ref(), record).await?;
    write(node, &town, &record).await.map_err(|failure| {
        if failure.message.contains("already") {
            Failure::new(format!(
                "another version {} of {town}'s record was written first; see it with `fastctx project show {town}` and edit from there",
                record.version
            ))
        } else {
            failure
        }
    })?;
    Ok(Reply::Record { town, record: Box::new(record), opened })
}

/// The record with its machines checked against those whoever asks may
/// work on, and its paths written the way their machines write them.
async fn checked(
    node: &Arc<Node>,
    caller: &Caller,
    hub: Option<&HubId>,
    mut record: Record,
) -> Result<Record, Failure> {
    if record.machines.is_empty() {
        return Err(Failure::new("a project spans at least one machine"));
    }
    let known = machines(node, &crate::around::acting_of(node, caller), hub).await;
    let view = hub.and_then(|hub| node.link_of(hub)).map(|link| link.view());
    let canonical = |name: &MachineName| match &view {
        Some(view) => view.canonical(name).clone(),
        None if node.is_me(name) => node.machine(),
        None => name.clone(),
    };
    for machine in &mut record.machines {
        *machine = canonical(machine);
    }
    for place in &mut record.places {
        place.location.machine = canonical(&place.location.machine);
    }
    if let Some(canon) = &mut record.canon {
        canon.machine = canonical(&canon.machine);
    }
    let unknown = |machine: &MachineName| !known.contains(machine);
    let here = |machine: &MachineName| view.as_ref().map_or_else(|| node.is_me(machine), |view| view.is_me(machine));
    let named = record
        .machines
        .iter()
        .chain(record.places.iter().map(|place| &place.location.machine))
        .chain(record.canon.iter().map(|canon| &canon.machine));
    if let Some(stranger) = named.into_iter().find(|machine| unknown(machine)) {
        return Err(Failure::new(format!(
            "there is no machine named {stranger}; the machines are {}",
            known.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )));
    }
    for place in &mut record.places {
        if here(&place.location.machine) {
            place.location.path = fastctx_platform::local_path(&place.location.path)
                .map_err(|error| Failure::new(error.to_string()))?
                .to_string_lossy()
                .into_owned();
        }
        place.location.path = absolute(&place.location.path).map_err(Failure::new)?;
        if place.what.trim().is_empty() {
            return Err(Failure::new(format!("the place {} says nothing of what is there", place.location.path)));
        }
    }
    if let Some(canon) = &mut record.canon {
        if here(&canon.machine) {
            canon.path = fastctx_platform::local_path(&canon.path)
                .map_err(|error| Failure::new(error.to_string()))?
                .to_string_lossy()
                .into_owned();
        }
        canon.path = absolute(&canon.path).map_err(Failure::new)?;
    }
    Ok(record)
}

/// The machines of the hub a project is kept at that `acting` may work on,
/// as this machine knows them now: no other is named to it, not even in a
/// refusal (`world-network.md`, 一个居民替谁干活). A device is never one of
/// a project's machines, and a project spans one hub. A project of a
/// computer on no hub spans that computer.
async fn machines(node: &Arc<Node>, acting: &fastctx_types::access::Acting, hub: Option<&HubId>) -> Vec<MachineName> {
    match hub.and_then(|hub| node.link_of(hub)) {
        Some(link) => {
            // Asked afresh, or as last heard when the hub cannot be asked.
            let _ = link.refresh().await;
            let accounts = node.accounts_on(&link, acting);
            let view = link.view();
            view.machines()
                .filter(|member| {
                    (view.is_me(&member.name) && node.works_here(acting).is_ok())
                        || accounts.iter().any(|account| view.rights(account, &member.name).hands)
                })
                .map(|member| member.name.clone())
                .collect()
        }
        None => vec![node.machine()],
    }
}

/// The account a project whoever asks opens is opened in: the one named,
/// of the hub named, when whoever asks works for it, or the one account they
/// work for. None on a computer that is on no hub, which keeps its projects
/// itself.
fn owner(
    node: &Node,
    caller: &Caller,
    named: Option<AccountName>,
    hub: Option<HubName>,
) -> Result<Option<HubAccount>, Failure> {
    let acting = crate::around::acting_of(node, caller);
    let mut accounts = Vec::new();
    for link in
        node.links().into_iter().filter(|link| hub.as_ref().is_none_or(|hub| link.membership().hub_name == *hub))
    {
        let id = link.membership().id.clone();
        for account in node.accounts_on(&link, &acting) {
            if named.as_ref().is_none_or(|named| *named == account) {
                accounts.push(HubAccount { hub: id.clone(), account });
            }
        }
    }
    match named {
        Some(named) => match accounts.len() {
            1 => Ok(accounts.pop()),
            0 => Err(Failure::not_run(format!(
                "this does not work for an account {named} on any hub this computer is on"
            ))),
            _ => Err(Failure::not_run(format!("accounts named {named} are on several hubs; name the hub"))),
        },
        None => match accounts.len() {
            0 if node.connected_anywhere() => {
                Err(Failure::not_run("this works for no account a project could be opened in"))
            }
            0 => Ok(None),
            1 => Ok(accounts.pop()),
            _ => Err(Failure::not_run(format!(
                "this works for several accounts; name the one the project is opened in ({})",
                accounts.iter().map(|account| account.account.to_string()).collect::<Vec<_>>().join(", ")
            ))),
        },
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

/// `record` of `town` as `caller` may know it (`ledger::query::seen`).
fn seen(
    node: &Node,
    caller: &Caller,
    town: &TownName,
    record: &mut fastctx_types::project::Record,
) -> Result<(), Failure> {
    let acting = crate::around::acting_of(node, caller);
    crate::ledger::query::seen(node, &acting, town, record)
        .map_err(|error| Failure::new(format!("reading the project {town}: {error}")))
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
