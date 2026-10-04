//! Claude sessions, as the node hears them through the World's plugin
//! (`world-colleagues.md`, 话怎样送到居民眼前).
//!
//! The node writes the plugin into the World's directory when it starts.
//! Every session the plugin runs in has a directory under `<world>/claude`,
//! which the node watches. A session's registration pairs it with its
//! resident: the one the World started under its id, the one the session
//! it continues after a clear was, or a new resident for this session.
//! MCP calls identify it by the call id its mod published before execution.
//! What it reports is read once it is paired, in the order it was written,
//! and goes to that resident; what waited for the resident goes to its mod.
//! What the World has for a paired session goes into its mod's inbox, and
//! the mod says which turn took it; a background session the World started
//! whose process the supervisor stopped is woken with it, and its next turn
//! takes it.
//!
//! Built against the mod API as measured on Claude Code 2.1.287 and run
//! against a stand-in host only; it has not yet run with Claude Code.

pub(crate) mod calls;
mod pairing;
mod process;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_hands::told;
use fastctx_hosts::claude::{self, background, exchange};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ResidentDir;
use fastctx_types::residents::{About, Activity, Delivery, Host, Landed, Notice, Now, Origin, Pending, PendingState};
use fastctx_types::session::{Happened, Speaker};
use fastctx_types::{AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;
use tokio::sync::mpsc;

use crate::Node;

#[derive(Default)]
pub struct Claude {
    state: Mutex<State>,
    calls: Mutex<()>,
    /// Held to read while the World starts a session and learns its id;
    /// to write, while a mod registration is assigned to a resident.
    pub births: tokio::sync::RwLock<()>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Session>,
    /// Resident to the session it is.
    paired: HashMap<ResidentId, String>,
    observed: HashSet<fastctx_types::ProcessIdentity>,
    delivering: HashMap<ResidentId, Arc<tokio::sync::Mutex<()>>>,
}

struct Session {
    registration: exchange::Registration,
    resident: Option<ResidentId>,
    reading: Arc<tokio::sync::Mutex<()>>,
}

/// Writes the plugin and starts watching the sessions.
pub fn start(node: &Arc<Node>) -> std::io::Result<()> {
    calls::recover(node);
    let plugin = claude::write_plugin(node.world.path(), &node.fastctx)?;
    let checking = node.clone();
    tokio::task::spawn_blocking(move || check_plugin(&checking, &plugin));
    let sessions = exchange::sessions(node.world.path());
    fastctx_platform::ensure_private_dir(&sessions)?;
    let (changed, mut changes) = mpsc::unbounded_channel::<PathBuf>();
    let root = sessions.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        match event {
            Ok(event) => {
                for path in event.paths {
                    // The session's directory: the first name under the root.
                    if let Ok(rest) = path.strip_prefix(&root)
                        && let Some(first) = rest.components().next()
                    {
                        let _ = changed.send(root.join(first));
                    }
                }
            }
            // Something was missed, as when the system's queue overflowed:
            // every session is looked at again.
            Err(_) => {
                let _ = changed.send(root.clone());
            }
        }
    })
    .map_err(std::io::Error::other)?;
    notify::Watcher::watch(&mut watcher, &sessions, notify::RecursiveMode::Recursive).map_err(std::io::Error::other)?;
    let node = node.clone();
    tokio::spawn(async move {
        // Kept for as long as the node watches.
        let _watcher = watcher;
        look_all(&node, &sessions).await;
        while let Some(first) = changes.recv().await {
            let mut directories = HashSet::from([first]);
            while let Ok(more) = changes.try_recv() {
                directories.insert(more);
            }
            for directory in directories {
                if directory == sessions {
                    look_all(&node, &sessions).await;
                } else {
                    look(&node, &directory).await;
                }
            }
        }
    });
    Ok(())
}

pub async fn replay(node: &Arc<Node>) {
    let directory = exchange::sessions(node.world.path());
    if directory.exists() {
        look_all(node, &directory).await;
    }
}

async fn look_all(node: &Arc<Node>, sessions: &Path) {
    let directories: Vec<PathBuf> = match std::fs::read_dir(sessions) {
        Ok(entries) => entries.filter_map(|entry| Some(entry.ok()?.path())).filter(|path| path.is_dir()).collect(),
        Err(error) => {
            node.log.line(format_args!("looking at the Claude sessions: {error}"));
            return;
        }
    };
    for directory in directories {
        look(node, &directory).await;
    }
}

/// Takes what is new in one session's directory.
async fn look(node: &Arc<Node>, directory: &Path) {
    let Some(session) = directory.file_name().and_then(|name| name.to_str()).map(str::to_string) else {
        return;
    };
    match exchange::registration(directory) {
        Ok(Some(registration)) if registration.session == session => hear(node, registration).await,
        Ok(Some(_)) => {
            node.log.line(format_args!("{} contains a registration for a different session", directory.display()));
            return;
        }
        Ok(None) => return,
        Err(error) => {
            node.log.line(format_args!("reading {}: {error}", directory.display()));
            return;
        }
    }
    // What it reports stays where it is until it is someone's: a node that
    // stops before it knows whose loses none of it.
    let Some(resident) = node
        .claude
        .state
        .lock()
        .expect("never poisoned")
        .sessions
        .get(&session)
        .and_then(|entry| entry.resident.clone())
    else {
        return;
    };
    let reading = node.claude.state.lock().expect("never poisoned").sessions[&session].reading.clone();
    let _reading = reading.lock().await;
    let harvested = {
        let directory = directory.to_path_buf();
        tokio::task::spawn_blocking(move || exchange::harvest(&directory)).await
    };
    let (reported, problems) = match harvested {
        Ok(Ok(harvested)) => harvested,
        Ok(Err(error)) => {
            node.log.line(format_args!("reading what {session} reported: {error}"));
            return;
        }
        Err(_) => return,
    };
    for problem in problems {
        node.log.line(problem);
    }
    for report in reported {
        let applied = async {
            for reported in report.events {
                match reported {
                    exchange::Reported::Uncertain { ids, text } => {
                        let speaking = node.residents.speaking(&resident).expect("known resident");
                        let _speaking = speaking.lock().await;
                        uncertain_inputs(node, &resident, &session, Some(&ids), text).await?;
                    }
                    exchange::Reported::Happened(event) => {
                        if let Happened::TurnStarted { turn } = &event.happened {
                            taken(node, &resident, &session, turn, |pending| pending.state == PendingState::Waking)
                                .await?;
                        }
                        let current = node
                            .residents
                            .about(&resident)
                            .is_some_and(|(about, _)| about.session.as_deref() == Some(&session));
                        if !current && matches!(event.happened, Happened::TurnStarted { .. } | Happened::Health { .. })
                        {
                            let town = node.residents.town_of(&resident).flatten();
                            let directory = node.world.resident(town.as_ref(), &resident);
                            let key = match &event.happened {
                                Happened::TurnStarted { turn } => format!("started:{turn}"),
                                _ => serde_json::to_string(&event).map_err(std::io::Error::other)?,
                            };
                            if crate::delivery::event(&directory, &key, &event)? {
                                node.watchers.publish(&resident, &event);
                            }
                        } else {
                            crate::turns::apply(node, &resident, event).await?;
                        }
                    }
                    exchange::Reported::Handed { ids, turn } => {
                        if turn.is_empty() {
                            return Err(std::io::Error::other(
                                "the mod handed words over without identifying their turn",
                            ));
                        }
                        taken(node, &resident, &session, &turn, |pending| {
                            ids.iter().any(|id| id == pending.delivery.id.as_str())
                        })
                        .await?;
                        exchange::acknowledge_handover(directory, &ids, &turn)?;
                    }
                    exchange::Reported::Ended { reason, instance } => {
                        let current = node
                            .claude
                            .state
                            .lock()
                            .expect("never poisoned")
                            .sessions
                            .get(&session)
                            .is_some_and(|entry| instance.is_none() || entry.registration.instance == instance);
                        if current {
                            ended(node, &resident, &session, reason).await?;
                        }
                    }
                }
            }
            exchange::acknowledge(directory, &report.name)
        }
        .await;
        if let Err(error) = applied {
            node.log.line(format_args!(
                "applying {} from {session}: {error}; the report remains for recovery",
                report.name
            ));
            return;
        }
    }
    match exchange::receipts(directory) {
        Ok(receipts) => {
            for (id, turn) in receipts {
                if let Err(error) =
                    taken(node, &resident, &session, &turn, |pending| pending.delivery.id.as_str() == id).await
                {
                    node.log.line(format_args!("applying the receipt for {id}: {error}"));
                    return;
                }
            }
        }
        Err(error) => {
            node.log.line(format_args!("reading consumption receipts for {session}: {error}"));
            return;
        }
    }
    if let Err(error) = exchange::handed_over(directory) {
        node.log.line(format_args!("clearing what {session}'s mod handed over: {error}"));
    }
}

// Register synchronously with the caller, without depending on a filesystem
// watcher having run. Registration, alias persistence and publication are one
// serialized operation; report consumption holds a separate per-session lock.
async fn hear(node: &Arc<Node>, registration: exchange::Registration) {
    if let Err(error) = pairing::validate_session(&registration.session) {
        node.log.line(format_args!("registering a Claude session: {error}"));
        return;
    }
    let session = registration.session.clone();
    let directory = exchange::sessions(node.world.path()).join(&session);
    if let Some(process) = &registration.process {
        match fastctx_platform::is_alive(process) {
            Ok(false) => match exchange::recover_reports(&directory) {
                Ok(problems) => {
                    for problem in problems {
                        node.log.line(problem);
                    }
                }
                Err(error) => {
                    node.log.line(format_args!("recovering the exited host's reports: {error}"));
                    return;
                }
            },
            Ok(true) => {}
            Err(error) => {
                node.log.line(format_args!("checking the Claude host process: {error}"));
                return;
            }
        }
    }
    // Even known sessions recover their last incomplete reports after the
    // host exits. They need not wait for unrelated background births.
    if node.claude.state.lock().expect("never poisoned").sessions.get(&session).is_some_and(|entry| {
        entry.resident.is_some() && (entry.registration == registration || entry.registration.at > registration.at)
    }) {
        return;
    }
    let _births = node.claude.births.write().await;
    let prior = {
        let state = node.claude.state.lock().expect("never poisoned");
        state
            .sessions
            .get(&session)
            .and_then(|entry| Some((entry.resident.clone()?, entry.registration.process.clone()?)))
    };
    if let Some((resident, process)) = &prior
        && registration.process.as_ref() != Some(process)
    {
        match fastctx_platform::is_alive(process) {
            Ok(false) => {
                if let Err(error) = ended_for_process(
                    node,
                    resident,
                    &session,
                    Some("its host process exited".to_string()),
                    Some(process),
                )
                .await
                {
                    node.log.line(format_args!("settling the previous Claude host: {error}"));
                    return;
                }
            }
            Ok(true) => {}
            Err(error) => {
                node.log.line(format_args!("checking the previous Claude host: {error}"));
                return;
            }
        }
    }
    let speaking = prior.as_ref().and_then(|(resident, _)| node.residents.speaking(resident));
    let guard = match speaking {
        Some(lock) => Some(lock.lock_owned().await),
        None => None,
    };
    let changed = {
        let mut state = node.claude.state.lock().expect("never poisoned");
        match state.sessions.get_mut(&session) {
            Some(entry) if entry.registration == registration || entry.registration.at > registration.at => false,
            Some(entry) => {
                entry.registration = registration;
                entry.resident = None;
                true
            }
            None => {
                state.sessions.insert(
                    session.clone(),
                    Session { registration, resident: None, reading: Arc::new(tokio::sync::Mutex::new(())) },
                );
                true
            }
        }
    };
    drop(guard);
    let unpaired = node
        .claude
        .state
        .lock()
        .expect("never poisoned")
        .sessions
        .get(&session)
        .is_some_and(|entry| entry.resident.is_none());
    if changed || unpaired {
        pair(node, &session).await;
    }
}

/// What the World kept for the resident that its session took into `turn`:
/// whoever it was for is owed that turn's end. Which are taken is said by
/// the mod, by id; `which` picks among those kept by how far they came.
async fn taken(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    turn: &str,
    which: impl Fn(&Pending) -> bool,
) -> std::io::Result<()> {
    let Some(town) = node.residents.town_of(resident) else { return Ok(()) };
    let directory = node.world.resident(town.as_ref(), resident);
    let speaking = node.residents.speaking(resident).expect("known resident");
    let _speaking = speaking.lock().await;
    crate::delivery::remember_session(&directory, turn, session)?;
    let mut kept: Vec<Pending> = crate::inbox::pending(&directory)?
        .into_iter()
        .filter(|pending| pending.session.as_deref().is_none_or(|bound| bound == session))
        .filter(which)
        .collect();
    if kept.is_empty() {
        return Ok(());
    }
    let mut audience = node.residents.readers(resident, turn);
    if audience.is_empty() {
        if let Some(finished) = crate::delivery::terminal(&directory, turn)? {
            audience = finished.readers;
        } else if node.residents.about(resident).is_some_and(|(about, _)| about.session.as_deref() == Some(session)) {
            audience = node.residents.working_for(resident);
        }
    }
    for pending in &mut kept {
        if pending.working {
            pending.readers = audience.clone();
            pending.working = false;
        }
    }
    let mut handover = crate::delivery::begin(&directory, kept)?;
    crate::speech::bind(node, resident, &directory, &mut handover, turn)
        .await
        .map_err(|error| std::io::Error::other(error.message))
}

async fn uncertain_inputs(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    ids: Option<&[String]>,
    why: String,
) -> std::io::Result<()> {
    let Some(town) = node.residents.town_of(resident) else {
        return Ok(());
    };
    let directory = node.world.resident(town.as_ref(), resident);
    let known: HashSet<_> = crate::delivery::handovers(&directory)?
        .into_iter()
        .filter(|handover| handover.turn.is_some())
        .flat_map(|handover| handover.inputs.into_iter().map(|input| input.delivery.id))
        .collect();
    let pending = crate::inbox::pending(&directory)?
        .into_iter()
        .filter(|pending| pending.session.as_deref() == Some(session) || (pending.session.is_none() && ids.is_some()))
        .filter(|pending| !known.contains(&pending.delivery.id))
        .filter(|pending| ids.is_none_or(|ids| ids.iter().any(|id| id == pending.delivery.id.as_str())))
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(());
    }
    let mut handover = crate::delivery::begin(&directory, pending)?;
    if let Some(turn) = handover.turn.clone() {
        crate::speech::bind(node, resident, &directory, &mut handover, &turn)
            .await
            .map_err(|error| std::io::Error::other(error.message))?;
    } else {
        crate::turns::unresolved(
            node,
            resident,
            &directory,
            &mut handover,
            fastctx_types::session::TurnHow::Interrupted,
            why,
        )
        .await?;
    }
    Ok(())
}

/// The resident's session ended: a turn it ran ends with it, and it sleeps
/// until words wake it.
async fn ended(node: &Arc<Node>, resident: &ResidentId, session: &str, reason: Option<String>) -> std::io::Result<()> {
    ended_for_process(node, resident, session, reason, None).await
}

async fn ended_for_process(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    reason: Option<String>,
    expected: Option<&fastctx_types::ProcessIdentity>,
) -> std::io::Result<()> {
    let speaking = node.residents.speaking(resident).expect("known resident");
    let _speaking = speaking.lock().await;
    if let Some(expected) = expected {
        let state = node.claude.state.lock().expect("never poisoned");
        if state.sessions.get(session).and_then(|entry| entry.registration.process.as_ref()) != Some(expected) {
            return Ok(());
        }
    }
    let Some((about, town)) = node.residents.about(resident) else { return Ok(()) };
    let directory = node.world.resident(town.as_ref(), resident);
    let current = about.session.as_deref() == Some(session);
    let mut turns = std::collections::BTreeSet::new();
    if let Some(now) = node.residents.now(resident) {
        turns.extend(now.owed.into_keys());
        turns.extend(now.turn);
    }
    for turn in turns {
        let owner = crate::delivery::session(&directory, &turn)?;
        if owner.as_deref() != Some(session) && !(owner.is_none() && current) {
            continue;
        }
        let why = match &reason {
            Some(reason) => format!("its Claude Code conversation ended ({reason})"),
            None => "its Claude Code conversation ended".to_string(),
        };
        let end = fastctx_types::session::TurnEnd {
            error: None,
            how: fastctx_types::session::TurnHow::Interrupted,
            reason: Some(why),
            duration_ms: None,
            last_message: None,
        };
        crate::turns::finish_locked(node, resident, &turn, end, &[]).await?;
    }
    uncertain_inputs(node, resident, session, None,
        "the conversation ended before the host identified which turn consumed these words; their acceptance is unknown".to_string()).await?;
    if current {
        let departed = node.residents.set_claude_here(resident, false);
        node.residents.change_now(&node.world, resident, |now| {
            now.activity = Activity::Asleep;
            now.since = Timestamp::now();
            now.turn = None;
        })?;
        if departed && reason.as_deref() != Some("clear") {
            crate::colleagues::left(node, resident.clone());
        }
    }
    Ok(())
}

/// Asks the paired mod to stop this resident's current turn.
pub fn stop(node: &Node, resident: &ResidentId) -> Result<bool, Failure> {
    let session = node.claude.state.lock().expect("never poisoned").paired.get(resident).cloned().ok_or_else(|| {
        Failure::new(format!("{resident}'s session has not been heard from the World's plugin, which is what stops it"))
    })?;
    let Some(turn) = node.residents.now(resident).and_then(|now| now.turn) else { return Ok(false) };
    let directory = exchange::sessions(node.world.path()).join(&session);
    exchange::stop(&directory, &turn).map_err(|error| Failure::new(format!("asking its plugin to stop: {error}")))?;
    Ok(true)
}

/// Whether the resident's Claude session is paired with its plugin now.
pub fn paired(node: &Node, resident: &ResidentId) -> bool {
    node.claude.state.lock().expect("never poisoned").paired.contains_key(resident)
}

/// Resolves this call's own evidence, including before the watcher has seen
/// its registration. No connection-wide or process-wide identity is reused.
pub async fn identify(node: &Arc<Node>, tool_use_id: &str) -> Result<fastctx_types::residents::Whereabouts, Failure> {
    let world = node.world.path().to_path_buf();
    let id = tool_use_id.to_string();
    let registration = tokio::task::spawn_blocking(move || claude::calls::registration(&world, &id))
        .await
        .map_err(|error| Failure::new(format!("reading Claude call evidence: {error}")))?
        .map_err(|error| Failure::new(format!("pairing Claude call {tool_use_id}: {error}")))?;
    let session = registration.session.clone();
    hear(node, registration).await;
    // The reports preceding the call may establish its turn and deliveries.
    look(node, &exchange::sessions(node.world.path()).join(&session)).await;
    let resident = node.claude.state.lock().expect("never poisoned").sessions.get(&session)
        .and_then(|entry| entry.resident.clone())
        .ok_or_else(|| Failure::new(format!("Claude call {tool_use_id} belongs to session {session}, whose resident could not be registered; see the node log")))?;
    let town =
        node.residents.town_of(&resident).ok_or_else(|| Failure::new("the paired Claude resident is missing"))?;
    Ok(fastctx_types::residents::Whereabouts { resident, town })
}

/// Pairs a registered session with its resident, if it is known yet.
async fn pair(node: &Arc<Node>, session: &str) {
    let registration = {
        let state = node.claude.state.lock().expect("never poisoned");
        match state.sessions.get(session) {
            Some(entry) if entry.resident.is_none() => entry.registration.clone(),
            _ => return,
        }
    };
    let resident = match pairing::resident(node, &registration) {
        Ok(resident) => resident,
        Err(error) => {
            node.log.line(format_args!("pairing Claude session {session}: {error}"));
            return;
        }
    };
    let speaking = node.residents.speaking(&resident).expect("known resident");
    let guard = speaking.lock().await;
    let current = node.residents.about(&resident).and_then(|(about, _)| about.session);
    let newer = match current.as_deref() {
        None => true,
        Some(prior) if prior == session => true,
        Some(prior) => match exchange::registration(&exchange::sessions(node.world.path()).join(prior)) {
            Ok(Some(previous)) => {
                registration.at.is_some()
                    && (registration.at > previous.at
                        || (registration.at == previous.at && registration.previous.as_deref() == Some(prior)))
            }
            Ok(None) => true,
            Err(error) => {
                node.log.line(format_args!("reading the current Claude registration: {error}"));
                return;
            }
        },
    };
    if newer {
        if let Some(prior) = current.as_deref().filter(|prior| *prior != session)
            && let Some((_, town)) = node.residents.about(&resident)
            && let Some(now) = node.residents.now(&resident)
        {
            let directory = node.world.resident(town.as_ref(), &resident);
            for turn in now.owed.keys().chain(now.turn.iter()) {
                if let Err(error) = crate::delivery::remember_session(&directory, turn, prior) {
                    node.log.line(format_args!("remembering the prior conversation: {error}"));
                    return;
                }
            }
        }
        if let Err(error) = node.residents.set_session(
            &node.world,
            &resident,
            session,
            registration.model_name(),
            registration.version_text(),
        ) {
            node.log.line(format_args!("recording which session {resident} is: {error}"));
            return;
        }
        if let Some(inbox) = &registration.inbox_address {
            node.residents.set_inbox(&resident, inbox.clone());
        }
        node.claude.state.lock().expect("never poisoned").paired.insert(resident.clone(), session.to_string());
    }
    // Publish the association only after both the durable alias and about
    // record are written. A failed write can then be retried on the next look.
    node.claude.state.lock().expect("never poisoned").sessions.get_mut(session).expect("looked up above").resident =
        Some(resident.clone());
    if newer {
        let present = match pairing::present(node, &registration) {
            Ok(present) => present,
            Err(error) => {
                node.log.line(format_args!("checking Claude presence: {error}"));
                return;
            }
        };
        node.residents.set_claude_here(&resident, present);
    }
    // What waited for the resident before its mod was there goes to the mod,
    // and stays kept until the mod says which turn took it.
    let Some(town) = node.residents.town_of(&resident) else { return };
    let directory = node.world.resident(town.as_ref(), &resident);
    let waiting = {
        let directory = directory.clone();
        tokio::task::spawn_blocking(move || crate::inbox::pending(&directory)).await
    };
    match waiting {
        Ok(Ok(kept)) => {
            for pending in &kept {
                if pending.state == PendingState::Waking
                    && pending.session.is_none()
                    && newer
                    && let Err(error) = crate::inbox::target(&directory, &pending.delivery.id, session)
                {
                    node.log.line(format_args!("binding the session's first words: {error}"));
                    return;
                }
            }
            for pending in kept.into_iter().filter(|pending| {
                pending.state == PendingState::Waiting
                    && match pending.session.as_deref() {
                        Some(bound) => bound == session,
                        None => newer,
                    }
            }) {
                if let Err(failure) = into_inbox(node, session, &directory, &pending) {
                    node.log.line(format_args!("handing {} to {resident}'s session: {failure}", pending.delivery.id));
                }
            }
        }
        Ok(Err(error)) => node.log.line(format_args!("reading what waits for {resident}: {error}")),
        Err(_) => {}
    }
    drop(guard);
    process::observe(node, registration.process);
}

/// Hands something kept to a resident whose Claude session the World
/// hears; `None` when it hears none for it.
pub async fn deliver(
    node: &Arc<Node>,
    to: &ResidentId,
    about: &About,
    directory: &ResidentDir,
    pending: &Pending,
) -> Option<Result<Landed, Failure>> {
    let delivering = {
        let mut state = node.claude.state.lock().expect("never poisoned");
        state.paired.get(to)?;
        state.delivering.entry(to.clone()).or_default().clone()
    };
    let _delivering = delivering.lock().await;
    let session = node.claude.state.lock().expect("never poisoned").paired.get(to).cloned()?;
    if let Err(error) = crate::inbox::target(directory, &pending.delivery.id, &session) {
        return Some(Err(Failure::new(error.to_string())));
    }
    if !about.world_started {
        // The user's own session: whatever its state, its mod hands this over
        // when the session runs, and when the user opens it again.
        return Some(into_inbox(node, &session, directory, pending));
    }
    Some(match running(node, &session).await {
        Ok(true) => into_inbox(node, &session, directory, pending),
        // Its supervisor stopped it: the words wake it, as they are, and its
        // next turn takes them.
        Ok(false) => {
            let (text, as_user) = words(&pending.delivery.notice);
            let claimed: std::io::Result<bool> = (|| {
                let taking = node.taking.lock().expect("never poisoned");
                if taking.contains(&(to.clone(), pending.delivery.id.clone())) {
                    return Ok(false);
                }
                let Some(current) = crate::inbox::read(directory, &pending.delivery.id)? else {
                    return Ok(false);
                };
                if current.state != PendingState::Waiting {
                    return Ok(false);
                }
                crate::inbox::set_state(directory, &pending.delivery.id, PendingState::Waking)?;
                Ok(true)
            })();
            match claimed {
                Ok(true) => {
                    match wake(
                        node,
                        &session,
                        if as_user { text } else { claude::framed(&text) },
                        pending.delivery.id.clone(),
                    )
                    .await
                    {
                        Ok(landed) => Ok(landed),
                        Err(error) => failed_wake(node, to, directory, &pending.delivery.id, error).await,
                    }
                }
                Ok(false) => Ok(Landed::Waiting {
                    why: Some("these words already have a handover in progress".to_string()),
                    answered: true,
                }),
                Err(error) => Err(Failure::new(format!("keeping what wakes {to}: {error}"))),
            }
        }
        Err(failure) => Err(failure),
    })
}

async fn failed_wake(
    node: &Arc<Node>,
    resident: &ResidentId,
    directory: &ResidentDir,
    id: &fastctx_types::NoticeId,
    error: Failure,
) -> Result<Landed, Failure> {
    let speaking = node.residents.speaking(resident).expect("known resident");
    let _speaking = speaking.lock().await;
    if let Some(turn) = crate::delivery::input_turn(directory, id).map_err(|error| Failure::new(error.to_string()))? {
        node.log.line(format_args!("the Claude command failed after its turn was identified: {error}"));
        return Ok(Landed::NewTurn { turn });
    }
    let why = format!("the Claude command failed before identifying a consuming turn: {error}");
    if let Some(pending) = crate::inbox::read(directory, id).map_err(|error| Failure::new(error.to_string()))? {
        crate::turns::unsettled(
            node,
            resident,
            directory,
            pending,
            fastctx_types::session::TurnHow::Interrupted,
            why.clone(),
        )
        .await;
    }
    Ok(Landed::Uncertain { why })
}

/// Puts something kept into the mod's inbox; it is kept until the mod says
/// which turn took it.
fn into_inbox(node: &Node, session: &str, directory: &ResidentDir, pending: &Pending) -> Result<Landed, Failure> {
    let taking = node.taking.lock().expect("never poisoned");
    if taking.iter().any(|(_, id)| *id == pending.delivery.id) {
        return Ok(Landed::Waiting {
            why: Some("a World tool is handing these words over".to_string()),
            answered: true,
        });
    }
    if crate::inbox::read(directory, &pending.delivery.id).map_err(|error| Failure::new(error.to_string()))?.is_none() {
        return Ok(Landed::Handed { id: pending.delivery.id.clone() });
    }
    let session_directory = exchange::sessions(node.world.path()).join(session);
    crate::inbox::target(directory, &pending.delivery.id, session).map_err(|error| Failure::new(error.to_string()))?;
    let (text, as_user) = words(&pending.delivery.notice);
    exchange::deliver(&session_directory, pending.delivery.id.as_str(), &text, as_user)
        .and_then(|()| crate::inbox::set_state(directory, &pending.delivery.id, PendingState::Handed))
        .map(|()| Landed::Handed { id: pending.delivery.id.clone() })
        .map_err(|error| Failure::new(format!("handing it to the session's plugin: {error}")))
}

/// The text a notice is in a Claude session, and whether it is the user's
/// own words, which go in as the user's.
fn words(notice: &Notice) -> (String, bool) {
    match notice {
        Notice::Words { from: Speaker::User { .. }, text } => (text.clone(), true),
        notice => (told::told(notice), false),
    }
}

/// Whether the background session has a running process.
async fn running(node: &Arc<Node>, session: &str) -> Result<bool, Failure> {
    let Command { program, cwd, env } = command(node)?;
    let session = session.to_string();
    tokio::task::spawn_blocking(move || {
        let args: Vec<OsString> = vec!["agents".into(), "--json".into(), "--all".into()];
        let listed = background::claude(&program, &args, &cwd, &env)?;
        background::running(&listed, &session)
    })
    .await
    .map_err(|error| Failure::new(format!("asking Claude Code about its sessions: {error}")))?
    .map_err(Failure::new)
}

async fn wake(node: &Arc<Node>, session: &str, words: String, id: fastctx_types::NoticeId) -> Result<Landed, Failure> {
    let Command { program, cwd, env } = command(node)?;
    let args = background::wake_args(session, &words);
    let world = node.world.path().to_path_buf();
    // Not tried twice: a resume refused for trust may already have queued
    // the words.
    tokio::task::spawn_blocking(move || trust(&world).and_then(|()| background::claude(&program, &args, &cwd, &env)))
        .await
        .map_err(|error| Failure::new(format!("waking the session: {error}")))?
        .map_err(Failure::new)?;
    Ok(Landed::Handed { id })
}

/// The `claude` command, where to run it, and the environment it gets: the
/// user's own, as for every program the node starts.
struct Command {
    program: PathBuf,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
}

fn command(node: &Node) -> Result<Command, Failure> {
    let mut env = fastctx_platform::world_process_environment()
        .map_err(|error| Failure::new(format!("reading the user's environment: {error}")))?;
    // A `claude --bg` may start Claude Code's daemon, whose environment every
    // later background session gets, the user's own included. Nothing of the
    // World goes into it: sessions are known by their ids, and the plugin
    // names its World itself.
    env.retain(|(name, _)| {
        !["FASTCTX_RESIDENT", "FASTCTX_HOME", "FASTCTX_COLLEAGUE"].iter().any(|world| name.eq_ignore_ascii_case(world))
    });
    // Found on the node's own search path, which for a node the World
    // started is the user's own.
    let path = std::env::var_os("PATH");
    let program = background::find(path.as_ref()).ok_or_else(|| {
        Failure::new(format!("Claude Code is not installed on {}: no claude command", node.machine()))
    })?;
    let cwd =
        fastctx_platform::user_home().map_err(|error| Failure::new(format!("finding the home directory: {error}")))?;
    Ok(Command { program, cwd, env })
}

/// Opens a Claude Code background session on this machine for the user
/// typing on `typed_on`, with their words as its first.
pub async fn open_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    town: Option<TownName>,
    text: String,
) -> Result<(ResidentId, Landed), Failure> {
    if let Some(town) = &town {
        crate::town::project::spans_here(node, town).await?;
    }
    let agent = AgentName::try_from("claude").expect("a valid agent name");
    let id = node.residents.new_id(&agent)?;
    let Command { program, env, .. } = command(node)?;
    let (words, heard) = crate::colleagues::instructions(node, &id, town.as_ref(), &Origin::User).await?;
    let born_at = Timestamp::now();
    let about = About {
        id: id.clone(),
        host: Host { agent, client: "claude-code".to_string(), version: String::new() },
        machine: node.machine(),
        born_at,
        origin: Origin::User,
        session: None,
        world_started: true,
        model: None,
    };
    let now = Now {
        activity: Activity::Busy,
        since: born_at,
        turn: None,
        owed: Default::default(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
    };
    node.residents
        .create(&node.world, about, town.clone(), now)
        .map_err(|error| Failure::new(format!("recording the session: {error}")))?;
    node.residents.set_heard(&id, heard);
    let directory = node.world.resident(town.as_ref(), &id);
    let settings = directory.path().join("claude-settings.json");
    let instructions = directory.path().join("claude-instructions.md");
    let written = std::fs::write(&settings, settings_json()).and_then(|()| std::fs::write(&instructions, words));
    written.map_err(|error| Failure::new(format!("writing the session's settings: {error}")))?;
    let cwd = crate::colleagues::standing(node, town.as_ref())?;
    let plugin = claude::plugin_dir(node.world.path());
    // The first words go in as the prompt the session starts with, which it
    // reads as the user's: they are. They are kept until its first turn
    // takes them, so whoever follows them knows which turn that is.
    let first = Pending {
        delivery: Delivery {
            id: crate::speech::new_id("words"),
            at: born_at,
            notice: Notice::Words { from: Speaker::User { machine: typed_on.clone() }, text: text.clone() },
        },
        readers: Vec::new(),
        state: PendingState::Waking,
        working: false,
        session: None,
    };
    crate::inbox::keep(&directory, &first)
        .map_err(|error| Failure::new(format!("keeping the first words: {error}")))?;
    let world = node.world.path().to_path_buf();
    // Until the World knows which session this is, a Claude session's MCP
    // connection it cannot place waits, rather than becoming someone new.
    let _birth = node.claude.births.read().await;
    let started = tokio::task::spawn_blocking(move || {
        let start =
            background::Start { plugin: &plugin, settings: &settings, instructions: &instructions, first_words: &text };
        let args = background::start_args(&start);
        trust(&world)?;
        match background::claude(&program, &args, &cwd, &env) {
            // A Claude Code that was running can write its state back over
            // the trust a moment after it was written; a new session refused
            // for exactly that was not created, so it is started once more.
            Err(why) if why.contains(background::UNTRUSTED) => {
                trust(&world)?;
                background::claude(&program, &args, &cwd, &env)
            }
            started => started,
        }
    })
    .await
    .map_err(|error| Failure::new(format!("starting the session: {error}")))
    .and_then(|result| result.map_err(Failure::new))
    .and_then(|printed| {
        background::started(&printed).ok_or_else(|| {
            Failure::new(format!(
                "Claude Code started a session without saying which, so the World cannot follow it; it printed: {}",
                printed.trim()
            ))
        })
    });
    let short = match started {
        Ok(short) => short,
        Err(error) => {
            drop(_birth);
            let landed = failed_wake(node, &id, &directory, &first.delivery.id, error).await?;
            return Ok((id, landed));
        }
    };
    // The session is known by the start of its id until its mod reports the
    // whole of it, which may have happened already.
    node.residents
        .set_session(&node.world, &id, &short, None, None)
        .map_err(|error| Failure::new(format!("recording which session {id} is: {error}")))?;
    drop(_birth);
    replay(node).await;
    Ok((id, Landed::Handed { id: first.delivery.id }))
}

/// Has Claude Code check the plugin as it checks one before loading it,
/// and says loudly when it would not load: its mod is read and judged by
/// Claude Code before it runs, and a mod it refuses tells the World nothing
/// at all. Without Claude Code here there is nothing to load it.
fn check_plugin(node: &Node, plugin: &Path) {
    let Ok(Command { program, cwd, env }) = command(node) else { return };
    let args: Vec<OsString> = vec!["plugin".into(), "validate".into(), plugin.into()];
    match background::claude(&program, &args, &cwd, &env) {
        Ok(_) => node.log.line("Claude Code accepts the World's plugin"),
        Err(why) => node.log.line(format_args!(
            "Claude Code would not load the World's plugin, so its sessions tell the World nothing: {why}"
        )),
    }
}

/// The World's directory, where every session the World starts stands, is
/// trusted in Claude Code, so it starts sessions there without asking
/// (`world-projects.md`).
fn trust(world: &Path) -> Result<(), String> {
    claude::trust::ensure_trusted(world)
        .map(|_| ())
        .map_err(|error| format!("trusting {} in Claude Code: {error}", world.display()))
}

/// Settings for a background session the World starts: the host's limit on
/// a tool answer opened past the World's own (`world-view.md`, 一次给多少),
/// and the host's own shell, file and search tools turned off, since the
/// World's take their place.
fn settings_json() -> String {
    let settings = serde_json::json!({
        "env": { "MAX_MCP_OUTPUT_TOKENS": fastctx_hosts::claude::MAX_MCP_OUTPUT_TOKENS.to_string() },
        "permissions": {
            "allow": [fastctx_hosts::claude::WORLD_TOOLS],
            "deny": ["Bash", "PowerShell", "Read", "Write", "Edit", "MultiEdit", "NotebookEdit", "Grep", "Glob"],
        },
    });
    serde_json::to_string_pretty(&settings).expect("JSON values serialize")
}
