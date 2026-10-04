//! Claude sessions, as the node hears them through the World's plugin
//! (`world-colleagues.md`, 话怎样送到居民眼前).
//!
//! The node writes the plugin into the World's directory when it starts.
//! Every session the plugin runs in has a directory under `<world>/claude`,
//! which the node watches. A session's registration pairs it with its
//! resident: the one the World started it as, the one this session was
//! before, or the one whose MCP connection gave the same inbox address.
//! What it reports is read once it is paired, in the order it was written,
//! and goes to that resident; what waited for the resident goes to its mod.
//! What the World has for a paired session goes into its mod's inbox, and
//! the mod says which turn took it; a background session the World started
//! whose process the supervisor stopped is woken with it, and its next turn
//! takes it.
//!
//! Built against the mod API as measured on Claude Code 2.1.287 and run
//! against a stand-in host only; it has not yet run with Claude Code.

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
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;
use tokio::sync::mpsc;

use crate::Node;

#[derive(Default)]
pub struct Claude {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Session>,
    /// Resident to the session it is.
    paired: HashMap<ResidentId, String>,
}

struct Session {
    registration: exchange::Registration,
    resident: Option<ResidentId>,
}

/// Writes the plugin and starts watching the sessions.
pub fn start(node: &Arc<Node>) -> std::io::Result<()> {
    let fastctx = std::env::current_exe()?;
    claude::write_plugin(node.world.path(), &fastctx)?;
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
    let known = node.claude.state.lock().expect("never poisoned").sessions.contains_key(&session);
    if !known {
        match exchange::registration(directory) {
            Ok(Some(registration)) => {
                node.claude
                    .state
                    .lock()
                    .expect("never poisoned")
                    .sessions
                    .insert(session.clone(), Session { registration, resident: None });
                pair(node, &session).await;
            }
            // Not registered yet, or not whole yet: its next change comes
            // here again.
            Ok(None) => return,
            Err(error) => {
                node.log.line(format_args!("reading {}: {error}", directory.display()));
                return;
            }
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
    for reported in reported {
        match reported {
            exchange::Reported::Happened(event) => {
                if let Happened::TurnStarted { turn } = &event.happened {
                    taken(node, &resident, turn, |state| state == PendingState::Waking).await;
                }
                crate::turns::happened(node, &resident, event).await
            }
            exchange::Reported::Handed { ids, turn } => {
                taken(node, &resident, &turn, |_| true).await;
                let _ = ids;
            }
            exchange::Reported::Ended { reason } => ended(node, &resident, reason).await,
        }
    }
    if let Err(error) = exchange::handed_over(directory) {
        node.log.line(format_args!("clearing what {session}'s mod handed over: {error}"));
    }
}

/// What the World kept for the resident that its session took into `turn`:
/// whoever it was for is owed that turn's end. Which are taken is said by
/// the mod, by id; `which` picks among those kept by how far they came.
async fn taken(node: &Arc<Node>, resident: &ResidentId, turn: &str, which: impl Fn(PendingState) -> bool) {
    let Some(town) = node.residents.town_of(resident) else { return };
    let directory = node.world.resident(town.as_ref(), resident);
    let kept = {
        let directory = directory.clone();
        tokio::task::spawn_blocking(move || crate::inbox::pending(&directory)).await
    };
    let kept = match kept {
        Ok(Ok(kept)) => kept,
        Ok(Err(error)) => {
            node.log.line(format_args!("reading what is on its way to {resident}: {error}"));
            return;
        }
        Err(_) => return,
    };
    let Some(speaking) = node.residents.speaking(resident) else { return };
    let _speaking = speaking.lock().await;
    for pending in kept.into_iter().filter(|pending| which(pending.state)) {
        if let Err(error) = node.residents.owe(&node.world, resident, turn, &pending.readers) {
            node.log.line(format_args!("recording who is owed {resident}'s turn: {error}"));
            continue;
        }
        match crate::speech::settle(&directory, &pending.delivery.id).await {
            Ok(_) => crate::speech::heard(node, resident, &pending.delivery, turn),
            Err(failure) => node.log.line(format_args!("settling what {resident} took: {failure}")),
        }
    }
}

/// The resident's session ended: a turn it ran ends with it, and it sleeps
/// until words wake it.
async fn ended(node: &Arc<Node>, resident: &ResidentId, reason: Option<String>) {
    let running =
        node.residents.now(resident).and_then(|now| (now.activity == Activity::Busy).then_some(now.turn)).flatten();
    if let Some(turn) = running {
        let why = match &reason {
            Some(reason) => format!("its Claude Code session ended ({reason})"),
            None => "its Claude Code session ended".to_string(),
        };
        crate::turns::stopped(node, resident, &turn, why).await;
    }
    let slept = node.residents.change_now(&node.world, resident, |now| {
        now.activity = Activity::Asleep;
        now.since = Timestamp::now();
        now.turn = None;
    });
    if let Err(error) = slept {
        node.log.line(format_args!("recording that {resident}'s session ended: {error}"));
    }
}

/// Whether the resident's Claude session is paired with its plugin now.
pub fn paired(node: &Node, resident: &ResidentId) -> bool {
    node.claude.state.lock().expect("never poisoned").paired.contains_key(resident)
}

/// A Claude session's MCP connection arrived: its session may have
/// registered already.
pub async fn arrived(node: &Arc<Node>, resident: &ResidentId) {
    let sessions: Vec<String> = {
        let state = node.claude.state.lock().expect("never poisoned");
        state.sessions.iter().filter(|(_, session)| session.resident.is_none()).map(|(id, _)| id.clone()).collect()
    };
    for session in sessions {
        pair(node, &session).await;
        if node.claude.state.lock().expect("never poisoned").paired.contains_key(resident) {
            return;
        }
    }
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
    let here = node.machine();
    let resident = match &registration.resident {
        Some(tag) => {
            tag.parse::<Address>().ok().filter(|address| address.machine == here).map(|address| address.resident)
        }
        // The session it was before the node restarted, or the one whose MCP
        // connection gave the same inbox address.
        None => node.residents.resident_of_thread(&registration.session).or_else(|| {
            registration.inbox_address.as_deref().and_then(|inbox| node.residents.resident_of_inbox(inbox))
        }),
    };
    let Some(resident) = resident.filter(|resident| node.residents.about(resident).is_some()) else {
        return;
    };
    let set = node.residents.set_session(
        &node.world,
        &resident,
        session,
        registration.model_name(),
        registration.version_text(),
    );
    if let Err(error) = set {
        node.log.line(format_args!("recording which session {resident} is: {error}"));
    }
    {
        let mut state = node.claude.state.lock().expect("never poisoned");
        state.paired.insert(resident.clone(), session.to_string());
        state.sessions.get_mut(session).expect("looked up above").resident = Some(resident.clone());
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
            for pending in kept.into_iter().filter(|pending| pending.state == PendingState::Waiting) {
                if let Err(failure) = into_inbox(node, session, &directory, &pending) {
                    node.log.line(format_args!("handing {} to {resident}'s session: {failure}", pending.delivery.id));
                }
            }
        }
        Ok(Err(error)) => node.log.line(format_args!("reading what waits for {resident}: {error}")),
        Err(_) => {}
    }
    // What it reported before it was paired is read now.
    let directory = exchange::sessions(node.world.path()).join(session);
    Box::pin(look(node, &directory)).await;
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
    let session = node.claude.state.lock().expect("never poisoned").paired.get(to).cloned()?;
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
            let woken = wake(node, &session, if as_user { text } else { framed(&text) }).await;
            if woken.is_ok()
                && let Err(error) = crate::inbox::set_state(directory, &pending.delivery.id, PendingState::Waking)
            {
                node.log.line(format_args!("keeping what woke {to}: {error}"));
            }
            woken
        }
        Err(failure) => Err(failure),
    })
}

/// Puts something kept into the mod's inbox; it is kept until the mod says
/// which turn took it.
fn into_inbox(node: &Node, session: &str, directory: &ResidentDir, pending: &Pending) -> Result<Landed, Failure> {
    let (text, as_user) = words(&pending.delivery.notice);
    let session_directory = exchange::sessions(node.world.path()).join(session);
    exchange::deliver(&session_directory, pending.delivery.id.as_str(), &text, as_user)
        .and_then(|()| crate::inbox::set_state(directory, &pending.delivery.id, PendingState::Handed))
        .map(|()| Landed::Handed)
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

/// What a woken session reads first when the World wakes it with words of
/// its own: said as the World's, as the mod says them.
fn framed(text: &str) -> String {
    format!("The fastctx plugin delivers a message from the World.\n\n{text}")
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

async fn wake(node: &Arc<Node>, session: &str, words: String) -> Result<Landed, Failure> {
    let Command { program, cwd, env } = command(node)?;
    let args = background::wake_args(session, &words);
    tokio::task::spawn_blocking(move || background::claude(&program, &args, &cwd, &env))
        .await
        .map_err(|error| Failure::new(format!("waking the session: {error}")))?
        .map_err(Failure::new)?;
    Ok(Landed::Handed)
}

/// The `claude` command, where to run it, and the environment it gets: the
/// user's own, as for every program the node starts.
struct Command {
    program: PathBuf,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
}

fn command(node: &Node) -> Result<Command, Failure> {
    let env = fastctx_platform::world_process_environment()
        .map_err(|error| Failure::new(format!("reading the user's environment: {error}")))?;
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
    let Command { program, mut env, .. } = command(node)?;
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
    let address = Address { machine: node.machine(), resident: id.clone() };
    env.retain(|(name, _)| name != "FASTCTX_RESIDENT" && name != "FASTCTX_HOME");
    env.push(("FASTCTX_RESIDENT".into(), address.tag().into()));
    env.push(("FASTCTX_HOME".into(), node.root.clone().into_os_string()));
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
    };
    crate::inbox::keep(&directory, &first)
        .map_err(|error| Failure::new(format!("keeping the first words: {error}")))?;
    tokio::task::spawn_blocking(move || {
        let start =
            background::Start { plugin: &plugin, settings: &settings, instructions: &instructions, first_words: &text };
        background::claude(&program, &background::start_args(&start), &cwd, &env)
    })
    .await
    .map_err(|error| Failure::new(format!("starting the session: {error}")))?
    .map_err(Failure::new)?;
    Ok((id, Landed::Handed))
}

/// Settings for a background session the World starts: the host's limit on
/// a tool answer opened past the World's own (`world-view.md`, 一次给多少),
/// and the host's own shell, file and search tools turned off, since the
/// World's take their place.
fn settings_json() -> String {
    let settings = serde_json::json!({
        "env": { "MAX_MCP_OUTPUT_TOKENS": "200000" },
        "permissions": {
            "deny": ["Bash", "PowerShell", "Read", "Write", "Edit", "MultiEdit", "NotebookEdit", "Grep", "Glob"],
        },
    });
    serde_json::to_string_pretty(&settings).expect("JSON values serialize")
}
