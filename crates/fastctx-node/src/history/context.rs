//! History is a host context block; durable receipts contain hashes, never its catalog text.

pub(crate) mod catalog;
pub(crate) mod heard;

use crate::{Node, context::Change};
use fastctx_platform::task;
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

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Cursor {
    scope: String,
    topics: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    versions: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}
impl Cursor {
    fn location_changed(&self, before: Option<&Self>) -> bool {
        before.is_some_and(|before| before.scope == self.scope && before.location != self.location)
    }
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
    if !node.residents.about(id).is_some_and(|(about, _)| super::offered::to(node, &about)) {
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
    let account = crate::plan::load_account(node, id, acting).await?;
    let (catalog, location) = if let Some(account) = &account {
        let Some(link) = node.link_of(&account.hub) else { return Ok(()) };
        if !link.view().shared_records {
            return unavailable(node, id, &me, "HISTORY\nUpdate the hub to use history and copies on every computer.")
                .await;
        }
        let catalog = match &scope {
            Scope::Project { project } => {
                let (world, project) = (node.world.clone(), project.clone());
                let place = task::spawn_blocking(move || crate::ledger::names::place(&world, &project))
                    .await
                    .map_err(failure)?
                    .map_err(failure)?
                    .ok_or_else(|| failure("the history's project identity is unavailable"))?;
                link.shared_history_catalog(&account.account, &place.town).await
            }
            Scope::Conversation { resident, machine } => {
                link.shared_conversation_history_catalog(
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
        let HistoryReply::Catalog { mut catalog } =
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
        let creating = path.clone();
        task::spawn_blocking(move || fastctx_platform::ensure_private_dir(&creating))
            .await
            .map_err(failure)?
            .map_err(failure)?;
        catalog.sort();
        (
            Arc::new(catalog),
            Some(fastctx_types::placement::Location { machine: node.machine(), path: crate::world::native(&path) }),
        )
    } else {
        return Ok(());
    };
    let prepared = node.contexts.history_catalogs.prepare(&catalog);
    let mut catalog = (*catalog).clone();
    {
        let authors = std::cell::RefCell::new(BTreeMap::new());
        let usable = |machine: &fastctx_types::MachineName| {
            *authors
                .borrow_mut()
                .entry(machine.clone())
                .or_insert_with(|| node.authorize_machine(acting, &machine.into()).is_ok())
        };
        for revision in &mut catalog.documents {
            let own = matches!(&revision.author.stamp.by, Writer::Resident { address } if address.resident == *id && node.is_me(&address.machine));
            let writer = revision.author.stamp.by.clone();
            revision.author.stamp.for_reader(&usable);
            if own {
                revision.author.stamp.by = writer;
            }
        }
        node.contexts.routes.remember_authors(id, authors.into_inner().into_keys().collect());
    }
    let scope_id = hash(&serde_json::to_string(&(&scope, &account)).map_err(failure)?);
    let mut previous_location = None;
    let mut text = match &location {
        Some(location) => fastctx_hands::history::context_with_topics(
            &prepared.listing,
            &location.path,
            location.machine.as_str(),
            town.is_some(),
        ),
        None => format!(
            "HISTORY\nThe history remains in the user's account. No eligible computer is online for its files; the app can still read and edit it.\n\n{}",
            prepared.listing
        ),
    };
    if town.is_some()
        && let Some(account) = &account
        && let Some(link) = node.link_of(&account.hub)
        && let Ok(earlier) = link.shared_conversation_history_catalog(&account.account, &owner).await
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
            previous_location = Some(location);
        }
    }
    let cursor = Cursor {
        scope: scope_id,
        topics: prepared.topics.clone(),
        versions: prepared.versions.clone(),
        location: Some(hash(&serde_json::to_string(&(&location, &previous_location)).map_err(failure)?)),
    };
    let directory = node.world.conversation(id);
    let (before, previous) =
        task::spawn_blocking(move || heard::read(&directory)).await.map_err(failure)?.map_err(failure)?;
    let changed = changes(before.as_ref(), &cursor, &catalog, &prepared.names, id, |machine| node.is_me(machine));
    // A path can change back while the receipt for the intervening path is in flight.
    let moved = cursor.location_changed(before.as_ref()) || cursor.location_changed(previous.as_ref());
    let notice = announcement(changed, moved, location.as_ref());
    let version = serde_json::to_string(&cursor).map_err(failure)?;
    crate::context::blocking::set_account(
        node,
        id,
        "history",
        &text,
        Change { by: notice.own.then_some(&me), version: &version, fact: &notice.fact, full: notice.full },
    )
    .await
    .map_err(failure)
}

struct Announcement {
    fact: String,
    own: bool,
    full: bool,
}
fn announcement(
    changed: Option<Vec<String>>,
    location_changed: bool,
    location: Option<&fastctx_types::placement::Location>,
) -> Announcement {
    let full = changed.is_none() || location_changed;
    let own = !location_changed && changed.as_ref().is_some_and(Vec::is_empty);
    let mut facts = changed.unwrap_or_default();
    if location_changed {
        facts.push(match location {
            Some(location) => format!("This history is in {} on {}.", location.path, location.machine),
            None => {
                "No eligible computer is online for this history's files; the history remains in the user's account."
                    .into()
            }
        });
    }
    let fact =
        if facts.is_empty() { "The history available to this conversation changed.".into() } else { facts.join("\n") };
    Announcement { fact, own, full }
}

fn changes(
    before: Option<&Cursor>,
    cursor: &Cursor,
    catalog: &fastctx_types::history::Catalog,
    names: &[String],
    id: &ResidentId,
    is_me: impl Fn(&fastctx_types::MachineName) -> bool,
) -> Option<Vec<String>> {
    before.filter(|before| before.scope == cursor.scope).map(|before| {
        catalog.documents.iter().zip(names)
            .filter(|(_, name)| {
                before.topics.get(*name) != cursor.topics.get(*name)
                    || before.versions.get(*name).is_some_and(|version| Some(version) != cursor.versions.get(*name))
            })
            .map(|(revision, _)| revision)
            .filter(|revision| !matches!(&revision.author.stamp.by, Writer::Resident { address } if address.resident == *id && is_me(&address.machine)))
            .map(fastctx_hands::history::changed).collect()
    })
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
        let cursor = heard::decode(version.as_bytes())?;
        let revision = receipt
            .revision("history")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "history receipt has no revision"))?;
        heard::keep(&node.world.conversation(id), &cursor, revision)?;
    }
    Ok(())
}
