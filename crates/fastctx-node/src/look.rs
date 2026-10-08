//! Details read on the machine that holds the resident or job.

use std::io;
use std::sync::Arc;

use fastctx_hands::run::record;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Action, MessageKind, MessageLine};
use fastctx_types::town::{CurrentTool, Looked, LookedUp, ResidentDetail, ResidentNow};
use fastctx_types::{Address, JobId, ResidentId};

use crate::Node;
use crate::activity::entries;
use crate::residents::OnRecord;

// Twenty action lines provide the recent working context; the named file
// retains the complete record (`world-town.md`).
const ACTIONS: usize = 20;

pub async fn look(node: &Arc<Node>, ids: Vec<String>) -> Vec<LookedUp> {
    let mut looked = Vec::new();
    for id in ids {
        let mut found = Vec::new();
        let mut unasked = Vec::new();
        let mut jobs = Vec::new();
        match look_here(node, &id).await {
            Ok((here, work)) => {
                found.extend(here);
                jobs.extend(work);
            }
            Err(error) => unasked.push(format!("{} ({error})", node.machine())),
        }
        if let Some(link) = node.link() {
            let refreshed = link.refresh().await;
            let (table, presence) = link.members();
            let me = link.membership().name.clone();
            let mut asks = Vec::new();
            for member in table
                .machines()
                .filter(|member| member.name != me && crate::admission::reaches(node, &table, &member.name))
            {
                if let Err(error) = &refreshed {
                    unasked.push(format!("{} (presence unknown: {error})", member.name));
                    continue;
                }
                match presence.iter().find(|seen| seen.name == member.name) {
                    Some(seen) if seen.online => {}
                    Some(_) => {
                        unasked.push(format!("{} (offline)", member.name));
                        continue;
                    }
                    None => {
                        unasked.push(format!("{} (presence unknown)", member.name));
                        continue;
                    }
                }
                let (link, name, id) = (link.clone(), member.name.clone(), id.clone());
                asks.push((name.clone(), tokio::spawn(async move { link.call(&name, RemoteCall::Look { id }).await })));
            }
            for (name, ask) in asks {
                match ask.await {
                    Ok(Ok(Ok(RemoteReply::Looked { looked, jobs: work }))) => {
                        found.extend(looked);
                        jobs.extend(work);
                    }
                    Ok(Ok(Ok(_))) => unasked.push(format!("{name} (answered something else)")),
                    Ok(Ok(Err(error))) => unasked.push(format!("{name} ({error})")),
                    Ok(Err(error)) => unasked.push(format!("{name} ({error})")),
                    Err(error) => unasked.push(format!("{name} (asking: {error})")),
                }
            }
        }
        let mut one = LookedUp { id, found, unasked, jobs };
        one.attach_jobs();
        looked.push(one);
    }
    looked
}

pub async fn look_here(
    node: &Arc<Node>,
    id: &str,
) -> Result<(Option<Looked>, Vec<fastctx_types::town::RunningJob>), Failure> {
    let record = node.residents.everyone().into_iter().find(|record| record.id.as_str() == id);
    let tools = record.as_ref().map(|r| node.watchers.tools(&r.id)).unwrap_or_default();
    let (world, id) = (node.world.clone(), id.to_string());
    tokio::task::spawn_blocking(move || {
        Ok::<_, io::Error>((read_here(&world, &id, record.as_ref(), tools)?, jobs_for(&world, &id)?))
    })
    .await
    .map_err(|error| Failure::new(format!("reading details: {error}")))?
    .map_err(|error| Failure::new(format!("reading details: {error}")))
}

pub(crate) fn read_here(
    world: &WorldDir,
    id: &str,
    record: Option<&OnRecord>,
    tools: Vec<CurrentTool>,
) -> io::Result<Option<Looked>> {
    // Hex-only job suffixes also satisfy the legacy resident-id grammar.
    // Resolve the typed job id before trying a resident with that spelling.
    if let Ok(job) = JobId::try_from(id) {
        for parent in world.job_parents()? {
            for resident in entries(&parent)? {
                let path = resident.join("jobs").join(format!("{job}.json"));
                if path.exists() {
                    return Ok(Some(Looked::Job { record: Box::new(record::read(&path)?) }));
                }
            }
        }
    }
    if ResidentId::try_from(id).is_ok() {
        let Some(record) = record else { return Ok(None) };
        let directory = world.resident(record.town.as_ref(), &record.id);
        let mut actions: Vec<Action> = lines(&directory.actions())?;
        let actions_total = actions.len();
        actions.drain(..actions_total.saturating_sub(ACTIONS));
        let messages: Vec<MessageLine> = lines(&directory.messages())?;
        let asked = messages.iter().find_map(|line| match &line.what {
            MessageKind::Asked { text, .. } => Some(text.clone()),
            _ => None,
        });
        let answers = messages.into_iter().filter(|line| matches!(line.what, MessageKind::Ended { .. })).collect();
        let owner = Address { machine: record.about.machine.clone(), resident: record.id.clone() };
        let mut jobs = Vec::new();
        for parent in world.job_parents()? {
            for directory in entries(&parent)? {
                crate::who::running_jobs(&directory.join("jobs"), &mut jobs)?;
            }
        }
        jobs.retain(|job| job.owner == owner);
        jobs.sort_by_key(|job| job.started_at);
        Ok(Some(Looked::Resident(Box::new(ResidentDetail {
            resident: ResidentNow {
                last_action: actions.last().cloned(),
                about: record.about.clone(),
                town: record.town.clone(),
                here: record.here,
                now: record.now.clone(),
                tools,
                moved: None,
                removed: record.removed,
            },
            records: directory.path().to_string_lossy().into_owned(),
            actions,
            actions_total,
            asked,
            answers,
            jobs,
        }))))
    } else {
        Ok(None)
    }
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

pub(crate) fn jobs_for(world: &WorldDir, id: &str) -> io::Result<Vec<fastctx_types::town::RunningJob>> {
    if JobId::try_from(id).is_ok() || ResidentId::try_from(id).is_err() {
        return Ok(Vec::new());
    }
    let mut jobs = Vec::new();
    for parent in world.job_parents()? {
        for directory in entries(&parent)? {
            crate::who::running_jobs(&directory.join("jobs"), &mut jobs)?;
        }
    }
    jobs.retain(|job| job.owner.resident().is_some_and(|owner| owner.resident.as_str() == id));
    Ok(jobs)
}
