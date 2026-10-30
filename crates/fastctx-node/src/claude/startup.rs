//! Observes a background birth, or a wake that carries words, until the mod
//! pairs, without timing it out.

use std::ffi::OsString;
use std::future::Future;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use fastctx_hosts::claude::{background, exchange, startup};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Activity, PendingState};

use fastctx_types::{NoticeId, ProcessIdentity, ResidentId};

use crate::Node;

type Exit = Pin<Box<dyn Future<Output = std::io::Result<Option<u32>>> + Send>>;

pub(super) use super::listing::Listings;

/// Only a turn of the replacement activation can consume a CLI wake. Old
/// starts can arrive after the next wake was kept, even after a node restart.
pub(super) fn takes_wake(
    node: &Node,
    resident: &ResidentId,
    session: &str,
    at: jiff::Timestamp,
    activation: Option<&str>,
    pending: &fastctx_types::residents::Pending,
) -> std::io::Result<bool> {
    if pending.state != PendingState::Waking {
        return Ok(false);
    }
    let registration = exchange::registration(&exchange::sessions(node.world.path()).join(session))?;
    let current = registration.as_ref().and_then(|registration| registration.instance.as_deref());
    if activation.is_some() && current.is_none() {
        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "the activation registration is not complete"));
    }
    if activation.is_some_and(|activation| current != Some(activation)) {
        return Ok(false);
    }
    let prior = crate::lifecycle::retry::progress(node, resident, &pending.delivery.id)?
        .and_then(|progress| progress.after_activation);
    Ok(match (activation, prior.as_deref()) {
        (Some(activation), Some(prior)) => activation != prior,
        // Legacy reports still must follow both the wake and the current
        // registration, and cannot consume it before the predecessor changes.
        _ => {
            prior.as_deref().is_none_or(|prior| current != Some(prior))
                && registration.and_then(|registration| registration.at).is_none_or(|registered| registered <= at)
                && pending.since.unwrap_or(pending.delivery.at) <= at
        }
    })
}

/// `waking`: the session paired before, and these words wake it; it is
/// settled once the session that came up takes them, not by its pairing.
pub(super) fn observe(node: &Arc<Node>, resident: &ResidentId, id: NoticeId, waking: bool) {
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
        loop {
            let result = async {
                if !pending(&node, &resident, &id, waking)? {
                    return Ok(());
                }
                let home = node.residents.about(&resident).and_then(|(about, _)| about.home);
                let command = super::command(&node, home.as_deref()).map_err(std::io::Error::other)?;
                watch(&node, &resident, &id, &command, waking).await
            }
            .await;
            let Err(error) = result else { break };
            let error = error.to_string();
            if let Err(error) = report(&node, &resident, &id, error.clone()) {
                node.log.line(format_args!("keeping startup observation: {error}"));
            }
            if told.as_ref() != Some(&error) {
                node.log.line(format_args!("observing Claude startup {resident}; will retry: {error}"));
                told = Some(error);
            }
            tokio::time::sleep(again.next().max(fastctx_hosts::claude::MOD_LOOK)).await;
        }
        node.claude.state.lock().expect("never poisoned").starting.remove(&resident);
    });
}

/// An unanswered launch is reconciled, never offered a second time. Reports
/// already flushed by the host win over the CLI's missing receipt.
pub(super) fn unknown(node: &Arc<Node>, resident: &ResidentId, id: &NoticeId, why: String) {
    if !node.claude.state.lock().expect("never poisoned").starting.insert(resident.clone()) {
        return;
    }
    let (node, resident, id) = (node.clone(), resident.clone(), id.clone());
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        tokio::time::sleep(fastctx_hosts::claude::MOD_LOOK).await;
        loop {
            super::replay(&node).await;
            if node.residents.about(&resident).is_none() {
                break;
            }
            let directory = node.world.conversation(&resident);
            match super::failed_wake(&node, &resident, &directory, &id, Failure::new(why.clone())).await {
                Ok(_) => break,
                Err(error) => node
                    .log
                    .line(format_args!("lifecycle {resident}: reconciling launch; retrying observation: {error}")),
            }
            tokio::time::sleep(again.next()).await;
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
            || (!about.world_started && !waking)
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
        for input in kept.iter().filter(|input| input.state == PendingState::Handed) {
            let (node, about, input) = (node.clone(), about.clone(), input.clone());
            tokio::spawn(async move {
                node.ready().await;
                let Some(session) = about.session.as_deref() else { return };
                let actions = crate::control_queue::gate(&node, &about.id);
                let _action = actions.lock().await;
                let Ok(gate) = node.residents.management(&about.id) else { return };
                let _reading = gate.read().await;
                if node.residents.active(&about.id).is_err() {
                    return;
                }
                let directory = node.world.conversation(&about.id);
                if let Err(error) = super::into_inbox(&node, &about.id, session, &directory, &input).await {
                    node.log.line(format_args!("reconciling {}'s plugin handover: {error}", about.id));
                }
            });
        }
        // A first turn that waited for its login when the node stopped waits
        // on: no session was asked for.
        if about.session.is_none() && super::queued::marked(&directory) {
            super::queued::wait(node, about.id.clone());
            continue;
        }
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
        match crate::lifecycle::retry::progress(node, &about.id, &first.delivery.id) {
            Ok(Some(progress)) if !progress.attempted => {
                if let Err(error) = crate::lifecycle::retry::defer(
                    node,
                    &about.id,
                    &first.delivery.id,
                    "the node restarted before the host launch",
                ) {
                    node.log.line(format_args!("recovering host preflight: {error}"));
                }
                continue;
            }
            Ok(Some(progress)) if !progress.accepted => {
                unknown(
                    node,
                    &about.id,
                    &first.delivery.id,
                    "the node restarted before the host launch receipt; acceptance requires reconciliation".into(),
                );
                continue;
            }
            Err(error) => {
                unknown(node, &about.id, &first.delivery.id, format!("host launch progress cannot be read: {error}"));
                continue;
            }
            _ => {}
        }
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
        observe(node, &about.id, first.delivery.id, waking);
    }
}

pub(super) async fn run(command: &super::Command, args: Vec<OsString>) -> Result<String, String> {
    let command = command.clone();
    tokio::task::spawn_blocking(fastctx_platform::doing::carried(move || {
        background::claude(&command.program, &args, &command.cwd, &command.env)
    }))
    .await
    .map_err(|error| format!("observing Claude startup: {error}"))?
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

/// Whether nothing took the words `id` that woke the session `session`:
/// they still wait as waking, no turn is recorded for them, and the
/// session's transcript holds nothing submitted since they were sent.
/// Anything unread or unknown counts as taken, so words never go in twice.
pub(super) fn untaken(
    node: &Node,
    session: &str,
    directory: &fastctx_types::layout::ConversationDir,
    id: &NoticeId,
) -> std::io::Result<bool> {
    if crate::delivery::input_turn(directory, id)?.is_some() {
        return Ok(false);
    }
    let Some(since) = crate::inbox::read(directory, id)?
        .filter(|pending| matches!(pending.state, PendingState::Waking | PendingState::Handed))
        .and_then(|pending| pending.since)
    else {
        return Ok(false);
    };
    let host_directory = exchange::sessions(node.world.path()).join(session);
    if host_directory.join("receipts").join(id.as_str()).try_exists()? {
        return Ok(false);
    }
    let Ok(Some(transcript)) = exchange::transcript(&host_directory) else {
        return Ok(false);
    };
    Ok(matches!(submitted_since(&transcript, since), Ok(false)))
}

/// The activation ended and `untaken` proved no submission. Remove its old
/// plugin envelope before retrying through the CLI, so the new activation
/// cannot receive both the old envelope and the resume prompt.
pub(super) fn retry_untaken(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    id: &NoticeId,
) -> std::io::Result<()> {
    let directory = exchange::sessions(node.world.path()).join(session);
    for path in [directory.join("inbox").join(format!("{id}.json")), directory.join("attempts").join(id.as_str())] {
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    crate::lifecycle::retry::defer(node, resident, id, "the host ended before taking the input")
        .map(|_| ())
        .map_err(std::io::Error::other)
}

/// Whether the transcript at `path` holds, at or after `since`, words
/// submitted to the session: a prompt, or one queued for it. Its lines are in
/// the order they were written. One that cannot be read, as a row cut short
/// when the host was killed, may hold an accepted prompt: it counts as
/// submitted, which makes the delivery uncertain rather than repeated, unless
/// a later line was written before `since`. A row damaged long ago therefore
/// does not make every later wake of the session uncertain.
fn submitted_since(path: &std::path::Path, since: jiff::Timestamp) -> std::io::Result<bool> {
    use std::io::BufRead as _;
    let since = since.as_millisecond();
    let mut unreadable = false;
    for line in std::io::BufReader::new(std::fs::File::open(path)?).split(b'\n') {
        let line = line?;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<serde_json::Value>(&line) else {
            unreadable = true;
            continue;
        };
        let at = entry["timestamp"].as_str().and_then(|at| at.parse::<jiff::Timestamp>().ok());
        if at.is_some_and(|at| at.as_millisecond() < since) {
            unreadable = false;
        }
        if matches!(entry["type"].as_str(), Some("user" | "queue-operation")) {
            match at {
                Some(at) if at.as_millisecond() >= since => return Ok(true),
                Some(_) => {}
                None => unreadable = true,
            }
        }
    }
    Ok(unreadable)
}

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
        .get_or_insert_with(&(command.program.clone(), home), || super::listing::Listing::new(command, &login));
    let listing = listing.as_ref().map_err(|error| std::io::Error::other(error.clone()))?;
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
    let mut observation = None;
    let mut pause = fastctx_hosts::claude::MOD_LOOK;
    let mut records = listing.changes();

    loop {
        // What the list read below has seen of the records: only a later
        // change wakes the wait for the next look.
        records.borrow_and_update();
        let changed = node.claude.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if !pending(node, resident, id, waking)? {
            return Ok(());
        }
        let listed = listing.agents(command, after).await;
        after = None;
        if !pending(node, resident, id, waking)? {
            return Ok(());
        }
        let listed = listed.and_then(|listed| startup::background(&listed, &short));
        let current =
            listed.as_ref().map(|job| job.as_ref().map(|job| (job.state.clone(), job.pid))).map_err(Clone::clone);
        pause = if observation.as_ref() == Some(&current) {
            (pause * 2).min(std::time::Duration::from_secs(60))
        } else {
            fastctx_hosts::claude::MOD_LOOK
        };
        observation = Some(current);
        match listed {
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
                        Ok(None) => {
                            if process.as_ref().is_some_and(|identity| identity.pid != pid) {
                                // A replacement already gone must not inherit the
                                // previous process's observed exit code.
                                process = None;
                                exited = None;
                                code = None;
                                process_ended = false;
                            }
                            // The supervisor can report a terminal job after its
                            // process disappeared, before we ever observed it.
                            if job.ended() {
                                process_ended = true;
                            }
                        }
                        Ok(_) => {}
                        Err(error) => node.log.line(format_args!("observing Claude startup process {pid}: {error}")),
                    }
                }
                if !process_ended
                    && exited.is_none()
                    && let Some(identity) = process.clone()
                {
                    exited =
                        Some(Box::pin(async move { fastctx_platform::host_process::exit_status(&identity).await }));
                }
                if (job.ended() && (job.pid.is_none_or(|pid| pid == 0) || process_ended)) || (job.blocked() && !blocked)
                {
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
                        if waking && untaken(node, &session, &directory, id)? {
                            drop(births);
                            retry_untaken(node, resident, &session, id)?;
                            return Ok(());
                        }
                        return failed(node, resident, &directory, id, why).await;
                    }
                    // The host's watchdog and a real setup dialog produce the
                    // same blocked state. Preserve the fact without declaring
                    // a slow but successful birth failed.
                    report(node, resident, id, why)?;
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
                    report(node, resident, id, why)?;
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
                    report(node, resident, id, why.clone())?;
                    last_problem = Some(why);
                }
            }
            Err(error) => {
                let _births = node.claude.births.write().await;
                if !pending(node, resident, id, waking)? {
                    return Ok(());
                }
                if last_problem.as_ref() != Some(&error) {
                    report(node, resident, id, error.clone())?;
                    last_problem = Some(error);
                }
            }
        }
        let saw_exit = {
            let exit = async {
                match &mut exited {
                    Some(exit) => exit.await,
                    None => std::future::pending().await,
                }
            };
            tokio::pin!(exit);
            let next = tokio::time::sleep(pause);
            tokio::pin!(next);
            loop {
                tokio::select! {
                    result = &mut exit => {
                        match result {
                            Ok(status) => {
                                code = status;
                                process_ended = true;
                            }
                            Err(error) => node.log.line(format_args!("reading Claude startup exit status: {error}")),
                        }
                        after = Some(Instant::now());
                        observation = None;
                        break true;
                    }
                    () = &mut changed => {
                        changed.set(node.claude.changed.notified());
                        changed.as_mut().enable();
                        if !pending(node, resident, id, waking)? {
                            return Ok(());
                        }
                        // Changes in another session do not relaunch this login's
                        // CLI. Pairing and input acceptance still wake us at once.
                    },
                    // Unchanged facts reduce observation work, never establish a
                    // failed birth. A process exit forces a fresh query at once,
                    // and a change to the supervisor's records a look at it.
                    Ok(()) = records.changed() => break false,
                    () = &mut next => break false,
                }
            }
        };
        if saw_exit {
            exited = None;
        }
    }
}

fn report(node: &Arc<Node>, resident: &ResidentId, id: &NoticeId, text: String) -> std::io::Result<()> {
    crate::lifecycle::retry::observing(node, resident, id, text).map_err(std::io::Error::other)
}
