//! Claude sessions, as the node hears them through the World's plugin
//! (`world-colleagues.md`, 话怎样送到居民眼前).
//!
//! The node writes the plugin into the World's directory when it starts.
//! Every session the plugin runs in has a directory under `<world>/claude`,
//! which the node watches. A session's registration pairs it with its
//! resident: the one the World started under its id, the one the session
//! it continues after a clear was, or a new resident for this session.
//! A call to the World's tools is known for its session by the proof the
//! mod wrote under the call's id before letting it through.
//! What it reports is read once it is paired, in the order it was written,
//! and goes to that resident; what waited for the resident goes to its mod.
//! What the World has for a paired session goes into its mod's inbox, and
//! the mod says which turn took it; a background session the World started
//! whose process the supervisor stopped is woken with it, and its next turn
//! takes it.
//!
//! Run with Claude Code 2.1.289.

pub(crate) mod calls;
mod catalog;
pub(crate) mod control;
pub(crate) mod control_outcome;
mod copies;
mod fork;
mod fork_attachments;
mod fork_history;
mod fork_ready;
mod fork_source;
pub(crate) mod garbage;
pub(crate) mod goal_compaction;
pub(crate) mod history;
mod image_records;
pub(crate) mod resume;
mod retire_command;
pub(crate) use image_records::paths as image_records;
mod image_flush;
pub(crate) use image_flush::wait_for_image;
pub(crate) mod lifecycle;
pub mod management;
mod oldest;
pub(crate) mod pairing;
mod plugin_check;
mod process;
mod rewind;
mod startup;
mod thoughts;
mod titles;
pub(crate) mod turn_facts;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_hands::told;
use fastctx_hosts::claude::{self, background, exchange};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::{About, Activity, Host, Landed, Notice, Now, Origin, Pending, PendingState};
use fastctx_types::session::{Happened, Speaker};
use fastctx_types::{AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;
use tokio::sync::mpsc;

use crate::Node;

pub struct Claude {
    title_files: titles::Files,
    thoughts: thoughts::Thoughts,
    state: Mutex<State>,
    bridge: control::Bridge,
    delivering: crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<()>>,
    reading: crate::leases::Registry<String, fastctx_platform::doing::Mutex<()>>,
    startup_listings: startup::Listings,
    changed: tokio::sync::Notify,
    /// Held to read while the World starts a session and learns its id;
    /// to write, while a mod registration is assigned to a resident.
    pub births: crate::ranked::Births,
    /// Wakes in flight, and the copies Claude Code started instead.
    copies: copies::Copies,
    /// The user's words handed to each resident's session as the user's,
    /// until Claude Code reports keeping them (`said_by_world`).
    handed: Mutex<HashMap<ResidentId, Vec<String>>>,
    /// The Claude Code last found new enough to start sessions in.
    checked: oldest::Checked,
}

struct State {
    sessions: HashMap<String, Session>,
    starting: HashSet<ResidentId>,
    /// Resident to the session it is.
    paired: crate::residents::cache::Index,
    observed: HashSet<fastctx_types::ProcessIdentity>,
    /// The last record each session's mod saw Claude Code keep, in this run
    /// of the node: what its file must hold before it is read as whole.
    stored: crate::residents::cache::Index,
}

struct Session {
    registration: exchange::Registration,
    resident: Option<ResidentId>,
}

impl Claude {
    pub fn new(world: &fastctx_types::layout::WorldDir) -> std::io::Result<Self> {
        let root = world.path().join("resident-cache");
        Ok(Self {
            title_files: Default::default(),
            thoughts: Default::default(),
            state: Mutex::new(State {
                sessions: HashMap::new(),
                starting: HashSet::new(),
                observed: HashSet::new(),
                paired: crate::residents::cache::Index::new(root.join("claude-paired"))?,
                stored: crate::residents::cache::Index::new(root.join("claude-stored"))?,
            }),
            bridge: Default::default(),
            births: Default::default(),
            copies: Default::default(),
            delivering: Default::default(),
            reading: Default::default(),
            startup_listings: Default::default(),
            changed: Default::default(),
            handed: Default::default(),
            checked: Default::default(),
        })
    }

    /// The user's words `text` go to `resident`'s session as the user's.
    fn handing(&self, resident: &ResidentId, text: &str) {
        self.handed.lock().expect("never poisoned").entry(resident.clone()).or_default().push(text.to_string());
    }

    /// Whether the user's message `text` Claude Code reports keeping in
    /// `resident`'s session is words the World handed it as the user's. The
    /// World shows those as it heard them, so the host's own record of them
    /// is the same words a second time; each handing answers one report.
    /// Words still in the inbox count too, as after the node restarted
    /// between handing them and the report.
    pub(crate) fn said_by_world(&self, directory: &ConversationDir, resident: &ResidentId, text: &str) -> bool {
        {
            let mut handed = self.handed.lock().expect("never poisoned");
            if let Some(words) = handed.get_mut(resident)
                && let Some(at) = words.iter().position(|words| words == text)
            {
                words.remove(at);
                if words.is_empty() {
                    handed.remove(resident);
                }
                return true;
            }
        }
        crate::inbox::pending(directory).is_ok_and(|pending| {
            pending.iter().any(|pending| {
                matches!(pending.state, PendingState::Handed | PendingState::Waking)
                    && matches!(words(&pending.delivery.notice, None), (words, true) if words == text)
            })
        })
    }
}

/// Writes the plugin and starts watching the sessions.
pub fn start(node: &Arc<Node>) -> std::io::Result<()> {
    claude::control::write(&node.world.path().join("claude-controller.json"), &fastctx_platform::current_identity()?)?;
    calls::sweep(node);
    let plugin = claude::write_plugin(node.world.path())?;
    let checking = node.clone();
    tokio::task::spawn_blocking(move || plugin_check::run(&checking, &plugin));
    let sessions = exchange::sessions(node.world.path());
    fastctx_platform::ensure_private_dir(&sessions)?;
    let (changed, mut changes) = mpsc::unbounded_channel::<PathBuf>();
    let root = sessions.clone();
    let seen = crate::file_events::named(&sessions)?;
    let conversations = node.world.conversations();
    fastctx_platform::ensure_private_dir(&conversations)?;
    let homes = crate::file_events::named(&conversations)?;
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        match event {
            Ok(event) if !crate::file_events::changes(&event) => {}
            Ok(event) if !event.need_rescan() => {
                for path in event.paths {
                    // The session's directory: the first name under the root.
                    if let Ok(rest) = path.strip_prefix(&seen)
                        && let Some(first) = rest.components().next()
                    {
                        let _ = changed.send(root.join(first));
                    } else if let Ok(rest) = path.strip_prefix(&homes) {
                        let mut parts = rest.iter();
                        if parts.next().is_some()
                            && parts.next().is_some_and(|name| name == "claude")
                            && let Some(session) = parts.next()
                        {
                            let _ = changed.send(root.join(session));
                        }
                    }
                }
            }
            // Something was missed, as when the system's queue overflowed:
            // every session is looked at again.
            _ => {
                let _ = changed.send(root.clone());
            }
        }
    })
    .map_err(std::io::Error::other)?;
    notify::Watcher::watch(&mut watcher, &sessions, notify::RecursiveMode::Recursive).map_err(std::io::Error::other)?;
    notify::Watcher::watch(&mut watcher, &conversations, notify::RecursiveMode::Recursive)
        .map_err(std::io::Error::other)?;
    let node = node.clone();
    tokio::spawn(async move {
        // Kept for as long as the node watches.
        let _watcher = watcher;
        look_all(&node, &sessions).await;
        let mut retry = tokio::time::interval(std::time::Duration::from_secs(300));
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        retry.tick().await;
        loop {
            let first = tokio::select! {
                first = changes.recv() => match first {
                    Some(first) => first,
                    None => break,
                },
                _ = retry.tick() => sessions.clone(),
            };
            let mut directories = HashSet::from([first]);
            while let Ok(more) = changes.try_recv() {
                directories.insert(more);
            }
            for directory in directories {
                if directory == sessions {
                    look_all(&node, &sessions).await;
                } else {
                    look_alone(&node, directory).await;
                }
            }
        }
    });
    Ok(())
}

/// Looks at one session's directory in a task of its own: whatever goes
/// wrong with one session stays with it, and the watching goes on.
async fn look_alone(node: &Arc<Node>, directory: PathBuf) {
    let (looking, path) = (node.clone(), directory.clone());
    if let Err(error) = tokio::spawn(async move { look(&looking, &path).await }).await {
        node.log.line(format_args!("looking at the Claude session {}: {error}", directory.display()));
    }
}

/// Held while what one session reported is read and applied.
pub(crate) fn reading(node: &Node, session: &str) -> crate::leases::Lease<String, fastctx_platform::doing::Mutex<()>> {
    node.claude.reading.get_or_insert_with(&session.to_string(), || crate::ranked::reading(session))
}

pub async fn replay(node: &Arc<Node>) {
    let directory = exchange::sessions(node.world.path());
    if let Ok(entries) = std::fs::read_dir(&directory) {
        for entry in entries {
            let result = entry.and_then(|entry| exchange::recover_readable(&entry.path()));
            match result {
                Ok(0) => {}
                Ok(count) => {
                    node.log.line(format_args!("recovered {count} Claude reports the current decoder can read"))
                }
                Err(error) => node.log.line(format_args!("reconsidering retained Claude reports: {error}")),
            }
        }
    }
    look_all(node, &directory).await;
}

pub(crate) fn recover_startups(node: &Arc<Node>) {
    fork_ready::recover(node);
    startup::recover(node);
}

/// Publishes what the session's mod waits for before its system prompt and
/// its first model request: its model and effort.
/// It is published as soon as the session is identified, transcript or not:
/// Claude Code may build the system prompt before the session-start hook
/// that reports where the transcript is, so waiting for that would hold the
/// first turn forever. The context fields come from the transcript when one
/// is known; the mod's rewind and fork read the transcript themselves.
fn policy(node: &Node, resident: &ResidentId, directory: &Path) -> std::io::Result<()> {
    node.residents.about(resident).ok_or_else(|| std::io::Error::other("unknown Claude resident"))?;
    let options = crate::session_options::read(node, resident).map_err(|error| std::io::Error::other(error.message))?;
    let path = directory.join("policy.json");
    let registration =
        exchange::registration(directory)?.ok_or_else(|| std::io::Error::other("Claude registration is incomplete"))?;
    let transcript = exchange::transcript(directory)?;
    if let Some(prior) = claude::control::read::<serde_json::Value>(&path)?
        && prior["instance"] == serde_json::to_value(&registration.instance).map_err(std::io::Error::other)?
    {
        return Ok(());
    }
    let mut last_uuid = None;
    let mut context_tail = None;
    let mut context_uuids = None;
    if let Some(transcript) = transcript {
        loop {
            match claude::transcript::read(&transcript) {
                Ok(read) => {
                    last_uuid = read.context_uuids.last().cloned();
                    context_tail = read.context_tail;
                    if read.context_uuids.len() < 4096 {
                        context_uuids = Some(read.context_uuids)
                    }
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    }
    crate::goals::holding::registered(node, resident, directory)
        .map_err(|error| std::io::Error::other(error.message))?;
    let value = serde_json::json!({"instance": registration.instance, "model": options.model,
        "effort": options.reasoning_effort, "last_uuid": last_uuid, "context_uuids":context_uuids, "context_tail":context_tail});
    let bytes = serde_json::to_vec(&value).map_err(std::io::Error::other)?;
    fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
}

async fn look_all(node: &Arc<Node>, sessions: &Path) {
    let entries = match std::fs::read_dir(sessions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            node.log.line(format_args!("looking at the Claude sessions: {error}"));
            return;
        }
    };
    for entry in entries {
        let directory = entry.and_then(|entry| {
            let path = entry.path();
            std::fs::metadata(&path).map(|metadata| metadata.is_dir().then_some(path))
        });
        match directory {
            Ok(Some(directory)) => look_alone(node, directory).await,
            Ok(None) => {}
            Err(error) => node.log.line(format_args!("listing a Claude session; will retry: {error}")),
        }
    }
}

/// Takes what is new in one session's directory.
async fn look(node: &Arc<Node>, directory: &Path) {
    let Some(session) = directory.file_name().and_then(|name| name.to_str()).map(str::to_string) else {
        return;
    };
    let _doing = fastctx_platform::doing::doing(format!("reading the reports of Claude session {session}"));
    // A session of a conversation deleted here: what it still reports is
    // dropped, and it is taken for nothing new.
    if node.residents.deleted_session(&session).is_some() {
        return;
    }
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
    let resident = node
        .claude
        .state
        .lock()
        .expect("never poisoned")
        .sessions
        .get(&session)
        .and_then(|entry| entry.resident.clone());
    let Some(resident) = resident else {
        let _births = node.claude.births.write().await;
        let mut state = node.claude.state.lock().expect("never poisoned");
        if state.sessions.get(&session).is_some_and(|entry| entry.resident.is_none()) {
            state.sessions.remove(&session);
            node.claude.changed.notify_waiters();
        }
        return;
    };
    if let Err(error) = policy(node, &resident, directory) {
        node.log.line(format_args!("publishing Claude session policy for {session}: {error}"));
        return;
    }
    if let Err(error) = titles::start(node, &resident, directory).await {
        node.log.line(format_args!("watching Claude's title: {error}"));
    }
    if node.residents.removed(&resident).is_some() {
        let _births = node.claude.births.write().await;
        node.claude.state.lock().expect("never poisoned").sessions.remove(&session);
        node.claude.changed.notify_waiters();
        return;
    }
    let reading = reading(node, &session);
    let _reading = reading.lock().await;
    // Its conversation may have been deleted while this waited.
    if !node.residents.knows(&resident) || node.residents.deleted_session(&session).is_some() {
        return;
    }
    if let Err(error) = control::look(node, &resident, directory).await {
        node.log.line(format_args!("reading Claude controls for {session}: {error}"));
    }
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
                        let Some(speaking) = node.residents.speaking(&resident) else { return Ok(()) };
                        let _speaking = speaking.lock().await;
                        uncertain_inputs(node, &resident, &session, Some(&ids), text, None).await?;
                    }
                    exchange::Reported::Happened(event, activation) => {
                        if let Happened::TurnStarted { turn } = &event.happened {
                            taken(node, &resident, &session, turn, |pending| {
                                startup::takes_wake(node, &resident, &session, event.at, activation.as_deref(), pending)
                            })
                            .await?;
                        }
                        let current = node
                            .residents
                            .about(&resident)
                            .is_some_and(|(about, _)| about.session.as_deref() == Some(&session));
                        if !current && matches!(event.happened, Happened::TurnStarted { .. } | Happened::Health { .. })
                        {
                            let directory = node.world.conversation(&resident);
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
                        crate::inbox::reclaim(&node.world.conversation(&resident), &ids)?;
                        taken(node, &resident, &session, &turn, |pending| {
                            Ok(ids.iter().any(|id| id == pending.delivery.id.as_str()))
                        })
                        .await?;
                        exchange::acknowledge_handover(directory, &ids, &turn)?;
                    }
                    exchange::Reported::Replaced { ids } => {
                        // Told again before they went in: they never will.
                        let conversation = node.world.conversation(&resident);
                        for id in ids {
                            if let Ok(id) = fastctx_types::NoticeId::try_from(id.as_str()) {
                                crate::inbox::settle(node, &conversation, &id)?;
                            }
                        }
                    }
                    exchange::Reported::Stored { uuid } => {
                        node.claude.state.lock().expect("never poisoned").stored.insert(session.clone(), uuid);
                    }
                    exchange::Reported::Thought { uuid, turn, item } => match exchange::transcript(directory)? {
                        Some(path) => {
                            let claim = thoughts::Claim { resident: resident.clone(), turn, item };
                            if let Some(ready) = node.claude.thoughts.claim(&path, uuid, claim) {
                                thoughts::show(node, ready).await?;
                            }
                        }
                        None => node
                            .log
                            .line(format_args!("{session} ended thinking in a record whose file its mod never named")),
                    },
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
                            ended(node, &resident, &session, reason, instance).await?;
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
                    crate::inbox::reclaim(&node.world.conversation(&resident), std::slice::from_ref(&id))
                {
                    node.log.line(format_args!("taking back the doubt about {id}: {error}"));
                }
                if let Err(error) =
                    taken(node, &resident, &session, &turn, |pending| Ok(pending.delivery.id.as_str() == id)).await
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
    // Births come before the session's own lock: it is let go and taken
    // again behind them (`fastctx_platform::doing::Rank`).
    drop(_reading);
    let _births = node.claude.births.write().await;
    let _reading = reading.lock().await;
    let registration = node
        .claude
        .state
        .lock()
        .expect("never poisoned")
        .sessions
        .get(&session)
        .map(|entry| entry.registration.clone());
    let Some(registration) = registration else { return };
    match pairing::present(node, &registration) {
        Ok(false) => {}
        Ok(true) => return,
        Err(error) => {
            node.log.line(format_args!("checking {session}'s lifetime: {error}"));
            return;
        }
    }
    if let Err(error) =
        ended(node, &resident, &session, Some("its host activation ended".into()), registration.instance.clone()).await
    {
        node.log.line(format_args!("settling {session}: {error}"));
        return;
    }
    let mut state = node.claude.state.lock().expect("never poisoned");
    if state.sessions.get(&session).is_some_and(|entry| entry.registration == registration) {
        state.sessions.remove(&session);
    }
    if state.sessions.is_empty() {
        state.sessions = HashMap::new();
    }
    node.claude.changed.notify_waiters();
}

// Register synchronously with the caller, without depending on a filesystem
// watcher having run. Registration, alias persistence and publication are one
// serialized operation; report consumption holds a separate per-session lock.
async fn hear(node: &Arc<Node>, registration: exchange::Registration) {
    let _doing = fastctx_platform::doing::doing(format!("registering Claude session {}", registration.session));
    if let Err(error) = pairing::validate_session(&registration.session) {
        node.log.line(format_args!("registering a Claude session: {error}"));
        return;
    }
    if node.residents.deleted_session(&registration.session).is_some() {
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
                    None,
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
    let guard = match &speaking {
        Some(lock) => Some(lock.lock().await),
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
                state.sessions.insert(session.clone(), Session { registration, resident: None });
                true
            }
        }
    };
    drop(guard);
    if changed {
        node.claude.changed.notify_waiters();
    }
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
    which: impl Fn(&Pending) -> std::io::Result<bool>,
) -> std::io::Result<()> {
    if !node.residents.knows(resident) {
        return Ok(());
    }
    let directory = node.world.conversation(resident);
    let Some(speaking) = node.residents.speaking(resident) else { return Ok(()) };
    let _speaking = speaking.lock().await;
    crate::delivery::remember_session(&directory, turn, session)?;
    let mut kept = Vec::new();
    for pending in crate::inbox::pending(&directory)? {
        if pending.session.as_deref().is_none_or(|bound| bound == session) && which(&pending)? {
            kept.push(pending);
        }
    }
    if kept.is_empty() {
        return Ok(());
    }
    let mut audience = Vec::new();
    if kept.iter().any(|pending| pending.working) && crate::goals::replies::inherits(node, resident)? {
        audience = node.residents.readers(resident, turn);
        if audience.is_empty() {
            if let Some(finished) = crate::delivery::terminal(&directory, turn)? {
                audience = finished.readers;
            } else if node.residents.about(resident).is_some_and(|(about, _)| about.session.as_deref() == Some(session))
            {
                audience = node.residents.working_for(resident);
            }
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

/// Words entrusted to `session` whose acceptance is unknown now: those
/// named by `ids`, or, as the host activation that registered at `ended`
/// ended, the words that activation had.
async fn uncertain_inputs(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    ids: Option<&[String]>,
    why: String,
    ended: Option<Timestamp>,
) -> std::io::Result<()> {
    if !node.residents.knows(resident) {
        return Ok(());
    }
    let directory = node.world.conversation(resident);
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
        .filter(|pending| had(pending, ended))
        .collect::<Vec<_>>();
    let mut unresolved = Vec::new();
    for input in pending {
        if matches!(input.state, PendingState::Waking | PendingState::Handed)
            && (input.state == PendingState::Handed
                || crate::lifecycle::retry::progress(node, resident, &input.delivery.id)?
                    .is_none_or(|progress| progress.accepted))
            && startup::untaken(node, session, &directory, &input.delivery.id)?
        {
            startup::retry_untaken(node, resident, session, &input.delivery.id)?;
        } else {
            unresolved.push(input);
        }
    }
    if unresolved.is_empty() {
        return Ok(());
    }
    let mut handover = crate::delivery::begin(&directory, unresolved)?;
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
            ids.is_none().then(|| serde_json::json!({"world":"host_exited"})),
        )
        .await?;
    }
    Ok(())
}

/// Whether the host activation that registered at `registered` had the
/// words `pending`: words handed to a running host are the activation's
/// running then, words sent to wake the session the one that came up after.
/// The session keeps its id across activations, so the id alone does not
/// tell; words kept before this was recorded go with the session.
fn had(pending: &Pending, registered: Option<Timestamp>) -> bool {
    match (pending.state, pending.since, registered) {
        (PendingState::Waiting, _, _) => false,
        (PendingState::Waking, Some(since), Some(registered)) => since <= registered,
        (PendingState::Handed, Some(since), Some(registered)) => since >= registered,
        _ => true,
    }
}

/// The resident's session ended: a turn it ran ends with it, and it sleeps
/// until words wake it.
async fn ended(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    reason: Option<String>,
    instance: Option<String>,
) -> std::io::Result<()> {
    ended_for_process(node, resident, session, reason, None, instance.map(Some)).await
}

async fn ended_for_process(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    reason: Option<String>,
    expected: Option<&fastctx_types::ProcessIdentity>,
    instance: Option<Option<String>>,
) -> std::io::Result<()> {
    // One whose conversation was deleted has no turn left to end.
    let Some(speaking) = node.residents.speaking(resident) else {
        control::ended(node, session);
        return Ok(());
    };
    let _speaking = speaking.lock().await;
    let registration =
        node.claude.state.lock().expect("never poisoned").sessions.get(session).map(|entry| entry.registration.clone());
    let registration = match registration {
        Some(registration) => Some(registration),
        None => exchange::registration(&exchange::sessions(node.world.path()).join(session))?,
    };
    if let Some(expected) = expected
        && registration.as_ref().and_then(|entry| entry.process.as_ref()) != Some(expected)
    {
        return Ok(());
    }
    if let Some(instance) = instance
        && registration.as_ref().is_none_or(|entry| entry.instance != instance)
    {
        return Ok(());
    }
    // Terminal records are idempotent. Reconcile late inbox entries too:
    // an older node could hand them to an activation it had already ended.
    control::ended(node, session);
    let Some((about, _)) = node.residents.about(resident) else { return Ok(()) };
    let directory = node.world.conversation(resident);
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
            facts: None,
            error: Some(serde_json::json!({"world":"host_exited"})),
            how: fastctx_types::session::TurnHow::Interrupted,
            reason: Some(why),
            duration_ms: None,
            last_message: None,
        };
        crate::turns::finish_locked(node, resident, &turn, end, &[]).await?;
    }
    uncertain_inputs(
        node,
        resident,
        session,
        None,
        "the conversation ended before the host identified which turn consumed these words; their acceptance is unknown"
            .to_string(),
        registration.as_ref().and_then(|registration| registration.at),
    )
    .await?;
    if current {
        let departed = crate::lifecycle::observed(node, resident, false, "activation ended");
        if departed && reason.as_deref() != Some("clear") {
            crate::colleagues::left(node, resident.clone());
        }
    }
    if let Some(registration) = registration {
        pairing::record_ended(node, &registration)?;
    }
    calls::sweep(node);
    node.claude.changed.notify_waiters();
    Ok(())
}

/// Asks the paired mod to stop this resident's current turn.
pub fn stop(node: &Node, resident: &ResidentId, turn: &str) -> Result<bool, Failure> {
    let session: String = node.claude.state.lock().expect("never poisoned").paired.get(resident).ok_or_else(|| {
        Failure::new(format!("{resident}'s session has not been heard from the World's plugin, which is what stops it"))
    })?;
    if node.residents.now(resident).and_then(|now| now.turn).as_deref() != Some(turn) {
        return Ok(false);
    }
    let directory = exchange::sessions(node.world.path()).join(&session);
    exchange::stop(&directory, turn).map_err(|error| Failure::new(format!("asking its plugin to stop: {error}")))?;
    Ok(true)
}

/// Whether the resident's Claude session is paired with its plugin now.
pub fn paired(node: &Node, resident: &ResidentId) -> bool {
    node.claude.state.lock().expect("never poisoned").paired.contains(resident)
}

pub fn removed(node: &Node, resident: &ResidentId) {
    let mut state = node.claude.state.lock().expect("never poisoned");
    state.sessions.retain(|_, session| session.resident.as_ref() != Some(resident));
    state.paired.remove(resident);
    if state.sessions.is_empty() {
        state.sessions = HashMap::new();
    }
    node.claude.changed.notify_waiters();
}

/// Resolves this call's own evidence, including before the watcher has seen
/// its registration. No connection-wide or process-wide identity is reused.
pub async fn identify(node: &Arc<Node>, tool_use_id: &str) -> Result<fastctx_types::residents::Whereabouts, Failure> {
    let world = node.world.path().to_path_buf();
    let id = tool_use_id.to_string();
    let sessions: Vec<_> = node
        .claude
        .state
        .lock()
        .expect("never poisoned")
        .sessions
        .keys()
        .map(|session| exchange::sessions(&world).join(session))
        .collect();
    let proof = tokio::task::spawn_blocking(move || claude::calls::take_from(&world, &id, sessions))
        .await
        .map_err(|error| Failure::new(format!("reading Claude call evidence: {error}")))?
        .map_err(|error| Failure::new(format!("pairing Claude call {tool_use_id}: {error}")))?;
    let (registration, subagent) = (proof.registration, proof.agent.is_some());
    pairing::validate_session(&registration.session)
        .map_err(|error| Failure::new(format!("pairing Claude call {tool_use_id}: {error}")))?;
    let session = registration.session.clone();
    hear(node, registration).await;
    // The reports preceding the call may establish its turn and deliveries.
    look(node, &exchange::sessions(node.world.path()).join(&session)).await;
    let alias = exchange::sessions(node.world.path()).join(&session).join("resident.json");
    let resident: ResidentId =
        serde_json::from_slice(&std::fs::read(alias).map_err(|error| Failure::new(error.to_string()))?)
            .map_err(|error| Failure::new(error.to_string()))?;
    node.residents.active(&resident)?;
    let town =
        node.residents.town_of(&resident).ok_or_else(|| Failure::new("the paired Claude resident is missing"))?;
    Ok(fastctx_types::residents::Whereabouts { resident, town, machine: None, subagent })
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
    let unknown = crate::adoption::Arrival { login: crate::adoption::Login::Unknown, own: false };
    let resident = match pairing::resident(node, &registration, &unknown) {
        Ok(resident) => resident,
        Err(error) => {
            node.log.line(format_args!("pairing Claude session {session}: {error}"));
            return;
        }
    };
    let Ok(gate) = node.residents.management(&resident) else { return };
    let _reading = gate.read().await;
    // Removed, or deleted while this waited for the gate: its session is no
    // one's to pair.
    let speaking = node.residents.speaking(&resident).filter(|_| node.residents.removed(&resident).is_none());
    let Some(speaking) = speaking else {
        node.claude.state.lock().expect("never poisoned").sessions.remove(session);
        node.claude.changed.notify_waiters();
        return;
    };
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
            && let Some(now) = node.residents.now(&resident)
        {
            let directory = node.world.conversation(&resident);
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
    let present = match pairing::present(node, &registration) {
        Ok(present) => present,
        Err(error) => {
            node.log.line(format_args!("checking Claude presence: {error}"));
            process::observe(node, registration.process);
            return;
        }
    };
    if newer {
        crate::lifecycle::observed(node, &resident, present, "activation registered");
    }
    // What waited for the resident before its mod was there goes to the mod,
    // and stays kept until the mod says which turn took it.
    if !node.residents.knows(&resident) {
        return;
    }
    let directory = node.world.conversation(&resident);
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
            // Registration records identity, not a second delivery path.
            // A fresh admission checks the activation after taking the action
            // gate, so a final report cannot feed a process that just exited.
            if kept.iter().any(|pending| pending.state == PendingState::Waiting)
                && (present || node.residents.about(&resident).is_some_and(|(about, _)| about.world_started))
            {
                crate::lifecycle::retry::kick(node, &resident);
            }
            if present
                && newer
                && kept
                    .iter()
                    .any(|pending| pending.state == PendingState::Waiting && pending.delivery.notice.along().is_some())
            {
                let (node, resident) = (node.clone(), resident.clone());
                tokio::spawn(async move { crate::speech::along(&node, &resident).await });
            }
        }
        Ok(Err(error)) => node.log.line(format_args!("reading what waits for {resident}: {error}")),
        Err(_) => {}
    }
    drop(guard);
    node.claude.changed.notify_waiters();
    process::observe(node, registration.process);
}

/// Hands something kept to a resident whose Claude session the World
/// hears; `None` when it hears none for it.
pub async fn deliver(
    node: &Arc<Node>,
    to: &ResidentId,
    about: &About,
    directory: &ConversationDir,
    pending: &Pending,
) -> Option<Result<Landed, Failure>> {
    let delivering = node.claude.delivering.get_or_insert_with(to, || crate::ranked::delivering(to));
    let _delivering = delivering.lock().await;
    match crate::inbox::read(directory, &pending.delivery.id) {
        Ok(Some(current)) if current.state == PendingState::Waiting => {}
        Ok(_) => return Some(Ok(Landed::Handed { id: pending.delivery.id.clone() })),
        Err(error) => return Some(Err(Failure::new(error.to_string()))),
    }
    match fork_ready::pending(node, to) {
        Ok(Some(why)) => return Some(Ok(Landed::Waiting { why: Some(why), answered: true })),
        Err(error) => return Some(Err(error)),
        Ok(None) => {}
    }
    // The user's own session (`world-layers.md`): nothing goes into it and
    // nothing wakes it; what is kept for it waits for its next call.
    if !about.world_managed() {
        return Some(Ok(Landed::Waiting { why: None, answered: false }));
    }
    let session: String = node
        .claude
        .state
        .lock()
        .expect("never poisoned")
        .paired
        .get(to)
        .or_else(|| (about.host.agent.as_str() == "claude").then(|| about.session.clone()).flatten())?;
    if let Err(error) = crate::inbox::target(directory, &pending.delivery.id, &session) {
        return Some(Err(Failure::new(error.to_string())));
    }
    let alive = match alive(node, about.home.as_deref(), &session).await {
        Ok(alive) => alive,
        Err(error) => return Some(crate::lifecycle::retry::defer(node, to, &pending.delivery.id, &error.message)),
    };
    let stopped_background = if !about.world_started && !alive {
        match management::background_session(node, about.home.as_deref(), &session).await {
            Ok(found) => found.is_some(),
            Err(error) => return Some(Err(error)),
        }
    } else {
        false
    };
    let current = match node.residents.about(to) {
        Some((about, _)) => about,
        None => return Some(Err(crate::authorization::no_subject())),
    };
    if let Err(error) = crate::lifecycle::wake::check(node, to, &current) {
        return Some(crate::lifecycle::retry::unavailable(node, to, &pending.delivery.id, error).await);
    }
    let user_action =
        matches!(pending.delivery.notice, Notice::Words { from: Speaker::User { .. }, .. } | Notice::Attached { .. });
    if !about.world_started && !stopped_background && !user_action && !pending.delivery.notice.wakes_closed() {
        // Unscheduled facts wait until the foreground session is opened again.
        return Some(if alive {
            into_inbox(node, to, &session, directory, pending).await
        } else {
            Ok(Landed::Waiting { why: None, answered: true })
        });
    }
    // A turn these words would start waits for room in its login
    // (`turn_slots`); words for a running turn go in with it.
    if pending.delivery.notice.along().is_none()
        && node.residents.now(to).is_none_or(|now| now.turn.is_none() && now.activity != Activity::Busy)
        && let Err(queued) = crate::turn_slots::admit(node, to, &current)
    {
        crate::lifecycle::retry::kick(node, to);
        return Some(Ok(queued.landed()));
    }
    Some(match alive {
        true => into_inbox(node, to, &session, directory, pending).await,
        // What rides with the next turn wakes nobody: it waits for the turn
        // that wakes it, and goes in then (`speech::along`).
        false if pending.delivery.notice.along().is_some() => Ok(Landed::Waiting { why: None, answered: true }),
        // Its supervisor stopped it: the words wake it, as they are, and its
        // next turn takes them.
        false => {
            match crate::inbox::pending(directory) {
                Ok(rows)
                    if rows
                        .iter()
                        .any(|row| row.state == PendingState::Waking && row.delivery.id != pending.delivery.id) =>
                {
                    crate::lifecycle::retry::kick(node, to);
                    return Some(Ok(Landed::Waiting { why: None, answered: true }));
                }
                Err(error) => return Some(Err(Failure::new(error.to_string()))),
                _ => {}
            }
            let (text, as_user) = words(&pending.delivery.notice, Some(&node.machine()));
            if as_user {
                node.claude.handing(to, &text);
            }
            let claimed: std::io::Result<bool> = {
                let Some(speaking) = node.residents.speaking(to) else {
                    return Some(Err(crate::authorization::no_subject()));
                };
                let _speaking = speaking.lock().await;

                (|| {
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
                    // Ended runtimes can have left the in-memory registry.
                    // Their durable registration still identifies the predecessor.
                    let previous = exchange::registration(&exchange::sessions(node.world.path()).join(&session))?
                        .and_then(|registration| registration.instance);
                    crate::lifecycle::retry::begin_after(node, to, &pending.delivery.id, previous)
                        .map_err(std::io::Error::other)?;
                    crate::inbox::set_state(directory, &pending.delivery.id, PendingState::Waking)?;
                    Ok(true)
                })()
            };
            match claimed {
                Ok(true) => {
                    match crate::lifecycle::wake::wake(
                        node,
                        to,
                        crate::lifecycle::wake::Source::of(&pending.delivery.notice),
                        Some((pending.delivery.id.clone(), if as_user { text } else { claude::framed(&text) })),
                    )
                    .await
                    {
                        Ok(_) => Ok(Landed::Handed { id: pending.delivery.id.clone() }),
                        Err(error) if !error.attempted => {
                            crate::lifecycle::retry::unavailable(node, to, &pending.delivery.id, error).await
                        }
                        Err(error) => {
                            startup::unknown(node, to, &pending.delivery.id, error.failure.message);
                            Ok(Landed::Handed { id: pending.delivery.id.clone() })
                        }
                    }
                }
                Ok(false) => Ok(Landed::Waiting {
                    why: Some("these words already have a handover in progress".to_string()),
                    answered: true,
                }),
                Err(error) => Err(Failure::new(format!("keeping what wakes {to}: {error}"))),
            }
        }
    })
}

async fn failed_wake(
    node: &Arc<Node>,
    resident: &ResidentId,
    directory: &ConversationDir,
    id: &fastctx_types::NoticeId,
    error: Failure,
) -> Result<Landed, Failure> {
    let speaking = node.residents.speaking(resident).ok_or_else(|| Failure::new(format!("{resident} was deleted")))?;
    let _speaking = speaking.lock().await;
    if let Some(turn) = crate::delivery::input_turn(directory, id).map_err(|error| Failure::new(error.to_string()))? {
        node.log.line(format_args!("the Claude command failed after its turn was identified: {error}"));
        crate::lifecycle::retry::clear(node, resident, id).map_err(|error| Failure::new(error.to_string()))?;
        return Ok(Landed::NewTurn { turn });
    }
    let why = format!("the Claude command failed before identifying a consuming turn: {error}");
    if let Some(pending) = crate::inbox::read(directory, id).map_err(|error| Failure::new(error.to_string()))? {
        let mut handover = crate::delivery::begin(directory, vec![pending])
            .map_err(|error| Failure::new(format!("recording interrupted Claude startup: {error}")))?;
        crate::turns::unresolved(
            node,
            resident,
            directory,
            &mut handover,
            fastctx_types::session::TurnHow::Interrupted,
            why.clone(),
            Some(serde_json::json!({"world":"host_exited"})),
        )
        .await
        .map_err(|error| Failure::new(format!("settling interrupted Claude startup: {error}")))?;
    }
    Ok(Landed::Uncertain { why })
}

/// Puts something kept into the mod's inbox; it is kept until the mod says
/// which turn took it.
async fn into_inbox(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    directory: &ConversationDir,
    pending: &Pending,
) -> Result<Landed, Failure> {
    let speaking = node.residents.speaking(resident).ok_or_else(crate::authorization::no_subject)?;
    let _speaking = speaking.lock().await;
    let session_directory = exchange::sessions(node.world.path()).join(session);
    let registration = exchange::registration(&session_directory).map_err(|error| Failure::new(error.to_string()))?;
    let live = registration
        .as_ref()
        .map(|registration| pairing::present(node, registration))
        .transpose()
        .map_err(|error| Failure::new(error.to_string()))?
        .unwrap_or(false);
    if !live {
        if let Some(registration) = registration {
            process::observe(node, registration.process);
        }
        crate::lifecycle::retry::begin(node, resident, &pending.delivery.id)?;
        crate::lifecycle::retry::kick(node, resident);
        return Ok(Landed::Waiting { why: None, answered: true });
    }
    if let Some(why) = fork_ready::pending(node, resident)? {
        return Ok(Landed::Waiting { why: Some(why), answered: true });
    }
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
    let (text, as_user) = words(&pending.delivery.notice, Some(&node.machine()));
    if as_user {
        node.claude.handing(resident, &text);
    }
    // Handed over before and not in yet, what this tells again goes in no
    // more: the mod drops what it still holds of it.
    let along = pending.delivery.notice.along();
    let replaces: Vec<String> = match along {
        Some(kind) => crate::inbox::pending(directory)
            .map_err(|error| Failure::new(error.to_string()))?
            .into_iter()
            .filter(|kept| {
                kept.delivery.id != pending.delivery.id
                    && kept.state == PendingState::Handed
                    && kept.delivery.notice.along() == Some(kind)
                    && kept.delivery.at <= pending.delivery.at
            })
            .map(|kept| kept.delivery.id.as_str().to_string())
            .collect(),
        None => Vec::new(),
    };
    let how = exchange::Delivering { at: Some(pending.delivery.at), as_user, along, replaces: &replaces };
    // Commit the intent and its timestamp before the plugin can consume it.
    // Replaying a Handed intent republishes the same id and preserves its time.
    if pending.state != PendingState::Handed {
        crate::inbox::set_state(directory, &pending.delivery.id, PendingState::Handed)
            .map_err(|error| Failure::new(error.to_string()))?;
    }
    exchange::deliver(&session_directory, pending.delivery.id.as_str(), &text, how)
        .map(|()| Landed::Handed { id: pending.delivery.id.clone() })
        .map_err(|error| Failure::new(format!("handing it to the session's plugin: {error}")))
}

/// The text a notice is in a Claude session, and whether it is the user's
/// own words, which go in as the user's.
fn words(notice: &Notice, here: Option<&fastctx_types::MachineName>) -> (String, bool) {
    match notice {
        Notice::Words { from: Speaker::User { .. }, text, .. } => (text.clone(), true),
        Notice::Attached { message, text } => (message.render(text), true),
        notice => (told::told(notice, here), false),
    }
}

/// Whether the session's process is alive, to hand it words: its mod named
/// that process when it registered, so no `claude` command runs for each
/// delivery. A session not heard since this node started is asked of
/// Claude Code itself, and so is one whose registered process is gone:
/// Claude Code's daemon may have started it again, its mod not registered
/// yet, and waking it then would start a copy.
async fn alive(node: &Arc<Node>, home: Option<&Path>, session: &str) -> Result<bool, Failure> {
    let registration =
        node.claude.state.lock().expect("never poisoned").sessions.get(session).map(|entry| entry.registration.clone());
    if let Some(registration) = registration
        && pairing::present(node, &registration)
            .map_err(|error| Failure::new(format!("checking whether the Claude session runs: {error}")))?
    {
        return Ok(true);
    }
    running(node, home, session).await
}

/// Whether the background session, of the home `home`, has a running
/// process.
async fn running(node: &Arc<Node>, home: Option<&Path>, session: &str) -> Result<bool, Failure> {
    let Command { program, cwd, env } = command(node, home)?;
    let session = session.to_string();
    tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        let args: Vec<OsString> = vec!["agents".into(), "--json".into(), "--all".into()];
        let listed = background::claude(&program, &args, &cwd, &env)?;
        background::running(&listed, &session)
    }))
    .await
    .map_err(|error| Failure::new(format!("asking Claude Code about its sessions: {error}")))?
    .map_err(Failure::new)
}

pub(crate) async fn wake(
    node: &Arc<Node>,
    resident: &ResidentId,
    home: Option<&Path>,
    session: &str,
    words: String,
    id: fastctx_types::NoticeId,
) -> Result<Landed, crate::lifecycle::wake::WakeFailure> {
    let mut launching = command(node, home)?;
    launching.cwd = resume_dir(node, session);
    oldest::check(node, &launching).await?;

    let Command { program, cwd, env } = launching;
    if let Some(resident) = node.residents.resident_of_thread(session) {
        crate::profile::session::claude(node, &resident, false)?;
    }
    let args = background::wake_args(session, &words);
    let (launch_node, launch_resident, launch_id) = (node.clone(), resident.clone(), id.clone());
    // A session that reports while this runs waits to learn whether it is a copy.
    let waking = node.claude.copies.waking();
    let printed = tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        use crate::lifecycle::wake::WakeFailure;
        trust(&[cwd.as_path()], &env).map_err(Failure::new)?;
        crate::lifecycle::retry::attempting(&launch_node, &launch_resident, &launch_id)?;
        background::invoke(&program, &args, &cwd, &env, std::time::Duration::from_secs(120)).map_err(|error| {
            WakeFailure { failure: Failure::new(error.message), attempted: error.started, permanent: false }
        })
    }))
    .await
    .map_err(|error| crate::lifecycle::wake::WakeFailure {
        failure: Failure::new(format!("waking the session: {error}")),
        attempted: true,
        permanent: false,
    })??;
    // The session was still running, so Claude Code copied it instead: the
    // copy is stopped, and the words, which never reached the session they
    // were for, wait to be handed to it again.
    if let Some(copy) = copies::of(session, &printed) {
        copies::discard(node, home, &copy).await;
        return Err(crate::lifecycle::wake::WakeFailure {
            failure: Failure::new(format!(
                "{session} was still running; Claude Code started a copy of it instead, which was stopped"
            )),
            attempted: false,
            permanent: false,
        });
    }
    drop(waking);
    crate::lifecycle::retry::accepted(node, resident, &id).map_err(|failure| crate::lifecycle::wake::WakeFailure {
        failure,
        attempted: true,
        permanent: false,
    })?;
    // Claude Code took the words; the session that takes them may still end
    // before the World's plugin pairs, which only watching it tells.
    startup::observe(node, resident, id.clone(), true);
    Ok(Landed::Handed { id })
}

/// Where a session is resumed from: the directory it was opened in, as its
/// registration says, so its own tools go on working there; the World's
/// directory where it was opened in the user's home, which Claude Code
/// cannot keep trusted, or where nothing says.
pub(crate) fn resume_dir(node: &Node, session: &str) -> PathBuf {
    let opened = exchange::registration(&exchange::sessions(node.world.path()).join(session))
        .ok()
        .flatten()
        .and_then(|registration| registration.cwd)
        .map(PathBuf::from)
        .filter(|directory| directory.is_dir());
    let home = fastctx_platform::user_home().ok();
    match opened {
        Some(directory) if home.as_deref().is_none_or(|home| !crate::homes::same_dir(&directory, home)) => directory,
        _ => node.world.path().to_path_buf(),
    }
}

/// The `claude` command, where to run it, and the environment it gets: the
/// user's own, as for every program the node starts, naming the home of the
/// account it works in when that is not the default one: a session, its
/// supervisor and every command about it must see the same home.
#[derive(Clone)]
struct Command {
    program: PathBuf,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
}

fn command(node: &Node, home: Option<&Path>) -> Result<Command, Failure> {
    let mut env = environment()?;
    if let Some(home) = home {
        crate::homes::listed(node, "claude", Some(home))?;
        env.retain(|(name, _)| !name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"));
        env.push(("CLAUDE_CONFIG_DIR".into(), home.as_os_str().to_owned()));
    }
    let program = self::program().ok_or_else(|| {
        Failure::new(format!("Claude Code is not installed on {}: no claude command", node.machine()))
    })?;
    let cwd =
        fastctx_platform::user_home().map_err(|error| Failure::new(format!("finding the home directory: {error}")))?;
    Ok(Command { program, cwd, env })
}

/// The `claude` command the World runs, for every Claude Code command it
/// runs: the first on the node's search path, looked up each time.
pub(crate) fn program() -> Option<PathBuf> {
    background::find(std::env::var_os("PATH").as_ref())
}

pub(crate) fn environment() -> Result<Vec<(OsString, OsString)>, Failure> {
    let mut env = fastctx_platform::world_process_environment()
        .map_err(|error| Failure::new(format!("reading the user's environment: {error}")))?;
    // A `claude --bg` may start Claude Code's daemon, whose environment every
    // later background session gets, the user's own included. Nothing of the
    // World goes into it: sessions are known by their ids, and the plugin
    // names its World itself.
    env.retain(|(name, _)| {
        !["FASTCTX_RESIDENT", "FASTCTX_HOME", "FASTCTX_COLLEAGUE"].iter().any(|world| name.eq_ignore_ascii_case(world))
    });
    Ok(env)
}

/// Opens a Claude Code background session on this machine for the user
/// typing on `typed_on`, with their words as its first.
pub async fn open_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    opened: crate::talk::Opened,
    town: Option<TownName>,
    text: String,
    options: fastctx_types::agents::SessionOptions,
    attachments: Vec<fastctx_types::attachments::Attachment>,
) -> Result<(ResidentId, Landed), Failure> {
    if let Some(town) = &town {
        crate::town::access::ready(node, &opened.acting, town).await?;
    }
    // Nothing of the conversation is made for a Claude Code that cannot
    // run it.
    let launching = command(node, opened.home.as_deref())?;
    oldest::check(node, &launching).await?;
    let agent = AgentName::try_from("claude").expect("a valid agent name");
    let (id, _opening) = crate::attachments::opening(node, &agent, &opened.acting, town.as_ref(), &attachments).await?;
    let _birth = crate::here::born(node, &id, opened.acting.clone(), &Origin::User);
    let Command { program, env, .. } = launching.clone();
    // A login assisting another account starts without the browser of this
    // machine (`background::Start::assisting`).
    crate::profile::session::ready(node, &opened.acting).await?;
    let assisting = crate::assists::host::starting(node, &id, &opened.acting)?;
    let (words, heard) =
        crate::colleagues::instructions(node, &id, town.as_ref(), &Origin::User, &opened.acting, false).await?;
    // What it is told of its machines and folders its plugin adds to the
    // first message's context, from the session's own directory.
    crate::focus::open(node, &node.world.conversation(&id), &opened.acting, &opened.focus).await?;
    let born_at = Timestamp::now();
    let about = About {
        side: false,
        forked_from: None,
        mcp_off: options.mcp_off.clone(),
        id: id.clone(),
        host: Host { agent, client: "claude-code".to_string(), version: String::new() },
        machine: node.machine(),
        born_at,
        origin: Origin::User,
        session: None,
        world_started: true,
        native_resumed: false,
        model: None,
        acting: opened.acting,
        focus: opened.focus,
        home: opened.home,
        callee: None,
    };
    let now = Now {
        activity: Activity::Busy,
        since: born_at,
        turn: None,
        last_turn: None,
        owed: Default::default(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
        first_words: None,
        last_said: None,
        last_compacted: false,
        last_heard_user: None,
        asked: None,
        told: None,
    };
    if let Some(town) = &town {
        crate::town::access::check(node, &about.acting, town)?;
    }
    node.residents
        .create(&node.world, about, town.clone(), now)
        .map_err(|error| Failure::new(format!("recording the session: {error}")))?;
    crate::context::instructed(node, &id, &heard).map_err(|error| Failure::new(error.to_string()))?;
    node.residents.set_instructed(&id, heard);
    crate::session_options::keep(node, &id, &options)?;
    let directory = node.world.conversation(&id);
    let settings = directory.claude_settings();
    let instructions = directory.claude_instructions();
    let written = std::fs::write(&settings, settings_json(&id)).and_then(|()| std::fs::write(&instructions, words));
    written.map_err(|error| Failure::new(format!("writing the session's settings: {error}")))?;
    let mcp = crate::profile::session::claude(node, &id, true)?;
    let cwd = crate::colleagues::standing(node, town.as_ref())?;
    let plugin = claude::plugin_dir(node.world.path());
    // Its files other than images go where the user chose, as later words'
    // do.
    let attachments = crate::attachments::place(node, &id, attachments, opened.files_on.as_ref()).await?;
    // The first words go in as the prompt the session starts with, which it
    // reads as the user's: they are. They are kept until its first turn
    // takes them, so whoever follows them knows which turn that is.
    let first = Pending {
        delivery: crate::attachments::delivery(typed_on, text, attachments, None),
        readers: Vec::new(),
        state: PendingState::Waking,
        working: false,
        session: None,
        // The session it starts is the first to have them.
        since: Some(Timestamp::now()),
    };
    crate::inbox::keep(&directory, &first)
        .map_err(|error| Failure::new(format!("keeping the first words: {error}")))?;
    let text = self::words(&first.delivery.notice, Some(&node.machine())).0;
    node.claude.handing(&id, &text);
    let world = node.world.path().to_path_buf();
    // Until the World knows which session this is, a Claude session's MCP
    // connection it cannot place waits, rather than becoming someone new.
    let _birth = node.claude.births.read().await;
    let started = tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        let start = background::Start {
            plugin: &plugin,
            settings: &settings,
            mcp: &mcp,
            instructions: &instructions,
            first_words: &text,
            model: options.model.as_deref(),
            reasoning_effort: options.reasoning_effort.as_deref(),
            assisting,
        };
        let args = background::start_args(&start);
        trust(&[world.as_path(), cwd.as_path()], &env)?;
        match background::claude(&program, &args, &cwd, &env) {
            // A Claude Code that was running can write its state back over
            // the trust a moment after it was written; a new session refused
            // for exactly that was not created, so it is started once more.
            Err(why) if why.contains(background::UNTRUSTED) => {
                trust(&[world.as_path(), cwd.as_path()], &env)?;
                background::claude(&program, &args, &cwd, &env)
            }
            started => started,
        }
    }))
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
    startup::observe(node, &id, first.delivery.id.clone(), false);
    Ok((id, Landed::Handed { id: first.delivery.id }))
}

/// The World's directories where its sessions stand are trusted in Claude
/// Code, so it starts sessions there without asking (`world-projects.md`):
/// by the home the session starts with in `env`. Each gets a record of its
/// own. Claude Code finds trust by walking up from where a session stands,
/// but it also writes an untrusted record of its own for each directory a
/// session opens in, so the World does not lean on what a version reads
/// past that.
fn trust(directories: &[&Path], env: &[(OsString, OsString)]) -> Result<(), String> {
    let home =
        env.iter().find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR")).map(|(_, value)| value.as_os_str());
    for directory in directories {
        claude::trust::ensure_trusted(directory, home)
            .map_err(|error| format!("trusting {} in Claude Code: {error}", directory.display()))?;
    }
    Ok(())
}

/// Takes back the trust `trust` gave `directory`, and whatever lies inside
/// it, in the login whose home is `home` (the user's default when `None`).
pub(crate) fn untrust(directory: &Path, home: Option<&Path>) -> Result<(), String> {
    let env = environment().map_err(|failure| failure.message)?;
    let configured = match home {
        Some(home) => Some(home.as_os_str().to_owned()),
        None => {
            env.into_iter().find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR")).map(|(_, value)| value)
        }
    };
    let key = claude::trust::uninstall_key(directory).map_err(|error| error.to_string())?;
    claude::trust::remove_world_trust(configured.as_deref(), &key).map(|_| ()).map_err(|error| error.to_string())
}

/// Settings for a background session the World starts for the conversation
/// `id`: the host's limit on a tool answer opened past the World's own
/// (`world-view.md`, 一次给多少), its limit on a tool's description opened so
/// the whole of `run` reaches the model, the host's own shell, file and search tools
/// turned off, since the World's take their place, the session working in
/// place rather than in a worktree of its own directory, which no repository
/// it edits on any machine lives in, and the conversation, which its plugin
/// reads back from them when it starts. The session runs without approvals
/// (`background::settings_args`), so nothing else needs allowing.
pub(crate) fn settings_json(id: &ResidentId) -> String {
    let mut settings = serde_json::json!({
        "env": {
            "MAX_MCP_OUTPUT_TOKENS": fastctx_hosts::claude::MAX_MCP_OUTPUT_TOKENS.to_string(),
            "CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH": fastctx_hosts::claude::MAX_MCP_DESCRIPTION_LENGTH.to_string(),
            "FASTCTX_CONVERSATION": id.as_str(),
        },
        "permissions": {
            // The World's own tools take these places: its shell and files,
            // the plan, the goal, `message_user` (`world-voice.md`), which
            // asks without stopping as well, and a sentinel for any later
            // wake-up (`world-senses.md`).
            "deny": ["Bash", "PowerShell", "Read", "Write", "Edit", "MultiEdit", "NotebookEdit", "Grep", "Glob",
                "TodoWrite", "TaskCreate", "TaskUpdate", "TaskList", "TaskGet", "ProposeGoal",
                "SendUserMessage", "AskUserQuestion", "PushNotification", "CronCreate", "CronDelete", "CronList",
                "ScheduleWakeup"],
        },
        "worktree": { "bgIsolation": "none" },
    });
    // Claude Code lets root skip approvals only in a sandbox declared as
    // one: where the World's conversations run as root, they are declared
    // so, and skip them as they do everywhere.
    if fastctx_platform::user_id() == Some(0) {
        settings["env"]["IS_SANDBOX"] = "1".into();
    }
    serde_json::to_string_pretty(&settings).expect("JSON values serialize")
}
