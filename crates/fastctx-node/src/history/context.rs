//! History is a host context block; durable receipts contain hashes, never its catalog text.

use crate::{Node, context::Change};
use fastctx_types::{
    Address, ResidentId, TownName,
    access::Acting,
    history::{HistoryCall, HistoryReply, Scope},
    ipc::Failure,
    project::Writer,
    residents::Origin,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io, sync::Arc};

#[derive(Serialize, Deserialize)]
struct Cursor {
    scope: String,
    topics: BTreeMap<String, String>,
}
fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) async fn refresh(
    node: &Arc<Node>,
    id: &ResidentId,
    town: Option<&TownName>,
    origin: &Origin,
    acting: &Acting,
) -> Result<(), Failure> {
    let me = Address { machine: node.machine(), resident: id.clone() };
    crate::context::blocking::set_account(
        node,
        id,
        "map",
        "",
        Change { by: Some(&me), version: "", fact: "", full: false },
    )
    .await
    .map_err(failure)?;
    if !fastctx_platform::preview() {
        return crate::context::blocking::set_account(
            node,
            id,
            "history",
            "",
            Change { by: Some(&me), version: "", fact: "", full: false },
        )
        .await
        .map_err(failure);
    }
    let owner = match origin {
        Origin::User => me.clone(),
        Origin::Colleague { asked_by, .. } => asked_by.clone(),
    };
    let scope = match town {
        Some(project) => Scope::Project { project: project.clone() },
        None => Scope::Conversation { resident: owner.resident.clone(), machine: owner.machine.clone() },
    };
    let account = crate::plan::file_account(node, id, acting)?;
    let (mut catalog, location) = if let Some(account) = &account {
        let Some(link) = node.link_of(&account.hub) else { return Ok(()) };
        if !link.view().shared_records {
            return unavailable(node, id, &me, "HISTORY\nUpdate the hub to use history and copies on every computer.")
                .await;
        }
        let catalog = match &scope {
            Scope::Project { project } => {
                let place = crate::ledger::names::place(&node.world, project)
                    .map_err(failure)?
                    .ok_or_else(|| failure("the history's project identity is unavailable"))?;
                link.history_catalog(&account.account, &place.town).await
            }
            Scope::Conversation { resident, machine } => {
                link.conversation_history_catalog(
                    &account.account,
                    &Address { resident: resident.clone(), machine: machine.clone() },
                )
                .await
            }
        };
        let catalog = match catalog {
            Ok(catalog) => catalog,
            Err(error) => {
                unavailable(
                    node,
                    id,
                    &me,
                    "HISTORY\nThe history is unavailable while its account access cannot be confirmed.",
                )
                .await?;
                return Err(failure(error));
            }
        };
        let location = crate::replicas::location::prepare(
            node,
            id,
            origin,
            acting,
            account,
            fastctx_types::placement::Resource::History { scope: scope.clone() },
        )
        .await?;
        (catalog, location)
    } else if node.links().is_empty() {
        let HistoryReply::Catalog { catalog } =
            super::local::call(node, HistoryCall::List { scope: scope.clone() }, None).await?
        else {
            return Err(failure("the local history catalog was not returned"));
        };
        let path = match &scope {
            Scope::Project { project } => node.world.town(project).path().join("history"),
            Scope::Conversation { resident, machine } => node
                .world
                .owner(&node.names(), &Address { resident: resident.clone(), machine: machine.clone() })
                .path()
                .join("history"),
        };
        fastctx_platform::ensure_private_dir(&path).map_err(failure)?;
        (
            catalog,
            Some(fastctx_types::placement::Location { machine: node.machine(), path: crate::world::native(&path) }),
        )
    } else {
        return Ok(());
    };
    catalog.sort();
    let usable = |machine: &fastctx_types::MachineName| node.authorize_machine(acting, &machine.into()).is_ok();
    for revision in &mut catalog.documents {
        let own = matches!(&revision.author.stamp.by, Writer::Resident { address } if address.resident == *id && node.is_me(&address.machine));
        let writer = revision.author.stamp.by.clone();
        revision.author.stamp.for_reader(&usable);
        if own {
            revision.author.stamp.by = writer;
        }
    }
    let scope_id = hash(&serde_json::to_string(&(&scope, &account)).map_err(failure)?);
    let cursor = Cursor {
        scope: scope_id,
        topics: catalog
            .documents
            .iter()
            .map(|revision| (hash(&revision.name), hash(&format!("{}:{}", revision.retired, revision.description))))
            .collect(),
    };
    let path = node.world.conversation(id).books().join("history-heard.json");
    let before: Option<Cursor> = match std::fs::read(&path) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(failure)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(failure(error)),
    };
    let changed = before.as_ref().filter(|before| before.scope == cursor.scope).map(|before| {
        catalog.documents.iter().filter(|revision| before.topics.get(&hash(&revision.name)) != cursor.topics.get(&hash(&revision.name)))
            .filter(|revision| !matches!(&revision.author.stamp.by, Writer::Resident { address } if address.resident == *id && node.is_me(&address.machine)))
            .map(fastctx_hands::history::changed).collect::<Vec<_>>()
    });
    let mut text = match location {
        Some(location) => fastctx_hands::history::context(&catalog, &location.path, location.machine.as_str(), town.is_some(), matches!(origin, Origin::Colleague { .. })),
        None => "HISTORY\nThe history remains in the user's account. No eligible computer is online for its files; the app can still read and edit it.".into(),
    };
    if town.is_some()
        && let Some(account) = &account
        && let Some(link) = node.link_of(&account.hub)
        && let Ok(earlier) = link.conversation_history_catalog(&account.account, &owner).await
        && !earlier.documents.is_empty()
    {
        let previous = Scope::Conversation { resident: owner.resident.clone(), machine: owner.machine.clone() };
        if let Some(location) = crate::replicas::location::prepare(
            node,
            id,
            origin,
            acting,
            account,
            fastctx_types::placement::Resource::History { scope: previous },
        )
        .await?
        {
            text.push_str(&format!(
                "\n\nThis conversation's earlier history is in {} on {}.",
                location.path, location.machine
            ));
        }
    }
    let fact = changed
        .as_ref()
        .filter(|facts| !facts.is_empty())
        .map(|facts| facts.join("\n"))
        .unwrap_or_else(|| "The history available to this conversation changed.".into());
    let own = changed.as_ref().is_some_and(Vec::is_empty);
    let version = serde_json::to_string(&cursor).map_err(failure)?;
    crate::context::blocking::set_account(
        node,
        id,
        "history",
        &text,
        Change { by: own.then_some(&me), version: &version, fact: &fact, full: changed.is_none() },
    )
    .await
    .map_err(failure)
}

async fn unavailable(node: &Arc<Node>, id: &ResidentId, me: &Address, text: &str) -> Result<(), Failure> {
    crate::context::blocking::set_account(
        node,
        id,
        "history",
        text,
        Change { by: Some(me), version: "", fact: "The history is unavailable.", full: true },
    )
    .await
    .map_err(failure)
}

pub(crate) fn observed(node: &Node, id: &ResidentId, receipt: &crate::context::Receipt) -> io::Result<()> {
    if let Some(version) = receipt.version("history").filter(|version| !version.is_empty()) {
        let cursor: Cursor = serde_json::from_str(version).map_err(io::Error::other)?;
        let directory = node.world.conversation(id).books();
        fastctx_platform::ensure_private_dir(&directory)?;
        fastctx_platform::publish(
            &directory.join("history-heard.json"),
            &serde_json::to_vec(&cursor)?,
            fastctx_platform::Permissions::Private,
        )?;
    }
    Ok(())
}
