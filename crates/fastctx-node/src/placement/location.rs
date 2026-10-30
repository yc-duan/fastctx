//! The host chooses one place between turns. The account record is the
//! authority; a local receipt pins the coordinates already told this turn.

use crate::Node;
use fastctx_types::{
    Address, MachineName, ResidentId,
    access::{Acting, HubAccount, Purpose},
    account_records::{Request, Response},
    ipc::Failure,
    placement::{KIND, Location, Place},
    remote::{RemoteCall, RemoteReply},
    residents::Origin,
};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct Locations(std::sync::Mutex<std::collections::HashMap<ResidentId, String>>);
impl Locations {
    pub(crate) fn invalidate(&self, id: &ResidentId) {
        self.0.lock().expect("never poisoned").remove(id);
    }
}

pub(crate) fn speaking(node: &Node, id: &ResidentId, machine: &MachineName) -> Result<(), Failure> {
    let room = node.world.conversation(id);
    fastctx_platform::ensure_private_dir(room.path()).map_err(failure)?;
    let path = room.path().join("working-from.json");
    fastctx_platform::publish(
        &path,
        &serde_json::to_vec(machine).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)
}

fn read<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<Option<T>, Failure> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(error)),
    }
}
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) async fn read_place(
    link: &fastctx_net::link::Link,
    scope: &HubAccount,
    owner: &Address,
) -> Result<(Option<u64>, Option<Place>), Failure> {
    let Response::Read { record } = link
        .account_records(&scope.account, Request::Read { kind: KIND.into(), key: owner.resident.to_string() })
        .await
        .map_err(failure)?
    else {
        return Err(failure("the hub did not return working-copy placement"));
    };
    let Some(record) = record else { return Ok((None, None)) };
    if record.metadata.deleted {
        return Ok((Some(record.metadata.version), None));
    }
    let place: Place = serde_json::from_value(record.body).map_err(failure)?;
    if place.owner.resident != owner.resident || !link.view().same(&place.owner.machine, &owner.machine) {
        return Err(failure("another conversation already owns this working-copy record"));
    }
    Ok((Some(record.metadata.version), Some(place)))
}

/// A colleague starts with the requester's working computer as its speaking
/// location. It still selects and authorizes a place independently thereafter.
pub(crate) async fn inherit(
    node: &Arc<Node>,
    id: &ResidentId,
    acting: &Acting,
    requester: &Address,
) -> Result<(), Failure> {
    let Some(scope) = super::account(node, acting, None)? else {
        return Ok(());
    };
    let link = node.link_of(&scope.hub).ok_or_else(|| failure("the colleague's account hub is unavailable"))?;
    if !link.view().plan_copies {
        return Ok(());
    }
    if let Some(machine) = read_place(&link, &scope, requester).await?.1.and_then(|place| place.machine) {
        speaking(node, id, &machine)?;
    }
    Ok(())
}

pub(crate) async fn prepare(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    acting: &Acting,
) -> Result<Option<Location>, Failure> {
    let room = node.world.conversation(id);
    let receipt = room.path().join("working-place.json");
    if node.residents.now(id).is_some_and(|now| now.turn.is_some()) {
        return Ok(read(&receipt)?.flatten());
    }
    let Some(scope) = crate::plan::file_account(node, id, acting)? else {
        if node.links().is_empty() && matches!(acting, Acting::Here) {
            return Ok(Some(Location { machine: node.machine(), path: crate::world::native(room.path()) }));
        }
        return Ok(None);
    };
    let link = node.link_of(&scope.hub).ok_or_else(|| failure("the plan's hub is unavailable"))?;
    if !link.view().plan_copies {
        if crate::plan_file::allowed(node, id)? {
            crate::plan_file::request(node, id);
            return Ok(Some(Location { machine: node.machine(), path: crate::world::native(room.path()) }));
        }
        return Err(failure("update the hub to place and synchronize this conversation's plan"));
    }
    let choices = super::destinations(node, &Acting::one(scope.hub.clone(), scope.account.clone()))?;
    let focus = match node.residents.about(id) {
        Some((about, _)) => about.focus,
        None => crate::focus::opening(&room)?.map(|opening| opening.focus).unwrap_or_default(),
    };
    let from: Option<MachineName> = read(&room.path().join("working-from.json"))?;
    // Earlier own conversations did not record the speaking device. Their
    // own computer remains the natural second choice, only if actually owned.
    let local = node.machine();
    let selected = super::select(&choices, &focus, from.as_ref().or(Some(&local)), None)?.cloned();
    let owner = Address { machine: node.machine(), resident: id.clone() };
    let mut desired =
        Place { owner: owner.clone(), machine: selected.as_ref().map(|choice| choice.machine.name.clone()) };
    let signature = serde_json::to_string(&(&scope, &desired)).map_err(failure)?;
    let known = node.locations.0.lock().expect("never poisoned").get(id) == Some(&signature);
    if known && let Some(location) = read::<Option<Location>>(&receipt)? {
        return Ok(location);
    }
    if let Some(selected) = &selected
        && !node.is_here(&selected.machine)
    {
        let result = link
            .call(&selected.machine.name, &scope.account, Purpose::Look, RemoteCall::Facts)
            .await
            .map_err(failure)??;
        if !matches!(result, RemoteReply::Facts(facts) if facts.plan_copies) {
            return Err(failure("update the selected working computer to synchronize plan copies"));
        }
    }
    let mut placement = None;
    for _ in 0..8 {
        let (version, current) = read_place(&link, &scope, &owner).await?;
        if let Some(current) = &current {
            desired.owner = current.owner.clone();
        }
        if current.as_ref() == Some(&desired) {
            placement = version;
            break;
        }
        match link
            .account_records(
                &scope.account,
                Request::Write {
                    kind: KIND.into(),
                    key: id.to_string(),
                    expected: version,
                    body: serde_json::to_value(&desired).map_err(failure)?,
                },
            )
            .await
            .map_err(failure)?
        {
            Response::Written { metadata } => {
                placement = Some(metadata.version);
                break;
            }
            Response::Conflict { .. } => {}
            _ => return Err(failure("the hub did not record the working-copy location")),
        }
    }
    let placement = placement.ok_or_else(|| failure("the working-copy location keeps changing"))?;
    let owner = desired.owner.clone();
    let resource = if matches!(origin, Origin::User) {
        fastctx_types::placement::Resource::Plan
    } else {
        fastctx_types::placement::Resource::Directory
    };
    let location = if let Some(selected) = selected {
        if node.is_here(&selected.machine) {
            let asker = crate::admission::Asker {
                machine: node.machine(),
                hub: scope.hub.clone(),
                account: scope.account.clone(),
                hands: true,
            };
            Some(super::prepare_copy(node, &asker, owner, placement, resource.clone()).await?)
        } else {
            match link
                .call(
                    &selected.machine.name,
                    &scope.account,
                    Purpose::Hands,
                    RemoteCall::WorkingCopy { owner, placement, resource: resource.clone() },
                )
                .await
                .map_err(failure)??
            {
                RemoteReply::WorkingCopy { location } => Some(location),
                _ => return Err(failure("the working computer did not return its plan path")),
            }
        }
    } else {
        None
    };
    fastctx_platform::publish(
        &receipt,
        &serde_json::to_vec(&location).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)?;
    node.locations.0.lock().expect("never poisoned").insert(id.clone(), signature);
    Ok(location)
}
