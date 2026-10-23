//! Details read on the machine that holds the resident or job.

use std::io;
use std::sync::Arc;

use fastctx_hands::run::record;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::remote::RemoteCall;
use fastctx_types::residents::{Action, MessageKind, MessageLine};
use fastctx_types::town::{CurrentTool, Looked, LookedUp, ResidentDetail, ResidentNow};
use fastctx_types::{Address, JobId, ResidentId};

use crate::Node;
use crate::activity::entries;
use crate::residents::OnRecord;

// Twenty action lines provide the recent working context; the named file
// retains the complete record (`world-town.md`).
const ACTIONS: usize = 20;

/// Each of `ids`, on this machine and on each machine whoever asks
/// (`acting`) may look at.
pub async fn look(node: &Arc<Node>, acting: &Acting, works_here: bool, ids: Vec<String>) -> Vec<LookedUp> {
    let mut looked = Vec::new();
    for id in ids {
        let mut found = Vec::new();
        let mut unasked = Vec::new();
        let mut jobs = Vec::new();
        // This machine answers for itself only to whoever works on it
        // (`around::works_here`), and to no one at all when it is a managed
        // machine (`world-network.md`, 一个居民替谁干活).
        if works_here && !crate::managed::is_managed(node) {
            match look_here(node, &id).await {
                Ok((here, work)) => {
                    found.extend(crate::admission::seen::looked_here(node, acting, here));
                    jobs.extend(work);
                }
                Err(error) => unasked.push(format!("{} ({error})", node.machine())),
            }
        }
        // A command is on one machine alone: found here, no other is asked.
        // A conversation's commands may have run on others, which are.
        let here_alone = found.iter().any(|looked| matches!(looked, Looked::Job { .. }));
        let mut one = LookedUp { id: id.clone(), found, unasked, jobs };
        if !here_alone {
            // Asked as each account whoever asks works for, which a machine
            // shows its own conversations apart.
            let machines = crate::around::reachable_each(node, acting, Purpose::Look);
            let heard = crate::around::ask_each(machines, |_| RemoteCall::Look { id: id.clone() }).await;
            for answers in fastctx_net::around::answers::collect(heard) {
                answers.look(&mut one);
            }
        }
        one.attach_jobs();
        looked.push(one);
    }
    looked
}

pub async fn look_here(
    node: &Arc<Node>,
    id: &str,
) -> Result<(Option<Looked>, Vec<fastctx_types::town::RunningJob>), Failure> {
    let record = id
        .parse::<fastctx_types::ResidentId>()
        .ok()
        .filter(|parsed| parsed.as_str() == id)
        .and_then(|parsed| node.residents.on_record(&parsed));
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
                    return Ok(Some(Looked::Job { source: None, record: Box::new(record::read(&path)?) }));
                }
            }
        }
    }
    if ResidentId::try_from(id).is_ok() {
        let Some(record) = record else { return Ok(None) };
        let directory = world.conversation(&record.id);
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
                source: None,
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
            goal_reports: crate::goals::reports::recent(&directory)?,
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
