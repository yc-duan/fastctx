//! Observes a background birth, or a wake that carries words, until the mod
//! pairs, without timing it out.

use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use fastctx_hosts::claude::{background, exchange, startup};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Activity, PendingState};
use fastctx_types::session::{Health, HealthKind};
use fastctx_types::{NoticeId, ProcessIdentity, ResidentId};

use crate::Node;

type Exit = Pin<Box<dyn Future<Output = std::io::Result<Option<u32>>> + Send>>;

pub(super) type Listings =
    crate::leases::Registry<(PathBuf, Option<OsString>), fastctx_platform::doing::Mutex<Option<Listed>>>;

pub(super) struct Listed {
    at: Instant,
    answer: Result<String, String>,
}

/// `waking`: the session paired before, and these words wake it; it is
/// settled once the session that came up takes them, not by its pairing.
pub(super) fn observe(node: &Arc<Node>, resident: &ResidentId, id: NoticeId, command: super::Command, waking: bool) {
    if !waking && super::paired(node, resident) {
        return;
    }
    if !node.claude.state.lock().expect("never poisoned").starting.insert(resident.clone()) {
        return;
    }
    let node = node.clone();
    let resident = resident.clone();
    tokio::spawn(async move {
        // Tried again after waits that double up to a minute, told once for
        // each new reason.
        let (mut again, mut told) = (crate::again::Again::default(), None);
        while let Err(error) = watch(&node, &resident, &id, &command, waking).await {
            let error = error.to_string();
            if told.as_ref() != Some(&error) {
                node.log.line(format_args!("observing Claude startup {resident}; will retry: {error}"));
                told = Some(error);
            }
            tokio::time::sleep(again.next().max(fastctx_hosts::claude::MOD_LOOK)).await;
        }
        node.claude.state.lock().expect("never poisoned").starting.remove(&resident);
    });
}

/// A node restart must resume observing births and wakes whose words are
/// still crossing the host boundary. No prompt is submitted a second time.
pub(super) fn recover(node: &Arc<Node>) {
    for record in node.residents.everyone() {
        let about = record.about;
        // A session that registered holds its whole id: its words wake it.
        let waking = about.session.as_ref().is_some_and(|session| session.len() != 8);
        if record.removed.is_some()
            || !about.world_started
            || about.host.agent.as_str() != "claude"
            || (!waking && super::paired(node, &about.id))
        {
            continue;
        }
        let directory = node.world.conversation(&about.id);
        let kept = match crate::inbox::pending(&directory) {
            Ok(pending) => pending,
            Err(error) => {
                node.log.line(format_args!("recovering Claude startup observation: {error}"));
                continue;
            }
        };
        let Some(first) = kept.into_iter().find(|pending| pending.state == PendingState::Waking) else {
            // Born and stopped before its first words were kept: no session
            // was asked for, and nothing will end the work it was shown at.
            if about.session.is_none()
                && node.residents.now(&about.id).is_some_and(|now| now.activity == Activity::Busy && now.turn.is_none())
                && let Err(error) = node.residents.change_now(&node.world, &about.id, |now| {
                    now.activity = Activity::Idle;
                    now.since = jiff::Timestamp::now();
                    now.trouble = Some(fastctx_types::residents::Trouble {
                        at: jiff::Timestamp::now(),
                        text: "the node stopped while this conversation was being started; say something to start it"
                            .into(),
                    });
                })
            {
                node.log.line(format_args!("ending a start the node stopped during: {error}"));
            }
            continue;
        };
        if about.session.is_none() {
            let node = node.clone();
            tokio::spawn(async move {
                let (mut again, mut told) = (crate::again::Again::default(), None);
                loop {
                    let result = async {
                        let _births = node.claude.births.write().await;
                        if !pending(&node, &about.id, &first.delivery.id, false)? {
                            return Ok(());
                        }
                        let why = "the node restarted before recording Claude Code's background session ID; its startup outcome is unknown";
                        super::failed_wake(&node, &about.id, &directory, &first.delivery.id, Failure::new(why))
                            .await.map(|_| ()).map_err(std::io::Error::other)
                    }.await;
                    match result {
                        Ok(()) => break,
                        Err(error) => {
                            let error = error.to_string();
                            if told.as_ref() != Some(&error) {
                                node.log
                                    .line(format_args!("recording interrupted Claude startup; will retry: {error}"));
                                told = Some(error);
                            }
                        }
                    }
                    tokio::time::sleep(again.next().max(fastctx_hosts::claude::MOD_LOOK)).await;
                }
            });
            continue;
        }
        match super::command(node, about.home.as_deref()) {
            Ok(command) => observe(node, &about.id, first.delivery.id, command, waking),
            Err(error) => {
                if let Err(error) = report(node, &about.id, error.message) {
                    node.log.line(format_args!("recording Claude startup problem for {}: {error}", about.id));
                }
            }
        }
    }
}

async fn run(command: &super::Command, args: Vec<OsString>) -> Result<String, String> {
    let command = command.clone();
    tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        background::claude(&command.program, &args, &command.cwd, &command.env)
    }))
    .await
    .map_err(|error| format!("observing Claude startup: {error}"))?
}

async fn agents(
    listing: &fastctx_platform::doing::Mutex<Option<Listed>>,
    command: &super::Command,
    after: Option<Instant>,
) -> Result<String, String> {
    let mut listing = listing.lock().await;
    if let Some(listed) = &*listing
        && after.is_none_or(|after| listed.at >= after)
        && listed.at.elapsed() < fastctx_hosts::claude::MOD_LOOK
    {
        return listed.answer.clone();
    }
    // Concurrent births in one login share one query. Repeated CLI launches
    // per resident would compete with startup on small machines.
    let at = Instant::now();
    let answer = run(command, vec!["agents".into(), "--json".into(), "--all".into()]).await;
    *listing = Some(Listed { at, answer: answer.clone() });
    answer
}

async fn diagnosis(command: &super::Command, short: &str, mut why: String) -> String {
    match run(command, vec!["logs".into(), short.into()]).await {
        Ok(output) if !output.is_empty() => {
            why.push_str("\nRecent terminal output from `claude logs` (Claude retains a bounded replay):\n");
            why.push_str(&output);
        }
        Ok(_) => why.push_str("; `claude logs` returned no terminal output"),
        Err(error) => why.push_str(&format!("; reading its terminal output failed: {error}")),
    }
    // A session that exits before it starts leaves no terminal output; its
    // daemon says why in its own log.
    if let Some(said) = daemon_said(command, short) {
        why.push_str(&format!("; Claude Code's daemon log says: {said}"));
    }
    why
}

/// The last line Claude Code's daemon logged of the session `short`, in the
/// login the command runs with (`daemon.log` in its directory).
fn daemon_said(command: &super::Command, short: &str) -> Option<String> {
    use std::io::{Read as _, Seek as _};
    let login = match command.env.iter().find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR")) {
        Some((_, dir)) => std::path::PathBuf::from(dir),
        None => fastctx_platform::user_home().ok()?.join(".claude"),
    };
    let mut log = std::fs::File::open(login.join("daemon.log")).ok()?;
    let length = log.metadata().ok()?.len();
    log.seek(std::io::SeekFrom::Start(length.saturating_sub(DAEMON_TAIL))).ok()?;
    let mut tail = Vec::new();
    log.read_to_end(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail);
    let line = tail.lines().rev().find(|line| line.contains(short))?.trim();
    Some(line.chars().take(400).collect())
}

/// Whether Claude Code's daemon logged that it settled the session `short`.
fn settled(command: &super::Command, short: &str) -> bool {
    daemon_said(command, short).is_some_and(|line| line.contains(&format!("bg settled {short}")))
}

/// How much of the end of Claude Code's daemon log is read for a session.
const DAEMON_TAIL: u64 = 256 * 1024;

/// How many times words wake a session that ends before taking them, and
/// how much longer each next time waits first, for its daemon to free what
/// it was short of.
const WAKES: u32 = 3;
const WAKE_AGAIN: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether nothing took the words `id` that woke the session `session`:
/// they still wait as waking, no turn is recorded for them, and the
/// session's transcript holds nothing submitted since they were sent.
/// Anything unread or unknown counts as taken, so words never go in twice.
fn untaken(
    node: &Node,
    session: &str,
    directory: &fastctx_types::layout::ConversationDir,
    id: &NoticeId,
) -> std::io::Result<bool> {
    if crate::delivery::input_turn(directory, id)?.is_some() {
        return Ok(false);
    }
    let Some(since) = crate::inbox::read(directory, id)?
        .filter(|pending| pending.state == PendingState::Waking)
        .and_then(|pending| pending.since)
    else {
        return Ok(false);
    };
    let Ok(Some(transcript)) = exchange::transcript(&exchange::sessions(node.world.path()).join(session)) else {
        return Ok(false);
    };
    Ok(matches!(submitted_since(&transcript, since), Ok(false)))
}

/// Whether the transcript at `path` holds, at or after `since`, words
/// submitted to the session: a prompt, or one queued for it.
fn submitted_since(path: &std::path::Path, since: jiff::Timestamp) -> std::io::Result<bool> {
    use std::io::{Read as _, Seek as _};
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(std::io::SeekFrom::Start(length.saturating_sub(TRANSCRIPT_TAIL)))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;
    Ok(String::from_utf8_lossy(&tail).lines().filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok()).any(
        |entry| {
            matches!(entry["type"].as_str(), Some("user" | "queue-operation"))
                && entry["timestamp"].as_str().and_then(|at| at.parse::<jiff::Timestamp>().ok()).is_some_and(|at| at >= since)
        },
    ))
}

/// How much of the end of a session's transcript is read for what was
/// submitted lately.
const TRANSCRIPT_TAIL: u64 = 256 * 1024;

async fn failed(
    node: &Arc<Node>,
    resident: &ResidentId,
    directory: &fastctx_types::layout::ConversationDir,
    id: &NoticeId,
    why: String,
) -> std::io::Result<()> {
    super::failed_wake(node, resident, directory, id, Failure::new(why)).await.map_err(std::io::Error::other)?;
    Ok(())
}

fn pending(node: &Node, resident: &ResidentId, id: &NoticeId, waking: bool) -> std::io::Result<bool> {
    if (!waking && super::paired(node, resident)) || node.residents.removed(resident).is_some() {
        return Ok(false);
    }
    if node.residents.about(resident).is_none() {
        return Ok(false);
    }
    let directory = node.world.conversation(resident);
    crate::inbox::read(&directory, id)
        .map(|pending| pending.is_some_and(|pending| !waking || pending.state == PendingState::Waking))
}

async fn watch(
    node: &Arc<Node>,
    resident: &ResidentId,
    id: &NoticeId,
    command: &super::Command,
    waking: bool,
) -> std::io::Result<()> {
    let Some(session) = node.residents.about(resident).and_then(|(about, _)| about.session) else { return Ok(()) };
    // A background job is named by the first eight characters of its
    // session's id, which a registered session holds whole.
    let short: String = session.chars().take(8).collect();
    let home = command
        .env
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        .map(|(_, home)| home.clone());
    let login =
        home.as_ref().map_or_else(|| command.program.display().to_string(), |home| home.to_string_lossy().into_owned());
    let listing = node
        .claude
        .startup_listings
        .get_or_insert_with(&(command.program.clone(), home), || crate::ranked::listing(&login));
    let mut process: Option<ProcessIdentity> = None;
    let mut exited: Option<Exit> = None;
    let mut code = None;
    let mut process_ended = false;
    let mut reported_exit = None;
    let mut last_problem = None;
    let mut blocked = false;
    // A cached list from before this birth cannot establish that its job
    // is missing. Concurrent births share a list begun after their starts.
    let mut after = Some(Instant::now());
    // How many times the words have woken the session.
    let mut woken = 1;
    loop {
        let changed = node.claude.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if !pending(node, resident, id, waking)? {
            return Ok(());
        }
        let listed = agents(&listing, command, after).await;
        after = None;
        if !pending(node, resident, id, waking)? {
            return Ok(());
        }
        match listed.and_then(|listed| startup::background(&listed, &short)) {
            Ok(Some(job)) => {
                if let Some(pid) = job.pid.filter(|pid| *pid != 0) {
                    match fastctx_platform::identity_of(pid) {
                        Ok(Some(identity)) if process.as_ref() != Some(&identity) => {
                            process = Some(identity.clone());
                            code = None;
                            process_ended = false;
                            exited =
                                Some(Box::pin(
                                    async move { fastctx_platform::host_process::exit_status(&identity).await },
                                ));
                        }
                        Ok(None) if process.as_ref().is_some_and(|identity| identity.pid != pid) => {
                            // A replacement already gone must not inherit the
                            // previous process's observed exit code.
                            process = None;
                            exited = None;
                            code = None;
                            process_ended = false;
                        }
                        Ok(_) => {}
                        Err(error) => node.log.line(format_args!("observing Claude startup process {pid}: {error}")),
                    }
                }
                if job.ended() || (job.blocked() && !blocked) {
                    if job.ended()
                        && process
                            .as_ref()
                            .is_some_and(|identity| matches!(fastctx_platform::has_ended(identity), Ok(true)))
                        && let Some(exit) = exited.take()
                    {
                        match exit.await {
                            Ok(status) => {
                                code = status;
                                process_ended = true;
                            }
                            Err(error) => node.log.line(format_args!("reading Claude startup exit status: {error}")),
                        }
                    }
                    let mut why = format!(
                        "Claude Code reports background session {short} as {} before the World's plugin paired",
                        job.state
                    );
                    if job.ended()
                        && let Some(code) = code
                        && let Some(identity) = &process
                    {
                        why.push_str(&format!(
                            "; observed process {} exited with code {code} (0x{code:08x})",
                            identity.pid
                        ));
                    }
                    let why = diagnosis(command, &short, why).await;
                    // A registration written before the host outcome wins,
                    // including one the filesystem watcher has not seen yet.
                    super::replay(node).await;
                    let births = node.claude.births.write().await;
                    if !pending(node, resident, id, waking)? {
                        return Ok(());
                    }
                    // Claude Code may go on listing a session its daemon
                    // settled long ago as blocked: that one has ended.
                    if job.ended() || settled(command, &short) {
                        if node.residents.about(resident).is_none() {
                            return Ok(());
                        }
                        let directory = node.world.conversation(resident);
                        // Words that woke a session which ended before taking
                        // them wake it again: Claude Code's daemon, short of
                        // memory, retires the very worker it just claimed
                        // for them as one long idle.
                        let mut why = why;
                        if waking && woken < WAKES && untaken(node, &session, &directory, id)? {
                            drop(births);
                            node.log.line(format_args!(
                                "waking {resident} again: its session ended before taking the words that woke it ({woken} of {WAKES}): {why}"
                            ));
                            tokio::time::sleep(WAKE_AGAIN * woken).await;
                            if !pending(node, resident, id, waking)? {
                                return Ok(());
                            }
                            match super::wake_again(node, resident, &session, id, command).await {
                                Ok(()) => {
                                    woken += 1;
                                    (process, exited, code, process_ended) = (None, None, None, false);
                                    (reported_exit, blocked, last_problem) = (None, false, None);
                                    after = Some(Instant::now());
                                    continue;
                                }
                                Err(error) => why.push_str(&format!("; waking it again failed: {}", error.message)),
                            }
                            let _births = node.claude.births.write().await;
                            if !pending(node, resident, id, waking)? {
                                return Ok(());
                            }
                            return failed(node, resident, &directory, id, why).await;
                        }
                        return failed(node, resident, &directory, id, why).await;
                    }
                    // The host's watchdog and a real setup dialog produce the
                    // same blocked state. Preserve the fact without declaring
                    // a slow but successful birth failed.
                    report(node, resident, why)?;
                } else if process_ended && reported_exit != process {
                    let identity = process.as_ref().expect("an observed process exited");
                    let mut why = format!(
                        "Claude Code startup process {} exited before the World's plugin paired; its supervisor still reports {}",
                        identity.pid, job.state
                    );
                    if let Some(code) = code {
                        why.push_str(&format!("; exit code {code} (0x{code:08x})"));
                    }
                    let why = diagnosis(command, &short, why).await;
                    super::replay(node).await;
                    let _births = node.claude.births.write().await;
                    if !pending(node, resident, id, waking)? {
                        return Ok(());
                    }
                    report(node, resident, why)?;
                    reported_exit = process.clone();
                }
                blocked = job.blocked();
                last_problem = None;
            }
            Ok(None) => {
                let _births = node.claude.births.write().await;
                if !pending(node, resident, id, waking)? {
                    return Ok(());
                }
                let why =
                    format!("Claude Code does not list background session {short}; its startup outcome is unknown");
                if last_problem.as_ref() != Some(&why) {
                    report(node, resident, why.clone())?;
                    last_problem = Some(why);
                }
            }
            Err(error) => {
                let _births = node.claude.births.write().await;
                if !pending(node, resident, id, waking)? {
                    return Ok(());
                }
                if last_problem.as_ref() != Some(&error) {
                    report(node, resident, error.clone())?;
                    last_problem = Some(error);
                }
            }
        }
        let exit = async {
            match &mut exited {
                Some(exit) => exit.await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            result = exit => {
                match result {
                    Ok(status) => {
                        code = status;
                        process_ended = true;
                    }
                    Err(error) => node.log.line(format_args!("reading Claude startup exit status: {error}")),
                }
                exited = None;
                after = Some(Instant::now());
                // A process exit alone is not a failed birth: the supervisor
                // can respawn it. Ask for the job's outcome immediately.
            }
            () = &mut changed => {},
            // This is observation frequency, shared with the mod, never a
            // deadline or a reason to declare the session failed.
            () = tokio::time::sleep(fastctx_hosts::claude::MOD_LOOK) => {},
        }
    }
}

fn report(node: &Arc<Node>, resident: &ResidentId, text: String) -> std::io::Result<()> {
    crate::turns::health(node, resident, None, Health { kind: HealthKind::Warning, text })
}
