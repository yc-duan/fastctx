//! Session identity is durable; process and MCP connection lifetimes are not.

use std::collections::HashSet;
use std::io;
use std::path::{Component, Path};

use fastctx_hosts::claude::exchange::{self, Registration};
use fastctx_platform::{Permissions, publish_new};
use fastctx_types::residents::{About, Host, Origin};
use fastctx_types::{AgentName, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// Called with the node's births lock held for writing.
/// A clear chain can arrive in any order, including only after a restart.
pub(super) fn resident(node: &Node, registration: &Registration) -> io::Result<ResidentId> {
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
        let previous = current.previous.clone().filter(|previous| previous != &current.session);
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
            let first = chain.last().expect("the chain includes this registration");
            let agent = AgentName::try_from("claude").expect("a valid agent name");
            let id = node.residents.new_id(&agent).map_err(io::Error::other)?;
            let at = first.at.unwrap_or_else(Timestamp::now);
            let about = About {
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
                model: first.model_name(),
                acting: Default::default(),
                works_on: None,
            };
            node.residents.create(&node.world, about, None, crate::residents::idle(at))?;
            id
        }
    };
    let bytes = serde_json::to_vec(&id).map_err(io::Error::other)?;
    for registration in chain.iter().rev() {
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
    registration.process.as_ref().map(fastctx_platform::is_alive).transpose().map(|alive| alive.unwrap_or(false))
}

fn same_activation(a: &Registration, b: &Registration) -> bool {
    a.session == b.session && a.instance == b.instance && a.process == b.process && a.at == b.at
}

pub(super) fn record_ended(node: &Node, registration: &Registration) -> io::Result<()> {
    if !present(node, registration)? {
        return Ok(());
    }
    // Older mods can report session end while their shared host process stays alive.
    fastctx_platform::publish(
        &exchange::sessions(node.world.path()).join(&registration.session).join("node-ended.json"),
        &serde_json::to_vec(registration).map_err(io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
}
