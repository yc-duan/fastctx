//! Adapters from the user's settings and project records into named context.

use super::{Change, Receipt, storage, words};
use crate::Node;
use fastctx_types::{Address, ResidentId, TownName, access::Acting, ipc::Failure, project::Writer, residents::Origin};
use std::{io, sync::Arc};

static SOURCES: std::sync::LazyLock<crate::record_gates::Gates> = std::sync::LazyLock::new(|| {
    crate::record_gates::Gates::new(fastctx_platform::doing::Rank::EMITTING, "local context source")
});

pub(crate) fn refresh_local(node: &Node, id: &ResidentId) -> io::Result<()> {
    let gate = SOURCES.get(node.world.conversation(id).path());
    let _refreshing = gate.lock();
    let (about, town) = node.residents.about(id).ok_or_else(|| io::Error::other(words::UNKNOWN))?;
    local_current(node, id, town.as_ref(), &about.acting)
}

fn local(node: &Node, id: &ResidentId, town: Option<&TownName>, acting: &Acting) -> io::Result<()> {
    let gate = SOURCES.get(node.world.conversation(id).path());
    let _refreshing = gate.lock();
    local_current(node, id, town, acting)
}

fn local_current(node: &Node, id: &ResidentId, town: Option<&TownName>, acting: &Acting) -> io::Result<()> {
    let prompt = crate::profile::session::text(&crate::profile::session::prompt(node, acting));
    storage::set(
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
                let writer = fastctx_hands::town::writer(&record.by, record.on.as_ref());
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
    storage::set(
        &node.world,
        id,
        "project",
        &text,
        Change { by: author.as_ref(), version: &version, fact: &words::changed("project", &by), full: true },
    )?;
    let focus = match fastctx_platform::disk::read_to_string(node.world.conversation(id).focus()) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    storage::set(
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
    fastctx_platform::task::spawn_blocking(move || fastctx_platform::ensure_private_dir(directory.path()))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(crate::record_migration::failure)?;
    if let Some(town) = town {
        crate::town::access::ready(node, acting, town).await?;
    }
    if node.residents.about(id).is_some() {
        crate::focus::refresh(node, id).await?;
    }
    let (local_town, local_acting) = (town.cloned(), acting.clone());
    super::blocking::with(node, id, move |node, id| local(node, id, local_town.as_ref(), &local_acting))
        .await
        .map_err(crate::record_migration::failure)?;
    // Skills are the account's; one whose hub cannot be asked now keeps what
    // it was told, and the conversation goes on without waiting for it.
    if let Err(failure) = skills(node, id, acting).await {
        node.log.line(format_args!("telling {id} its skills: {}", failure.message));
    }
    if let Err(failure) = crate::record_ownership::register(node, id, acting).await {
        node.log.line(format_args!("registering the records of {id}: {}", failure.message));
    }
    plan_guide(node, id, origin, acting).await.map_err(|error| Failure::new(error.to_string()))?;
    crate::plan::context(node, id, origin, acting).await?;
    map(node, id, town, origin, acting).await?;
    super::blocking::with(node, id, |node, id| storage::opening(&node.world, id, &node.context_bodies))
        .await
        .map_err(crate::record_migration::failure)
}

/// The names and sentences of the skills of the account the conversation
/// works for, as the "skills" block (`crate::skills`); kept as it was where
/// the hub cannot be asked now.
pub(crate) async fn skills(node: &Arc<Node>, id: &ResidentId, acting: &Acting) -> Result<(), Failure> {
    let Some((text, version)) = crate::skills::context(node, acting).await else { return Ok(()) };
    super::blocking::set_account(
        node,
        id,
        "skills",
        &text,
        Change { by: None, version: &version, fact: &words::changed("skills", words::USER), full: true },
    )
    .await
    .map_err(crate::record_migration::failure)
}

/// The skills block, as `version` of the account's index has it.
pub(crate) fn set_skills(node: &Node, id: &ResidentId, text: &str, version: &str) -> Result<(), Failure> {
    let change = Change { by: None, version, fact: &words::changed("skills", words::USER), full: true };
    super::set_account(node, id, "skills", text, change).map_err(|error| Failure::new(error.to_string()))
}

/// The single working-copy path for this turn; a colleague reads its
/// requester's account record and never receives a writable plan path.
pub(crate) async fn plan_guide(node: &Arc<Node>, id: &ResidentId, origin: &Origin, acting: &Acting) -> io::Result<()> {
    if node.residents.now(id).is_some_and(|now| now.turn.is_some()) {
        return Ok(());
    }
    let own = node.residents.about(id).is_some_and(|(about, _)| !about.world_managed());
    let (text, fact) = if own {
        (String::new(), String::new())
    } else if matches!(origin, Origin::Colleague { .. }) {
        let placed = crate::placement::location::prepare(node, id, origin, acting).await;
        let failure = placed.as_ref().err().map(|error| error.message.as_str());
        if node.locations.tell(id, failure)
            && let Some(failure) = failure
        {
            node.log.line(format_args!("placing the colleague {id}: {failure}"));
        }
        (String::new(), String::new())
    } else {
        let placed = crate::placement::location::prepare(node, id, origin, acting).await;
        // A failure is logged when it starts or changes, not at every refresh.
        let failure = placed.as_ref().err().map(|error| error.message.as_str());
        if node.locations.tell(id, failure)
            && let Some(failure) = failure
        {
            node.log.line(format_args!("placing the plan of {id}: {failure}"));
        }
        match placed {
            Ok(Some(location)) => {
                let path=crate::placement::plan_path(&location);
                let replicated = crate::plan::file_account(node, id, acting).ok().flatten().and_then(|account| node.link_of(&account.hub)).is_some_and(|link| link.view().shared_records);
                (words::plan_guide(&path,location.machine.as_str(),replicated),format!("The plan file is now on {} at {path}.",location.machine))
            },
            Ok(None) => ("YOUR PLAN\n\nYour plan remains in your account. There is no online working computer for its file now; the app can still read and edit it.".into(),"There is no online working computer for the plan file now.".into()),
            Err(error) => {
                let fact = match error.message.as_str() {
                    crate::placement::location::OLD_COMPUTER => words::PLAN_OLD_COMPUTER,
                    crate::placement::location::OLD_HUB => words::PLAN_OLD_HUB,
                    _ => words::PLAN_NOT_PREPARED,
                };
                (format!("YOUR PLAN\n\nYour plan remains in your account; the app can still read and edit it. {fact}"), fact.to_string())
            }
        }
    };
    super::blocking::with(node, id, move |node, id| {
        let change = Change { by: None, version: "", fact: &fact, full: false };
        if node.residents.about(id).is_some() {
            super::set(node, id, "plan-guide", &text, change)
        } else {
            storage::set(&node.world, id, "plan-guide", &text, change)
        }
    })
    .await
}

/// Refreshes the history block through the shared context delivery path.
pub(crate) async fn map(
    node: &Arc<Node>,
    id: &ResidentId,
    town: Option<&TownName>,
    origin: &Origin,
    acting: &Acting,
) -> Result<(), Failure> {
    crate::history::context::refresh(node, id, town, origin, acting).await
}

pub(super) fn observed(node: &Node, id: &ResidentId, receipt: &Receipt) -> io::Result<()> {
    crate::history::context::observed(node, id, receipt)?;
    if let Some(version) = receipt.versions.get("plan").filter(|version| !version.is_empty()) {
        crate::plan::context_seen(node, id, version).map_err(|error| io::Error::other(error.message))?;
    }
    Ok(())
}

pub(crate) async fn plan(
    node: &Arc<Node>,
    id: &ResidentId,
    page: &fastctx_types::plan::Page,
    version: &str,
) -> io::Result<()> {
    let by: Option<&Address> = page.body.as_ref().and_then(|body| match &body.author {
        fastctx_types::plan::Author::Resident { address } => Some(address),
        fastctx_types::plan::Author::User | fastctx_types::plan::Author::Elsewhere => None,
    });
    let who = by.map(|by| by.resident.to_string()).unwrap_or_else(|| words::USER.into());
    super::blocking::set_account(
        node,
        id,
        "plan",
        page.body.as_ref().map(|body| body.markdown.as_str()).unwrap_or_default(),
        Change { by, version, fact: &words::changed("plan", &who), full: true },
    )
    .await
}

pub(crate) async fn plan_unavailable(node: &Arc<Node>, id: &ResidentId) -> io::Result<()> {
    super::blocking::with(node, id, |node, id| {
        storage::set(
            &node.world,
            id,
            "plan",
            "",
            Change { by: None, version: "", fact: words::PLAN_UNAVAILABLE, full: true },
        )
    })
    .await
}
