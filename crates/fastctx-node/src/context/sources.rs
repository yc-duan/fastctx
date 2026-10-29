//! Adapters from the user's settings and project records into named context.

use super::{Change, Receipt, storage, words};
use crate::Node;
use fastctx_types::{Address, ResidentId, TownName, access::Acting, ipc::Failure, project::Writer, residents::Origin};
use std::{io, sync::Arc};

pub(crate) fn refresh_local(node: &Node, id: &ResidentId) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    let (about, town) = node.residents.about(id).ok_or_else(|| io::Error::other(words::UNKNOWN))?;
    local_locked(node, id, town.as_ref(), &about.acting)
}

fn local(node: &Node, id: &ResidentId, town: Option<&TownName>, acting: &Acting) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    local_locked(node, id, town, acting)
}

fn local_locked(node: &Node, id: &ResidentId, town: Option<&TownName>, acting: &Acting) -> io::Result<()> {
    let prompt = crate::profile::session::text(&crate::profile::session::prompt(node, acting));
    storage::set_locked(
        &node.world,
        id,
        "user",
        &prompt,
        Change { by: None, version: "", fact: &words::changed("user", words::USER), full: true },
    )?;
    let (text, author, version, by) = match town {
        Some(town) => match crate::town::instructions::current(node, town, acting)? {
            Some((text, record)) => {
                let version = format!("{town}:{}", record.version);
                let writer = fastctx_hands::town::writer(&record.by, &record.on);
                let by = match record.by {
                    Writer::Resident { address } => Some(address),
                    _ => None,
                };
                (text, by, version, writer)
            }
            None => (String::new(), None, String::new(), words::USER.into()),
        },
        None => (String::new(), None, String::new(), words::USER.into()),
    };
    storage::set_locked(
        &node.world,
        id,
        "project",
        &text,
        Change { by: author.as_ref(), version: &version, fact: &words::changed("project", &by), full: true },
    )?;
    let focus = match std::fs::read_to_string(node.world.conversation(id).focus()) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    storage::set_locked(
        &node.world,
        id,
        "focus",
        &focus,
        Change { by: None, version: "selection", fact: &words::changed("focus", words::USER), full: true },
    )
}

pub(crate) async fn prepare(
    node: &Arc<Node>,
    id: &ResidentId,
    town: Option<&TownName>,
    origin: &Origin,
    acting: &Acting,
) -> Result<(String, Receipt), Failure> {
    let directory = node.world.conversation(id);
    fastctx_platform::ensure_private_dir(directory.path()).map_err(|error| Failure::new(error.to_string()))?;
    if let Some(town) = town {
        crate::town::access::ready(node, acting, town).await?;
    }
    if node.residents.about(id).is_some() {
        crate::focus::refresh(node, id).await?;
    }
    local(node, id, town, acting).map_err(|error| Failure::new(error.to_string()))?;
    plan_guide(node, id, origin).map_err(|error| Failure::new(error.to_string()))?;
    crate::plan::context(node, id, origin, acting).await?;
    map(node, id, town, origin, acting).await?;
    storage::opening(&node.world, id).map_err(|error| Failure::new(error.to_string()))
}

/// How to keep a plan, for a conversation that keeps its own; a colleague
/// only reads the plan of the conversation that asked it, and a session the
/// user opened outside Fastctx keeps none (`world-layers.md`).
fn plan_guide(node: &Node, id: &ResidentId, origin: &Origin) -> io::Result<()> {
    let own = node.residents.about(id).is_some_and(|(about, _)| !about.world_managed());
    let text = if matches!(origin, Origin::Colleague { .. }) || own {
        String::new()
    } else {
        let path = crate::plan_file::path(node, id).to_string_lossy().replace('\\', "/");
        words::plan_guide(&path, node.machine().as_str())
    };
    let change = Change { by: None, version: "", fact: &words::changed("plan-guide", words::USER), full: true };
    if node.residents.about(id).is_some() {
        super::set(node, id, "plan-guide", &text, change)
    } else {
        storage::set(&node.world, id, "plan-guide", &text, change)
    }
}

/// The project's memory index as the "map" block. The project instructions
/// ride the "project" block, so this carries only the journals and decisions
/// the project keeps; it is empty when memory is off or there is no project.
/// What the conversation wrote itself is not told back to it; what another
/// wrote is told as one fact a document, and the whole index goes with the
/// opening, a compaction or a change of project (`world-memory.md`, 地图).
pub(crate) async fn map(
    node: &Arc<Node>,
    id: &ResidentId,
    town: Option<&TownName>,
    origin: &Origin,
    acting: &Acting,
) -> Result<(), Failure> {
    let read_only = matches!(origin, Origin::Colleague { .. });
    let snapshot = match town {
        Some(town) => crate::memory::context::text(node, town, acting, read_only).await?,
        None => None,
    };
    let directory = node.world.conversation(id);
    let before = map_cursor::load(&directory).map_err(|error| Failure::new(error.to_string()))?;
    let after = map_cursor::Cursor::of(town, snapshot.as_ref().map(|(_, catalog)| catalog));
    let me = Address { machine: node.machine(), resident: id.clone() };
    let told = match (&before, &snapshot) {
        (Some(before), Some((_, catalog))) if before.town == after.town => {
            Some(map_cursor::facts(node, before, catalog, id))
        }
        _ => None,
    };
    let fact = match &told {
        Some(facts) if !facts.is_empty() => facts.join("\n"),
        _ => words::changed("map", words::USER),
    };
    // A change with no one else's document in it is this conversation's own.
    let own = told.as_ref().is_some_and(Vec::is_empty);
    let version = serde_json::to_string(&after).map_err(|error| Failure::new(error.to_string()))?;
    let change = Change { by: own.then_some(&me), version: &version, fact: &fact, full: told.is_none() };
    let text = snapshot.as_ref().map(|(text, _)| text.as_str()).unwrap_or("");
    // Opening instructions are prepared before the conversation is born.
    if node.residents.about(id).is_some() {
        super::set(node, id, "map", text, change)
    } else {
        storage::set(&node.world, id, "map", text, change)
    }
    .map_err(|error| Failure::new(error.to_string()))
}

/// Which version of each project document a conversation's map has shown it.
mod map_cursor {
    use crate::Node;
    use fastctx_types::{ResidentId, TownName, layout::ConversationDir, memory::Catalog, project::Writer};
    use serde::{Deserialize, Serialize};
    use std::{collections::BTreeMap, io};

    #[derive(Serialize, Deserialize)]
    pub(super) struct Cursor {
        pub town: Option<TownName>,
        versions: BTreeMap<String, u32>,
    }
    impl Cursor {
        pub fn of(town: Option<&TownName>, catalog: Option<&Catalog>) -> Self {
            let versions = catalog
                .map(|catalog| catalog.journals.iter().chain(&catalog.specs))
                .into_iter()
                .flatten()
                .map(|revision| (revision.name.clone(), revision.version))
                .collect();
            Self { town: town.cloned(), versions }
        }
    }

    fn path(directory: &ConversationDir) -> std::path::PathBuf {
        directory.books().join("map-heard.json")
    }
    pub fn load(directory: &ConversationDir) -> io::Result<Option<Cursor>> {
        match std::fs::read(path(directory)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn keep(directory: &ConversationDir, cursor: &Cursor) -> io::Result<()> {
        fastctx_platform::ensure_private_dir(&directory.books())?;
        fastctx_platform::publish(
            &path(directory),
            &serde_json::to_vec(cursor)?,
            fastctx_platform::Permissions::Private,
        )
    }
    /// One fact for each document someone other than `id` wrote since `before`.
    pub fn facts(node: &Node, before: &Cursor, after: &Catalog, id: &ResidentId) -> Vec<String> {
        after
            .journals
            .iter()
            .chain(&after.specs)
            .filter(|revision| before.versions.get(&revision.name) != Some(&revision.version))
            .filter(|revision| {
                !matches!(&revision.author.stamp.by, Writer::Resident { address }
                    if address.resident == *id && node.is_me(&address.machine))
            })
            .map(fastctx_hands::memory::changed)
            .collect()
    }
}

pub(super) fn observed(node: &Node, id: &ResidentId, receipt: &Receipt) -> io::Result<()> {
    if let Some(version) = receipt.versions.get("map").filter(|version| !version.is_empty()) {
        let cursor = serde_json::from_str(version).map_err(io::Error::other)?;
        map_cursor::keep(&node.world.conversation(id), &cursor)?;
    }
    if let Some(version) = receipt.versions.get("plan").filter(|version| !version.is_empty()) {
        crate::plan::context_seen(node, id, version).map_err(|error| io::Error::other(error.message))?;
    }
    Ok(())
}

pub(crate) fn plan(node: &Node, id: &ResidentId, page: &fastctx_types::plan::Page, version: &str) -> io::Result<()> {
    let by: Option<&Address> = page.body.as_ref().and_then(|body| match &body.author {
        fastctx_types::plan::Author::Resident { address } => Some(address),
        fastctx_types::plan::Author::User => None,
    });
    let who = by.map(|by| by.resident.to_string()).unwrap_or_else(|| words::USER.into());
    let set = |text: &str, change| {
        if node.residents.about(id).is_some() {
            super::set(node, id, "plan", text, change)
        } else {
            storage::set(&node.world, id, "plan", text, change)
        }
    };
    set(
        page.body.as_ref().map(|body| body.markdown.as_str()).unwrap_or_default(),
        Change { by, version, fact: &words::changed("plan", &who), full: true },
    )
}

pub(crate) fn plan_unavailable(node: &Node, id: &ResidentId) -> io::Result<()> {
    storage::set(
        &node.world,
        id,
        "plan",
        "",
        Change { by: None, version: "", fact: words::PLAN_UNAVAILABLE, full: true },
    )
}
