//! Durable progress before a host accepts work. Waiting never owns a gate.

use crate::Node;
use fastctx_types::{
    NoticeId, ResidentId,
    ipc::Failure,
    layout::ConversationDir,
    residents::{About, Landed, Pending, PendingState},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

/// The conversations whose handing over waits to be tried again, each with
/// the trouble last told of it, so the log does not repeat itself.
#[derive(Default)]
pub(crate) struct Retrying(Mutex<HashMap<ResidentId, Option<String>>>);

/// The kind of job that hands a conversation's waiting work to its host.
const HANDING: &str = "host/hand-over";
/// Conversations handed to at once: a handing over may start a host session.
const HANDED_AT_ONCE: usize = 32;
/// The longest one step may take.
const HANDS_WITHIN: Duration = Duration::from_secs(10 * 60);
/// Steps in a row that may be dropped for taking too long before the job is
/// set aside until the conversation is kicked again.
const HAND_TRIES: u32 = 5;
/// How soon work that still waits is looked at again when nothing says the
/// host is ready sooner (`changed`).
const LOOKS_AGAIN: Duration = Duration::from_secs(3);

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Progress {
    pub began: Timestamp,
    pub next: Timestamp,
    /// Starts that failed; what the host said while it started is not one.
    pub failures: u32,
    /// Why the last start failed.
    pub reason: Option<String>,
    pub attempted: bool,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub after_activation: Option<String>,
    /// What watching the host start last said, kept for the log and the
    /// doctor: never a failure.
    #[serde(default)]
    pub observed: Option<String>,
    /// When the host's process was last asked for, and when it took the
    /// words: the phases of a start, and how long each took.
    #[serde(default)]
    pub attempted_at: Option<Timestamp>,
    #[serde(default)]
    pub accepted_at: Option<Timestamp>,
}

impl Progress {
    /// How far the start has come (`KeptWords::starting`).
    pub fn starting(&self) -> fastctx_types::session::Starting {
        use fastctx_types::session::{StartPhase, Starting};
        let now = Timestamp::now();
        let waiting = self.failures > 0 && !self.attempted && self.next > now;
        let phase = if waiting {
            StartPhase::Retrying
        } else if !self.attempted {
            StartPhase::Preparing
        } else if !self.accepted {
            StartPhase::Launching
        } else {
            StartPhase::Connecting
        };
        Starting {
            since: self.began,
            phase,
            failures: self.failures,
            reason: (self.failures > 0).then(|| self.reason.clone()).flatten(),
            next: waiting.then_some(self.next),
        }
    }
}

/// Milliseconds from `from` to now, for the log.
fn since(from: Timestamp) -> i64 {
    Timestamp::now().as_millisecond().saturating_sub(from.as_millisecond())
}

pub(crate) fn directory(node: &Node, id: &ResidentId) -> io::Result<fastctx_types::layout::ConversationDir> {
    let directory = node.world.conversation(id);
    crate::records::legacy::directory::<Progress>(&directory, "host-progress", &directory.book("host-progress"))?;
    Ok(directory)
}
pub(crate) fn progress_now(node: &Node, id: &ResidentId, input: &NoticeId) -> io::Result<Option<Progress>> {
    let directory = directory(node, id)?;
    crate::records::read(&directory, |rows| rows.get("host-progress", input.as_str()))
}
/// `progress_now`, read where reading may block.
pub(crate) async fn progress(node: &Arc<Node>, id: &ResidentId, input: &NoticeId) -> io::Result<Option<Progress>> {
    let (node, id, input) = (node.clone(), id.clone(), input.clone());
    fastctx_platform::task::blocking(move || progress_now(&node, &id, &input)).await
}
fn initial() -> Progress {
    let now = Timestamp::now();
    Progress {
        began: now,
        next: now,
        failures: 0,
        reason: None,
        attempted: false,
        accepted: false,
        after_activation: None,
        observed: None,
        attempted_at: None,
        accepted_at: None,
    }
}
fn update<T: Send + 'static>(
    node: &Node,
    id: &ResidentId,
    input: &NoticeId,
    create: bool,
    apply: impl FnOnce(&mut Progress) -> fastctx_storage::Result<T> + Send + 'static,
) -> Result<Option<T>, Failure> {
    let directory = directory(node, id).map_err(failure)?;
    let key = input.to_string();
    crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, 2048, move |rows| {
        let mut progress = match rows.get::<Progress>("host-progress", &key)? {
            Some(row) => row,
            None if create => initial(),
            None => return Ok(None),
        };
        let result = apply(&mut progress)?;
        rows.put("host-progress", &key, &progress)?;
        Ok(Some(result))
    })
    .map_err(failure)
}
/// Preflight is durable before Waking is published.
pub(crate) fn begin(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    update(node, id, input, true, |_| Ok(())).map(|_| ())
}
pub(crate) fn begin_after(
    node: &Node,
    id: &ResidentId,
    input: &NoticeId,
    activation: Option<String>,
) -> Result<(), Failure> {
    update(node, id, input, true, move |row| {
        row.after_activation = activation;
        Ok(())
    })
    .map(|_| ())
}
/// Commit the host boundary before issuing a command that could accept words.
pub(crate) fn attempting(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    let row = update(node, id, input, false, |row| {
        row.attempted = true;
        row.accepted = false;
        row.attempted_at = Some(Timestamp::now());
        Ok(row.clone())
    })?
    .ok_or_else(|| failure("missing host preflight"))?;
    node.log.line(format_args!(
        "lifecycle {id}: launching its host, {} ms after the start began (try {})",
        since(row.began),
        row.failures + 1
    ));
    Ok(())
}
pub(crate) fn accepted(node: &Node, id: &ResidentId, input: &NoticeId) -> Result<(), Failure> {
    let row = update(node, id, input, false, |row| {
        row.accepted = true;
        row.accepted_at = Some(Timestamp::now());
        Ok(row.clone())
    })?;
    if let Some(row) = row {
        node.log.line(format_args!(
            "lifecycle {id}: its host took the words, {} ms after the start began, {} ms after launching",
            since(row.began),
            row.attempted_at.map_or(0, since)
        ));
    }
    Ok(())
}

/// Logs how long a start took, phase by phase, once a turn took its words.
pub(crate) fn started(node: &Node, id: &ResidentId, progress: &Progress) {
    let at = |phase: Option<Timestamp>| phase.map_or_else(|| "-".to_string(), |at| format!("{} ms", since(at)));
    node.log.line(format_args!(
        "lifecycle {id}: a turn took the words {} ms after the start began ({} since launching, {} since its host took them; {} failed tries)",
        since(progress.began),
        at(progress.attempted_at),
        at(progress.accepted_at),
        progress.failures
    ));
}

pub(crate) async fn unavailable(
    node: &Arc<Node>,
    id: &ResidentId,
    input: &NoticeId,
    error: super::wake::WakeFailure,
) -> Result<Landed, Failure> {
    if error.permanent {
        super::refused::reject(node, id, input, error.failure).await
    } else if crate::claude::wake_in_flight(node, id, &error.failure) {
        // A wake already on its way is no failure of these words: they wait
        // for it, looked at again within moments and at once as it settles
        // (`due_now`), never put off longer for each look.
        let (node, id, input, reason) = (node.clone(), id.clone(), input.clone(), error.failure.message);
        fastctx_platform::task::blocking(move || {
            queued_now(&node, &id, &input, &reason)?;
            crate::inbox::set_state(&node.world.conversation(&id), &input, PendingState::Waiting).map_err(failure)?;
            kick(&node, &id);
            Ok(Landed::Waiting { why: None, answered: true })
        })
        .await
    } else {
        defer(node, id, input, &error.failure.message).await
    }
}

/// Makes the handing over of the conversation's next waiting words due
/// now, where they wait for what has settled (their last try said `why`):
/// words put off for a failure of their own wait out their time.
pub(crate) async fn due_now(node: &Arc<Node>, id: &ResidentId, why: &'static str) {
    let (working, resident) = (node.clone(), id.clone());
    let made = fastctx_platform::task::blocking(move || {
        let (node, id) = (working, resident);
        let conversation = node.world.conversation(&id);
        let Some(next) =
            crate::inbox::first(&conversation, |row| row.state == PendingState::Waiting).map_err(failure)?
        else {
            return Ok(());
        };
        update(&node, &id, &next.delivery.id, false, move |row| {
            if !row.attempted && row.reason.as_deref() == Some(why) && row.next > Timestamp::now() {
                row.next = Timestamp::now();
            }
            Ok(())
        })
        .map(|_| ())
    })
    .await;
    // Unwritten, the words are looked at again on their own time all the same.
    if let Err(error) = made {
        node.log.line(format_args!("lifecycle {id}: hurrying its waiting words: {}", error.message));
    }
}

/// The caller owns the action and management gates and retries its durable work.
/// Its scheduler may check sooner, but only this progress permits another wake.
pub(crate) async fn prepare_work(node: &Arc<Node>, id: &ResidentId, key: &NoticeId) -> Result<(), Failure> {
    if progress(node, id, key).await.map_err(failure)?.is_some_and(|progress| progress.next > Timestamp::now()) {
        return Err(Failure::host_unavailable("the host retry is waiting for its next attempt"));
    }
    match super::wake::wake(node, id, super::wake::Source::Control, None).await {
        Ok(_) => clear(node, id, key).await.map_err(failure),
        Err(error) => {
            if !error.permanent {
                failed(node, id, key, &error.failure.message).await?;
            } else {
                clear(node, id, key).await.map_err(failure)?;
            }
            Err(error.control())
        }
    }
}

/// Call only with evidence that this input has not crossed the host boundary.
pub(crate) fn defer_now(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<Landed, Failure> {
    failed_now(node, id, input, reason)?;
    crate::inbox::set_state(&node.world.conversation(id), input, PendingState::Waiting).map_err(failure)?;
    kick(node, id);
    Ok(Landed::Waiting { why: None, answered: true })
}
/// `defer_now`, written where writing may block.
pub(crate) async fn defer(
    node: &Arc<Node>,
    id: &ResidentId,
    input: &NoticeId,
    reason: &str,
) -> Result<Landed, Failure> {
    let (node, id, input, reason) = (node.clone(), id.clone(), input.clone(), reason.to_string());
    fastctx_platform::task::blocking(move || defer_now(&node, &id, &input, &reason)).await
}

/// `failed_now`, written where writing may block.
pub(crate) async fn failed(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    let (node, id, input, reason) = (node.clone(), id.clone(), input.clone(), reason.to_string());
    fastctx_platform::task::blocking(move || failed_now(&node, &id, &input, &reason)).await
}

pub(crate) fn failed_now(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    let why = reason.to_string();
    let changed = update(node, id, input, true, move |row| {
        let changed = row.reason.as_deref() != Some(&why);
        row.reason = Some(why);
        row.attempted = false;
        row.accepted = false;
        row.failures = row.failures.saturating_add(1);
        let seconds = 3u64.saturating_mul(1u64 << row.failures.saturating_sub(1).min(5)).min(60);
        row.next = Timestamp::now()
            .checked_add(Duration::from_secs(seconds))
            .map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
        Ok(changed)
    })?
    .unwrap_or(false);
    if changed {
        node.log.line(format_args!("lifecycle {id}: host unavailable; retrying: {reason}"));
    }
    Ok(())
}
/// `queued_now`, written where writing may block.
pub(crate) async fn queued(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    let (node, id, input, reason) = (node.clone(), id.clone(), input.clone(), reason.to_string());
    fastctx_platform::task::blocking(move || queued_now(&node, &id, &input, &reason)).await
}
pub(crate) fn queued_now(node: &Arc<Node>, id: &ResidentId, input: &NoticeId, reason: &str) -> Result<(), Failure> {
    let why = reason.to_string();
    update(node, id, input, true, move |row| {
        row.reason = Some(why);
        row.attempted = false;
        row.accepted = false;
        row.next = Timestamp::now()
            .checked_add(Duration::from_secs(2))
            .map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
        Ok(())
    })
    .map(|_| ())
}
pub(crate) fn observing(node: &Node, id: &ResidentId, input: &NoticeId, reason: String) -> Result<(), Failure> {
    let why = reason.clone();
    let changed = update(node, id, input, true, move |row| {
        let changed = row.observed.as_ref() != Some(&why);
        if changed {
            row.observed = Some(why);
        }
        Ok(changed)
    })?
    .unwrap_or(false);
    if changed {
        node.log.line(format_args!("lifecycle {id}: observing startup: {reason}"));
    }
    Ok(())
}
/// `clear_now`, written where writing may block.
pub(crate) async fn clear(node: &Arc<Node>, id: &ResidentId, input: &NoticeId) -> io::Result<()> {
    let (node, id, input) = (node.clone(), id.clone(), input.clone());
    fastctx_platform::task::blocking(move || clear_now(&node, &id, &input)).await
}
pub(crate) fn clear_now(node: &Node, id: &ResidentId, input: &NoticeId) -> io::Result<()> {
    let directory = directory(node, id)?;
    let key = input.to_string();
    crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, key.len(), move |rows| {
        rows.delete("host-progress", &key).map(|_| ())
    })
}

/// Asks for the conversation's waiting work to be handed to its host, as a
/// job (`fastctx_jobs`): one for the conversation however often it is asked.
pub(crate) fn kick(node: &Node, id: &ResidentId) {
    let job = fastctx_jobs::Job { kind: HANDING, key: id.to_string(), unit: id.to_string(), payload: Vec::new() };
    let (database, jobs) = (node.database.clone(), node.durable_jobs.clone());
    fastctx_platform::task::spawn(async move {
        let bytes = job.key.len() * 2 + 64;
        if database.write_relaxed(bytes, move |db| fastctx_jobs::submit(db, &job)).await.is_ok() {
            jobs.stir(HANDING);
        }
    });
}

/// Host readiness and consuming-turn receipts release waiting arrivals
/// immediately: a handing over that waits to be tried again is due now.
/// Where none waits there is nothing to hurry.
pub(crate) fn changed(node: &Node, id: &ResidentId) {
    if node.retrying.0.lock().expect("never poisoned").contains_key(id) {
        kick(node, id);
    }
}

/// Names what hands waiting work to hosts, before the node's jobs start.
pub(crate) fn register(node: &Arc<Node>) {
    use fastctx_jobs::{Kind, Outcome, Taken};
    let jobs = node.durable_jobs.clone();
    let node = Arc::downgrade(node);
    let run = move |taken: Taken| -> std::pin::Pin<Box<dyn std::future::Future<Output = Outcome> + Send>> {
        let node = node.upgrade();
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            let Ok(id) = taken.unit.parse::<ResidentId>() else { return Outcome::Done };
            match step(&node, &id).await {
                Some(after) => Outcome::Again { after },
                None => {
                    node.retrying.0.lock().expect("never poisoned").remove(&id);
                    Outcome::Done
                }
            }
        })
    };
    jobs.register(
        Kind { name: HANDING, at_once: HANDED_AT_ONCE, within: HANDS_WITHIN, tries: HAND_TRIES },
        Arc::new(run),
    );
}

/// Tells trouble with a conversation's waiting work once for as long as it
/// stays the same.
fn troubled(node: &Node, id: &ResidentId, error: String) {
    let mut retrying = node.retrying.0.lock().expect("never poisoned");
    let said = retrying.entry(id.clone()).or_default();
    if said.as_ref() != Some(&error) {
        node.log.line(format_args!("lifecycle {id}: reading pending work; retrying: {error}"));
        *said = Some(error);
    }
}

/// One step of handing the conversation's waiting work to its host: none
/// when nothing is left to hand over, and otherwise how long until the next
/// step, which `changed` may hurry.
async fn step(node: &Arc<Node>, id: &ResidentId) -> Option<Duration> {
    node.ready_for(id).await;
    node.residents.active(id).ok()?;
    let (about, _) = node.residents.about(id)?;
    let directory = node.world.conversation(id);
    // Marked as waiting before anything is read: a host that becomes ready
    // meanwhile then hurries the step after this one.
    node.retrying.0.lock().expect("never poisoned").entry(id.clone()).or_default();
    let input = match first_ready(node, &about, &directory).await {
        Ok(Some(input)) => input,
        Ok(None) => match crate::control_queue::waiting_for_host(node, id).await {
            Ok(true) => {
                if let Err(error) = crate::control_queue::run_pending(node, id).await {
                    node.log.line(format_args!("lifecycle {id}: retrying control: {error}"));
                }
                return Some(LOOKS_AGAIN);
            }
            Ok(false) => return None,
            Err(error) => {
                node.log.line(format_args!("lifecycle {id}: reading pending controls: {error}"));
                return Some(LOOKS_AGAIN);
            }
        },
        Err(error) => {
            troubled(node, id, error.to_string());
            return Some(LOOKS_AGAIN);
        }
    };
    match progress(node, id, &input.delivery.id).await {
        Ok(Some(progress)) => {
            let remaining = progress.next.as_millisecond().saturating_sub(Timestamp::now().as_millisecond());
            if remaining > 0 {
                return Some(Duration::from_millis(remaining.min(60_000) as u64));
            }
        }
        Ok(None) => {}
        Err(error) => {
            node.log.line(format_args!("lifecycle {id}: cannot read host progress: {error}"));
            return Some(LOOKS_AGAIN);
        }
    }
    let (about, _) = node.residents.about(id)?;
    if let Err(error) = crate::speech::hand_over(node, id, &about, &directory, input.clone()).await {
        // A failure after handover is not permission to repeat it.
        // Only the still-Waiting state proves no submission happened.
        if still_waiting(&directory, &input.delivery.id).await {
            if let Err(error) = defer(node, id, &input.delivery.id, &error.message).await {
                node.log.line(format_args!("lifecycle {id}: keeping retry: {error}"));
            }
        } else {
            node.log.line(format_args!("lifecycle {id}: handover needs reconciliation: {error}"));
        }
    }
    // What still waits is looked at again in a while or when its host is
    // ready; what went in makes way for the next at once.
    Some(if still_waiting(&directory, &input.delivery.id).await { LOOKS_AGAIN } else { Duration::ZERO })
}

/// The first of what waits in `directory` that may go in now, read where
/// reading may block.
async fn first_ready(node: &Arc<Node>, about: &About, directory: &ConversationDir) -> io::Result<Option<Pending>> {
    let (node, about, directory) = (node.clone(), about.clone(), directory.clone());
    fastctx_platform::task::blocking(move || {
        crate::inbox::first(&directory, |row| {
            row.state == PendingState::Waiting
                && row.delivery.notice.along().is_none()
                && crate::speech::ready_for_delivery(&node, &about, row)
        })
    })
    .await
}

/// Whether `input` still waits to go in: only that proves it was not handed
/// over, so only then is it put off for later.
pub(crate) async fn still_waiting(directory: &ConversationDir, input: &NoticeId) -> bool {
    let (directory, input) = (directory.clone(), input.clone());
    fastctx_platform::task::blocking(move || {
        crate::inbox::read(&directory, &input)
            .is_ok_and(|row| row.is_some_and(|row| row.state == PendingState::Waiting))
    })
    .await
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("keeping host progress: {error}"))
}
