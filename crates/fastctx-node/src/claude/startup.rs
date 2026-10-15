//! Observes a background birth until the mod pairs, without timing it out.

use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use fastctx_hosts::claude::{background, startup};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::PendingState;
use fastctx_types::session::{Health, HealthKind};
use fastctx_types::{NoticeId, ProcessIdentity, ResidentId};

use crate::Node;

type Exit = Pin<Box<dyn Future<Output = std::io::Result<Option<u32>>> + Send>>;

pub(super) type Listings = crate::leases::Registry<(PathBuf, Option<OsString>), tokio::sync::Mutex<Option<Listed>>>;

pub(super) struct Listed {
    at: Instant,
    answer: Result<String, String>,
}

pub(super) fn observe(node: &Arc<Node>, resident: &ResidentId, id: NoticeId, command: super::Command) {
    if super::paired(node, resident) {
        return;
    }
    if !node.claude.state.lock().expect("never poisoned").starting.insert(resident.clone()) {
        return;
    }
    let node = node.clone();
    let resident = resident.clone();
    tokio::spawn(async move {
        watch(&node, &resident, &id, &command).await;
        node.claude.state.lock().expect("never poisoned").starting.remove(&resident);
    });
}

/// A node restart must resume observing births whose first words are still
/// crossing the host boundary. No prompt is submitted a second time.
pub(super) fn recover(node: &Arc<Node>) {
    for record in node.residents.everyone() {
        let about = record.about;
        if record.removed.is_some()
            || !about.world_started
            || about.host.agent.as_str() != "claude"
            || super::paired(node, &about.id)
        {
            continue;
        }
        if about.session.as_ref().is_some_and(|session| session.len() != 8) {
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
        let Some(first) = kept.into_iter().find(|pending| pending.state == PendingState::Waking) else { continue };
        if about.session.is_none() {
            let node = node.clone();
            tokio::spawn(async move {
                let _births = node.claude.births.write().await;
                if !pending(&node, &about.id, &first.delivery.id) {
                    return;
                }
                let why = "the node restarted before recording Claude Code's background session ID; its startup outcome is unknown";
                if let Err(error) =
                    super::failed_wake(&node, &about.id, &directory, &first.delivery.id, Failure::new(why)).await
                {
                    node.log.line(format_args!("recording interrupted Claude startup: {error}"));
                }
            });
            continue;
        }
        match super::command(node, about.home.as_deref()) {
            Ok(command) => observe(node, &about.id, first.delivery.id, command),
            Err(error) => {
                let _ = report(node, &about.id, error.message);
            }
        }
    }
}

async fn run(command: &super::Command, args: Vec<OsString>) -> Result<String, String> {
    let command = command.clone();
    tokio::task::spawn_blocking(move || background::claude(&command.program, &args, &command.cwd, &command.env))
        .await
        .map_err(|error| format!("observing Claude startup: {error}"))?
}

async fn agents(
    listing: &tokio::sync::Mutex<Option<Listed>>,
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

/// How much of the end of Claude Code's daemon log is read for a session.
const DAEMON_TAIL: u64 = 256 * 1024;

fn pending(node: &Node, resident: &ResidentId, id: &NoticeId) -> bool {
    if super::paired(node, resident) || node.residents.removed(resident).is_some() {
        return false;
    }
    if node.residents.about(resident).is_none() {
        return false;
    }
    let directory = node.world.conversation(resident);
    match crate::inbox::read(&directory, id) {
        Ok(pending) => pending.is_some(),
        Err(error) => {
            node.log.line(format_args!("reading unpaired Claude startup {resident}: {error}"));
            false
        }
    }
}

async fn watch(node: &Arc<Node>, resident: &ResidentId, id: &NoticeId, command: &super::Command) {
    let Some(short) = node.residents.about(resident).and_then(|(about, _)| about.session) else { return };
    let home = command
        .env
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        .map(|(_, home)| home.clone());
    let listing = node.claude.startup_listings.get_or_insert_with(&(command.program.clone(), home), Default::default);
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
    loop {
        let changed = node.claude.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if !pending(node, resident, id) {
            return;
        }
        let listed = agents(&listing, command, after).await;
        after = None;
        if !pending(node, resident, id) {
            return;
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
                    let _births = node.claude.births.write().await;
                    if !pending(node, resident, id) {
                        return;
                    }
                    if job.ended() {
                        if node.residents.about(resident).is_none() {
                            return;
                        }
                        let directory = node.world.conversation(resident);
                        if let Err(error) = super::failed_wake(node, resident, &directory, id, Failure::new(why)).await
                        {
                            node.log.line(format_args!("recording failed Claude startup: {error}"));
                        }
                        return;
                    }
                    // The host's watchdog and a real setup dialog produce the
                    // same blocked state. Preserve the fact without declaring
                    // a slow but successful birth failed.
                    if let Err(error) = report(node, resident, why) {
                        node.log.line(format_args!("recording blocked Claude startup: {error}"));
                    }
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
                    if !pending(node, resident, id) {
                        return;
                    }
                    if let Err(error) = report(node, resident, why) {
                        node.log.line(format_args!("recording exited Claude startup process: {error}"));
                    }
                    reported_exit = process.clone();
                }
                blocked = job.blocked();
                last_problem = None;
            }
            Ok(None) => {
                let _births = node.claude.births.write().await;
                if !pending(node, resident, id) {
                    return;
                }
                let why =
                    format!("Claude Code does not list background session {short}; its startup outcome is unknown");
                if last_problem.as_ref() != Some(&why) {
                    let _ = report(node, resident, why.clone());
                    last_problem = Some(why);
                }
            }
            Err(error) => {
                let _births = node.claude.births.write().await;
                if !pending(node, resident, id) {
                    return;
                }
                if last_problem.as_ref() != Some(&error) {
                    let _ = report(node, resident, error.clone());
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
