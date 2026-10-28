//! Session identity is durable; process and MCP connection lifetimes are not.

use std::collections::HashSet;
use std::io;
use std::path::{Component, Path};
use std::sync::Arc;

use fastctx_hosts::claude::exchange::{self, Registration};
use fastctx_platform::{Permissions, publish_new};
use fastctx_types::residents::{About, Host, Origin};
use fastctx_types::{AgentName, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// Creates the home and compatibility alias before a new mod publishes any
/// files. Existing legacy writers keep their path until they have exited.
/// `home` is the login the session runs with, as its environment names it,
/// and `background` whether Claude Code's daemon runs it.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn directory(
    node: &Arc<Node>,
    session: String,
    previous: Option<String>,
    cwd: Option<String>,
    conversation: Option<String>,
    process: Option<fastctx_types::ProcessIdentity>,
    home: Option<String>,
    background: Option<bool>,
) -> Result<String, fastctx_types::ipc::Failure> {
    let _place = match conversation.as_deref() {
        Some(conversation) => Some(node.places.using(conversation).await),
        None => None,
    };
    if let Some(deleted) = node.residents.deleted_session(&session) {
        return Err(fastctx_types::ipc::Failure::new(format!(
            "the Claude session was {deleted}, a conversation deleted here"
        )));
    }
    let started_here = |id: &ResidentId| {
        node.residents.about(id).is_some_and(|(about, _)| about.world_started && about.host.agent.as_str() == "claude")
    };
    // The settings a session the World started was started with name its
    // conversation. Before its sessions stood in their project's directory,
    // a conversation's own directory was where they stood.
    let known = match conversation {
        Some(conversation) => {
            let id = ResidentId::try_from(conversation.as_str())
                .ok()
                .filter(|id| started_here(id))
                .ok_or_else(|| {
                    fastctx_types::ipc::Failure::new(format!(
                        "the Claude session was started for {conversation}, not a Claude conversation the World started here"
                    ))
                })?;
            Some(id)
        }
        None => cwd.as_deref().and_then(|cwd| {
            let cwd = Path::new(cwd);
            let id = ResidentId::try_from(cwd.file_name()?.to_str()?).ok()?;
            (started_here(&id) && crate::homes::same_dir(cwd, node.world.conversation(&id).path())).then_some(id)
        }),
    };
    // A session that names no conversation may be a copy Claude Code started
    // instead of waking one: a wake in flight says so before it pairs.
    if known.is_none() {
        node.claude.copies.settled().await;
        if node.claude.copies.is_copy(&session) {
            node.log.line(format_args!("refused the Claude session {session}: it is a copy the World stopped"));
            return Err(fastctx_types::ipc::Failure::new(
                "the Claude session is a copy Claude Code started instead of waking a running session; the World stopped it",
            ));
        }
    }
    let arrival = crate::adoption::Arrival {
        login: match home {
            Some(home) => crate::adoption::Login::Named(Some(home.into())),
            None => crate::adoption::Login::Unknown,
        },
        own: background == Some(false),
    };
    // A World launch holds births.read until the background CLI returns.
    // Its settings already name its conversation, independently of that ID.
    // Births come before the session's own lock (`fastctx_platform::doing::Rank`).
    let _births = if known.is_none() { Some(node.claude.births.write().await) } else { None };
    let reading = super::reading(node, &session);
    let _reading = reading.lock().await;
    let observing = process.clone();
    let reported = session.clone();
    let reserve = || -> io::Result<String> {
        validate_session(&session)?;
        let source = exchange::sessions(node.world.path()).join(&session);
        // A session resumed once its records were gathered into its
        // conversation's room finds this path an alias of the room
        // (`gather::claude_room`), which is private already.
        let aliased = std::fs::symlink_metadata(&source)
            .is_ok_and(|metadata| fastctx_platform::owned_directory::is_link(&metadata));
        if !aliased {
            fastctx_platform::ensure_private_dir(&source)?;
        }
        let registration = Registration {
            session,
            previous,
            at: None,
            instance: None,
            process,
            inbox_address: None,
            entrypoint: None,
            cwd: cwd.clone(),
            model: None,
            version: None,
        };
        let id = match known {
            Some(id) => {
                crate::gather::claude_room(&node.world, &id, &registration.session)?;
                let path = source.join("resident.json");
                match std::fs::read(&path) {
                    Ok(bytes) if serde_json::from_slice::<ResidentId>(&bytes).map_err(io::Error::other)? != id => {
                        return Err(io::Error::other("the Claude session belongs to another conversation"));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        publish_new(&path, &serde_json::to_vec(&id).map_err(io::Error::other)?, Permissions::Private)?;
                    }
                    Err(error) => return Err(error),
                }
                node.residents.set_session(&node.world, &id, &registration.session, None, None)?;
                id
            }
            None => resident(node, &registration, &arrival)?,
        };
        node.residents.active(&id).map_err(|failure| io::Error::other(failure.message))?;
        crate::context::refresh_local(node, &id)?;
        crate::context::hand_to_session(node, &id, &source)?;
        if registration.process.is_some() {
            fastctx_platform::publish(
                &source.join("session.json"),
                &serde_json::to_vec(&registration).map_err(io::Error::other)?,
                Permissions::Private,
            )?;
            crate::lifecycle::observed(node, &id, true, "session claimed");
        }
        let mut state = node.claude.state.lock().expect("never poisoned");
        if state
            .sessions
            .get(&registration.session)
            .is_none_or(|prior| prior.registration.process != registration.process)
        {
            state.sessions.insert(registration.session.clone(), super::Session { registration, resident: Some(id) });
        }
        Ok(crate::world::native(&std::fs::canonicalize(&source)?))
    };
    let result = reserve()
        .map_err(|error| fastctx_types::ipc::Failure::new(format!("reserving the Claude session directory: {error}")));
    if let Err(failure) = &result {
        node.log.line(format_args!("Claude session {reported}: {}", failure.message));
    }
    if result.is_ok() {
        super::process::observe(node, observing);
        node.claude.changed.notify_waiters();
    }
    result
}

/// Called with the node's births lock held for writing.
/// A clear chain can arrive in any order, including only after a restart.
/// A session that continues no conversation here becomes one only when it
/// is the owner's own (`crate::adoption`), and never when it is a copy.
pub(super) fn resident(
    node: &Node,
    registration: &Registration,
    arrival: &crate::adoption::Arrival,
) -> io::Result<ResidentId> {
    if let Some(deleted) = node.residents.deleted_session(&registration.session) {
        return Err(io::Error::other(format!("it was {deleted}, a conversation deleted here")));
    }
    let root = exchange::sessions(node.world.path());
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut candidates = HashSet::new();
    let mut current = registration.clone();
    loop {
        validate_session(&current.session)?;
        if !seen.insert(current.session.clone()) {
            return Err(io::Error::other("the Claude clear chain contains a cycle"));
        }
        let directory = root.join(&current.session);
        if node.residents.records_for_session(&current.session) > 1 {
            return Err(io::Error::other(format!("more than one resident names Claude session {}", current.session)));
        }
        match std::fs::read(directory.join("resident.json")) {
            Ok(bytes) => {
                let id: ResidentId = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if node.residents.about(&id).is_none_or(|(about, _)| about.host.agent.as_str() != "claude") {
                    return Err(io::Error::other(format!(
                        "{} names an unknown Claude resident {id}",
                        directory.display()
                    )));
                }
                candidates.insert(id);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if let Some(id) = node.residents.resident_of_thread(&current.session) {
            if node.residents.about(&id).is_none_or(|(about, _)| about.host.agent.as_str() != "claude") {
                return Err(io::Error::other("the Claude session id is already assigned to another host"));
            }
            candidates.insert(id);
        }
        if let Some(short) = current.session.get(..8)
            && let Some(id) = node.residents.resident_of_thread(short)
            && node.residents.about(&id).is_some_and(|(about, _)| {
                about.world_started && about.host.agent.as_str() == "claude" && about.session.as_deref() == Some(short)
            })
        {
            if node.residents.records_for_session(short) > 1 {
                return Err(io::Error::other(format!("more than one resident names background id {short}")));
            }
            for entry in std::fs::read_dir(&root)? {
                let entry = entry?;
                let name = entry.file_name();
                if name.to_str().is_some_and(|name| name != current.session && name.starts_with(short))
                    && exchange::registration(&entry.path())?.is_some()
                {
                    return Err(io::Error::other(format!(
                        "more than one Claude session matches background id {short}"
                    )));
                }
            }
            candidates.insert(id);
        }
        // What it continues may be a conversation deleted here: it begins
        // anew then.
        let previous = current
            .previous
            .clone()
            .filter(|previous| previous != &current.session && node.residents.deleted_session(previous).is_none());
        chain.push(current);
        let Some(previous) = previous else { break };
        validate_session(&previous)?;
        current = exchange::registration(&root.join(&previous))?.ok_or_else(|| {
            io::Error::other(format!("the preceding Claude session {previous} has no complete registration"))
        })?;
        if current.session != previous {
            return Err(io::Error::other("a Claude registration does not name its own directory"));
        }
    }
    if candidates.len() > 1 {
        return Err(io::Error::other("the Claude clear chain belongs to more than one resident"));
    }
    let id = match candidates.into_iter().next() {
        Some(id) => id,
        None => {
            if node.claude.copies.is_copy(&registration.session) {
                return Err(io::Error::other(
                    "it is a copy Claude Code started instead of waking a running session; the World stopped it",
                ));
            }
            let home = crate::adoption::owners_own(node, "claude", arrival)?;
            let first = chain.last().expect("the chain includes this registration");
            let agent = AgentName::try_from("claude").expect("a valid agent name");
            let id = node.residents.new_id(&agent).map_err(io::Error::other)?;
            let at = first.at.unwrap_or_else(Timestamp::now);
            let about = About {
                side: false,
                forked_from: None,
                mcp_off: Default::default(),
                id: id.clone(),
                host: Host {
                    agent,
                    client: "claude-code".to_string(),
                    version: first.version_text().unwrap_or_default(),
                },
                machine: node.machine(),
                born_at: at,
                origin: Origin::User,
                session: Some(first.session.clone()),
                world_started: false,
                native_resumed: false,
                model: first.model_name(),
                acting: Default::default(),
                focus: Default::default(),
                home,
                callee: None,
            };
            node.residents.create(&node.world, about, None, crate::residents::idle(at))?;
            id
        }
    };
    let bytes = serde_json::to_vec(&id).map_err(io::Error::other)?;
    for registration in chain.iter().rev() {
        crate::gather::claude_room(&node.world, &id, &registration.session)?;
        let path = root.join(&registration.session).join("resident.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                let held: ResidentId = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if held != id {
                    return Err(io::Error::other(format!("{} already belongs to {held}", registration.session)));
                }
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match publish_new(&path, &bytes, Permissions::Private) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let held: ResidentId = serde_json::from_slice(&std::fs::read(&path)?).map_err(io::Error::other)?;
                if held != id {
                    return Err(io::Error::other(format!("{} already belongs to {held}", registration.session)));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(id)
}

pub(crate) fn validate_session(session: &str) -> io::Result<()> {
    let mut components = Path::new(session).components();
    if session.is_empty()
        || session.starts_with('.')
        || session.contains(['/', '\\', ':'])
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(io::Error::other("a Claude session id is not a single directory name"));
    }
    Ok(())
}

pub(super) fn present(node: &Node, registration: &Registration) -> io::Result<bool> {
    if let Some(instance) = &registration.instance {
        validate_session(instance)?;
        let closed = exchange::sessions(node.world.path()).join(&registration.session).join("closed").join(instance);
        if closed.try_exists()? {
            return Ok(false);
        }
    }
    let path = exchange::sessions(node.world.path()).join(&registration.session).join("node-ended.json");
    if let Some(ended) = fastctx_hosts::claude::control::read::<Registration>(&path)?
        && same_activation(&ended, registration)
    {
        return Ok(false);
    }
    registration
        .process
        .as_ref()
        .map(|process| fastctx_platform::has_ended(process).map(|ended| !ended))
        .transpose()
        .map(|alive| alive.unwrap_or(false))
}

fn same_activation(a: &Registration, b: &Registration) -> bool {
    a.session == b.session && a.instance == b.instance && a.process == b.process && a.at == b.at
}

/// Whether the end of the activation `registration` was settled already.
pub(super) fn ended_already(node: &Node, registration: &Registration) -> io::Result<bool> {
    let path = exchange::sessions(node.world.path()).join(&registration.session).join("node-ended.json");
    Ok(fastctx_hosts::claude::control::read::<Registration>(&path)?
        .is_some_and(|ended| same_activation(&ended, registration)))
}

/// Records that the activation `registration` ended, so that it is settled
/// once: its process may have exited long before, as Claude Code retires an
/// idle background session without telling the World. Older mods can also
/// report session end while their shared host process stays alive.
pub(super) fn record_ended(node: &Node, registration: &Registration) -> io::Result<()> {
    if ended_already(node, registration)? {
        return Ok(());
    }
    fastctx_platform::publish(
        &exchange::sessions(node.world.path()).join(&registration.session).join("node-ended.json"),
        &serde_json::to_vec(registration).map_err(io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
}
