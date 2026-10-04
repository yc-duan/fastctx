//! `fastctx w <id>`: one resident or job in detail, answered by the machine
//! where it is (`world-town.md`, `fastctx w <id>`). Every machine that can
//! be asked is, since an id is only unique on its own machine.

use std::io;
use std::sync::Arc;

use fastctx_hands::run::record;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Action, MessageKind, MessageLine};
use fastctx_types::town::{Looked, LookedUp, ResidentDetail, ResidentNow};
use fastctx_types::{Address, JobId, ResidentId};

use crate::Node;

/// How many of a resident's latest actions the detail shows: twenty lines
/// are about a thousand characters, enough to see what it has been doing;
/// the rest are in its records, which the detail names.
const ACTIONS: usize = 20;
/// How many of a colleague's latest turn ends the detail shows.
const ANSWERS: usize = 5;

pub async fn look(node: &Arc<Node>, ids: Vec<String>) -> Vec<LookedUp> {
    let mut looked = Vec::new();
    for id in ids {
        let mut found = Vec::new();
        let mut unasked = Vec::new();
        if let Some(here) = look_here(node, &id).await {
            found.push(here);
        }
        if let Some(link) = node.link() {
            let refreshed = link.refresh().await;
            let (table, presence) = link.members();
            let me = link.membership().name.clone();
            let mut asks = Vec::new();
            for member in table.members().filter(|member| member.name != me) {
                let online = presence.iter().any(|presence| presence.name == member.name && presence.online);
                if refreshed.is_err() || !online {
                    unasked.push(format!("{} (offline)", member.name));
                    continue;
                }
                let (link, name, id) = (link.clone(), member.name.clone(), id.clone());
                asks.push(tokio::spawn(async move { (name.clone(), link.call(&name, RemoteCall::Look { id }).await) }));
            }
            for ask in asks {
                let Ok((name, answer)) = ask.await else { continue };
                match answer {
                    Ok(Ok(RemoteReply::Looked { looked: Some(there) })) => found.push(there),
                    Ok(Ok(_)) => {}
                    Ok(Err(failure)) => unasked.push(format!("{name} ({failure})")),
                    Err(failure) => unasked.push(format!("{name} ({failure})")),
                }
            }
        }
        looked.push(LookedUp { id, found, unasked });
    }
    looked
}

/// What this machine has under `id`.
pub async fn look_here(node: &Arc<Node>, id: &str) -> Option<Looked> {
    if let Ok(resident) = ResidentId::try_from(id) {
        let record = node.residents.everyone().into_iter().find(|record| record.id == resident)?;
        let (world, here) = (node.world.clone(), node.machine());
        let detail = tokio::task::spawn_blocking(move || -> io::Result<ResidentDetail> {
            let directory = world.resident(record.town.as_ref(), &record.id);
            let mut actions: Vec<Action> = lines(&directory.actions())?;
            let actions_total = actions.len();
            actions.drain(..actions_total.saturating_sub(ACTIONS));
            let messages: Vec<MessageLine> = lines(&directory.messages())?;
            let asked = messages.iter().find_map(|line| match &line.what {
                MessageKind::Asked { text, .. } => Some(text.clone()),
                _ => None,
            });
            let mut answers: Vec<MessageLine> =
                messages.into_iter().filter(|line| matches!(line.what, MessageKind::Ended { .. })).collect();
            answers.drain(..answers.len().saturating_sub(ANSWERS));
            let owner = Address { machine: here, resident: record.id.clone() };
            let mut jobs = Vec::new();
            crate::who::running_jobs(&directory.jobs(), &mut jobs)?;
            jobs.retain(|job| job.owner == owner);
            Ok(ResidentDetail {
                resident: ResidentNow {
                    last_action: actions.last().cloned(),
                    about: record.about,
                    town: record.town,
                    here: record.here,
                    now: record.now,
                    moved: None,
                },
                records: directory.path().to_string_lossy().replace('\\', "/"),
                actions,
                actions_total,
                asked,
                answers,
                jobs,
            })
        });
        return match detail.await {
            Ok(Ok(detail)) => Some(Looked::Resident(Box::new(detail))),
            Ok(Err(error)) => {
                node.log.line(format_args!("looking at {id}: {error}"));
                None
            }
            Err(_) => None,
        };
    }
    let job = JobId::try_from(id).ok()?;
    let world = node.world.clone();
    let found = tokio::task::spawn_blocking(move || -> io::Result<Option<Looked>> {
        for parent in world.resident_parents()? {
            for resident in entries(&parent)? {
                let path = resident.join("jobs").join(format!("{job}.json"));
                if path.exists() {
                    return Ok(Some(Looked::Job { record: Box::new(record::read(&path)?) }));
                }
            }
        }
        Ok(None)
    });
    found.await.ok()?.ok()?
}

fn lines<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> io::Result<Vec<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    text.lines()
        .enumerate()
        .map(|(number, line)| {
            serde_json::from_str(line).map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{} line {}: {error}", path.display(), number + 1))
            })
        })
        .collect()
}

fn entries(directory: &std::path::Path) -> io::Result<Vec<std::path::PathBuf>> {
    match std::fs::read_dir(directory) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
