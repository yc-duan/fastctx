//! Durable between-turn controls and a shared admission order for words and controls.

use crate::Node;
pub(crate) mod storage;
use fastctx_platform::doing::{Mutex, Rank};
use fastctx_types::ipc::FailureCode;
use fastctx_types::session::control::{Control, ControlState, Controlled, PendingControl};
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use fastctx_types::{ResidentId, ipc::Failure};
use std::sync::Arc;
use std::time::Duration;
pub(crate) use storage::pending;

/// How long a Codex compaction may take before what came of it is not known.
pub(crate) const COMPACTING: Duration = Duration::from_secs(30 * 60);
/// How long the turn of a Codex compaction may take to begin.
pub(crate) const COMPACTION_BEGINS: Duration = Duration::from_secs(2 * 60);

pub(crate) type Actions = crate::leases::Registry<ResidentId, Mutex<()>>;

pub(crate) fn gate(node: &Node, id: &ResidentId) -> crate::leases::Lease<ResidentId, Mutex<()>> {
    node.actions.get_or_insert_with(id, || Mutex::new(Rank::ACTION, "conversation actions", id.to_string(), ()))
}

async fn event(node: &Arc<Node>, id: &ResidentId, row: &PendingControl) -> Result<(), Failure> {
    storage::tell(node, id, row, false, false).await
}

/// Whether a control is told in its conversation as it begins and as it
/// ends (`world-gui.md`, 对话): all a user does to it, but answering it,
/// reading what it offers and withdrawing one that waits.
pub(crate) fn told(control: &Control) -> bool {
    !matches!(
        control,
        Control::Answer { .. } | Control::CancelControl { .. } | Control::Agents | Control::AgentMessages { .. }
    ) && !matches!(control, Control::Command { name, args, path: None } if name == "model" && args.trim().is_empty())
}

/// A control done now, told in its conversation as begun.
pub(crate) async fn begin(node: &Arc<Node>, id: &ResidentId, control: &Control) -> Result<PendingControl, Failure> {
    let row = PendingControl {
        id: crate::speech::new_id("control").to_string(),
        control: control.clone(),
        at: jiff::Timestamp::now(),
        state: ControlState::Running,
        result: None,
        error: None,
    };
    storage::tell(node, id, &row, false, true).await?;
    Ok(row)
}

/// Tells how a control told as begun ended, and gives its result back, a
/// failure marked as told there.
pub(crate) async fn end(
    node: &Arc<Node>,
    id: &ResidentId,
    row: PendingControl,
    result: Result<Controlled, Failure>,
) -> Result<Controlled, Failure> {
    // Its row and the retries it settles are records, reached where that may block.
    let (node, id) = (node.clone(), id.clone());
    crate::blocking::on(async move { end_now(&node, &id, row, result).await }).await
}

async fn end_now(
    node: &Arc<Node>,
    id: &ResidentId,
    row: PendingControl,
    result: Result<Controlled, Failure>,
) -> Result<Controlled, Failure> {
    if let Err(error) = &result
        && error.code == Some(FailureCode::HostUnavailable)
    {
        defer_host(node, id, &row, &error.message).await?;
        return Ok(Controlled::Reported { said: None, queued: Some(row.id) });
    }
    settle(node, id, row.clone(), &result).await?;
    crate::composer::forget_command(node, id, &row.id).await?;
    crate::lifecycle::retry::clear(node, id, &control_key(&row)?).map_err(failure)?;
    node.watchers.changed.notify_one();
    result.map_err(|mut failure| {
        failure.code.get_or_insert(FailureCode::Told);
        failure
    })
}

fn control_key(row: &PendingControl) -> Result<fastctx_types::NoticeId, Failure> {
    row.id.parse().map_err(|error| Failure::new(format!("invalid control identity: {error}")))
}

/// The durable row stays Waiting while its host is being prepared. A restart
/// must not confuse that phase with a control that might have executed.
pub(crate) async fn prepare(node: &Arc<Node>, id: &ResidentId, row: &PendingControl) -> Result<(), Failure> {
    let mut waiting = row.clone();
    waiting.state = ControlState::Waiting;
    storage::put(node, id, waiting).await?;
    crate::composer::place_command(node, id, &control_key(row)?).await?;
    ready(node, id, row).await
}

async fn ready(node: &Arc<Node>, id: &ResidentId, row: &PendingControl) -> Result<(), Failure> {
    let key = control_key(row)?;
    if crate::lifecycle::retry::progress(node, id, &key)
        .map_err(failure)?
        .is_some_and(|progress| progress.next > jiff::Timestamp::now())
    {
        return Err(Failure::host_unavailable("the host retry is waiting for its next attempt"));
    }
    let gate = node.residents.management(id)?;
    let _reading = crate::gate_waits::shared(node, id, &gate).await?;
    crate::lifecycle::wake::wake(node, id, crate::lifecycle::wake::Source::Control, None)
        .await
        .map_err(crate::lifecycle::wake::WakeFailure::control)?;
    storage::running(node, id, &row.id).await
}

async fn defer_host(node: &Arc<Node>, id: &ResidentId, row: &PendingControl, why: &str) -> Result<(), Failure> {
    let key = control_key(row)?;
    let mut waiting = row.clone();
    waiting.state = ControlState::Waiting;
    storage::put(node, id, waiting).await?;
    crate::composer::place_command(node, id, &key).await?;
    // One waiting for a turn of its login asks again soon, to keep its
    // place in line; a host that is not there is asked less and less often.
    if why == crate::turn_slots::FULL {
        crate::lifecycle::retry::queued(node, id, &key, why)?;
    } else {
        crate::lifecycle::retry::failed(node, id, &key, why)?;
    }
    crate::lifecycle::retry::kick(node, id);
    Ok(())
}

pub(crate) async fn waiting_for_host(node: &Arc<Node>, id: &ResidentId) -> Result<bool, Failure> {
    let rows = pending(node, id).await?;
    let (node, id) = (node.clone(), id.clone());
    tokio::task::spawn_blocking(move || {
        for row in rows {
            if row.state == ControlState::Waiting
                && crate::lifecycle::retry::progress(&node, &id, &control_key(&row)?).map_err(failure)?.is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
}

pub(crate) async fn run_pending(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let actions = gate(node, id);
    let _action = actions.lock().await;
    drain(node, id).await
}

/// Tells how a control ended: at once, or, for a Codex compaction, which
/// its host only starts, once the turn it began has ended.
async fn settle(
    node: &Arc<Node>,
    id: &ResidentId,
    mut row: PendingControl,
    result: &Result<Controlled, Failure>,
) -> Result<(), Failure> {
    match result {
        Ok(done) if compacts_later(node, id, &row.control, done) => {
            after_compaction(node.clone(), id.clone(), row);
            return Ok(());
        }
        Ok(done) => {
            row.state = ControlState::Completed;
            row.result = Some(Box::new(done.clone()));
        }
        Err(error) => {
            row.state = if error.is_control_uncertain() { ControlState::Uncertain } else { ControlState::Failed };
            row.error = Some(error.message.clone());
        }
    }
    finish(node, id, &row).await
}

/// Whether what a control answered is only the start of a compaction that
/// ends with a turn of its own: Codex's.
fn compacts_later(node: &Node, id: &ResidentId, control: &Control, done: &Controlled) -> bool {
    matches!(done, Controlled::Done { .. })
        && (matches!(control, Control::Compact)
            || matches!(control, Control::Command { name, path: None, .. } if name == "compact"))
        && node.residents.about(id).is_some_and(|(about, _)| about.host.agent.as_str() == "codex")
}

/// Tells the compaction a control started once its turn has ended: done
/// where it compacted, failed where it did not.
fn after_compaction(node: Arc<Node>, id: ResidentId, mut row: PendingControl) {
    tokio::spawn(async move {
        let began = std::time::Instant::now();
        let mut seen = false;
        loop {
            let Some(now) = node.residents.now(&id) else { return };
            let compacted = node.residents.last_compaction(&id).is_some_and(|at| at > row.at);
            seen |= now.turn.is_some() || compacted;
            if seen && now.turn.is_none() && now.since > row.at {
                if compacted {
                    row.state = ControlState::Completed;
                    row.result = Some(Box::new(Controlled::Done { said: None }));
                } else {
                    // A process exit can synthesize the turn's end before its
                    // compaction receipt arrives. No boundary is no proof of failure.
                    row.state = ControlState::Uncertain;
                    row.error = Some(
                        now.trouble
                            .map(|trouble| trouble.text)
                            .unwrap_or_else(|| "the compaction's turn ended without confirming its outcome".into()),
                    );
                }
                break;
            }
            let waited = began.elapsed();
            if waited > COMPACTING || (!seen && waited > COMPACTION_BEGINS) {
                row.state = ControlState::Uncertain;
                row.error = Some("no end of the compaction was heard from its host".into());
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if let Err(error) = finish(&node, &id, &row).await {
            node.log.line(format_args!("telling {id} how its compaction ended: {}", error.message));
        }
        schedule(&node, &id);
    });
}

/// Controls done only between turns, waiting for the one under way: those
/// that change what the session is, and a new name, which reaches into the
/// host's own record of the session and is never written while the host
/// writes it too.
pub(crate) fn between_turns(control: &Control) -> bool {
    matches!(control, Control::Compact | Control::Rewind { .. } | Control::Fork { .. } | Control::Rename { .. })
        || matches!(control, Control::Command { name, path: None, .. }
            if matches!(name.as_str(), "compact" | "review" | "code-review" | "fork" | "rename"))
}

pub(crate) fn busy(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    if node.residents.now(id).is_some_and(|now| now.turn.is_some()) || node.long_controls.of(id) {
        return Ok(true);
    }
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown conversation"))?;
    if about.host.agent.as_str() == "codex"
        && let Some(thread) = about.session.as_deref()
    {
        return Ok(node.codex.of(node, &about)?.busy(thread));
    }
    Ok(false)
}

/// Called with the action gate held. No gate is held while waiting for a turn to end.
pub(crate) async fn enqueue_if_busy(
    node: &Arc<Node>,
    id: &ResidentId,
    control: &Control,
) -> Result<Option<Controlled>, Failure> {
    // While a long control runs, every control told in the conversation
    // waits for it, not only those that wait for a turn.
    if !(between_turns(control) || (told(control) && node.long_controls.of(id))) {
        return Ok(None);
    }
    // A fork through a turn already ended takes that turn's history as it
    // stands, which the turn under way does not change: it is made at once,
    // the conversation going on undisturbed. A fork of the
    // whole, or through the turn under way, waits for that turn to end.
    if let Control::Fork { through: Some(turn), .. } = control
        && !node.long_controls.of(id)
        && node.residents.now(id).is_none_or(|now| now.turn.as_deref() != Some(turn.as_str()))
    {
        return Ok(None);
    }
    // Ones still waiting behind queued words keep their place ahead of it;
    // words handed over whose turn has not begun yet are a turn under way.
    if !busy(node, id)? && pending(node, id).await?.is_empty() && !words_ahead(node, id, None).await? {
        return Ok(None);
    }
    let row = PendingControl {
        id: crate::speech::new_id("control").to_string(),
        control: control.clone(),
        at: jiff::Timestamp::now(),
        state: ControlState::Waiting,
        result: None,
        error: None,
    };
    storage::put(node, id, row.clone()).await?;
    // Its place among the words that wait: after them, until moved.
    let key = fastctx_types::NoticeId::try_from(row.id.clone()).map_err(|error| Failure::new(error.to_string()))?;
    crate::composer::place_command(node, id, &key).await?;
    event(node, id, &row).await?;
    schedule(node, id);
    Ok(Some(Controlled::Reported { said: Some("Waiting for the current turn to end.".into()), queued: Some(row.id) }))
}

/// Cancellation competes with the durable claim, not the long-running host operation.
pub(crate) async fn cancel(node: &Arc<Node>, id: &ResidentId, key: &str) -> Result<Controlled, Failure> {
    node.residents.active(id)?;
    let row = storage::cancel(node, id, key).await?;
    crate::composer::forget_command(node, id, &row.id).await?;
    schedule(node, id);
    Ok(Controlled::Done { said: Some("The waiting control was withdrawn.".into()) })
}

async fn finish(node: &Arc<Node>, id: &ResidentId, row: &PendingControl) -> Result<(), Failure> {
    storage::tell(node, id, row, true, false).await
}

/// Runs older waiting controls before a newly admitted action, while holding its action gate.
pub(crate) fn drain<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Failure>> + Send + 'a>> {
    // The queue and the retries it reads are records, reached where that may block.
    let (node, id) = (node.clone(), id.clone());
    Box::pin(crate::blocking::on(async move { drain_now(&node, &id).await }))
}

fn drain_now<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Failure>> + Send + 'a>> {
    Box::pin(crate::gate_waits::internal(Box::pin(async move {
        loop {
            node.residents.active(id)?;
            if busy(node, id)? {
                return Ok(());
            }
            let rows = pending(node, id).await?;
            // Startup recovery settles abandoned attempts before admission opens.
            // A running row here belongs to this process, including a compaction
            // whose TurnStarted or completion notification has not arrived yet.
            if rows.iter().any(|row| row.state == ControlState::Running) {
                return Ok(());
            }
            if let Some(row) = rows.iter().find(|row| row.state != ControlState::Waiting) {
                finish(node, id, row).await?;
                crate::composer::forget_command(node, id, &row.id).await?;
                continue;
            }
            // Words before it in the one order go first; it waits for
            // their turn to end.
            let waiting: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
            let Some(next) = crate::composer::next_command(node, id, &waiting).await? else {
                return Ok(());
            };
            let Some(first) = rows.iter().find(|row| row.id == next) else {
                return Ok(());
            };
            if words_ahead(node, id, Some(first.at)).await? {
                return Ok(());
            }
            let key = control_key(first)?;
            if crate::lifecycle::retry::progress(node, id, &key)
                .map_err(failure)?
                .is_some_and(|progress| progress.next > jiff::Timestamp::now())
            {
                return Ok(());
            }
            if let Err(error) = ready(node, id, first).await {
                // One that cannot be done ends with why, and the queue goes
                // on; one that may be done later keeps its place and is
                // asked again.
                if crate::obligations::permanent(&error) {
                    let _ = end(node, id, first.clone(), Err(error)).await;
                    continue;
                }
                defer_host(node, id, first, &error.message).await?;
                return Ok(());
            }
            let mut row = first.clone();
            row.state = ControlState::Running;
            event(node, id, &row).await?;
            // A long one runs on its own as one given directly does: the
            // gate let go, words and other controls waiting for it, the rest
            // of the queue taken up as it ends (`crate::control_long`).
            if crate::control_long::long(node, id, &row.control) {
                crate::control_long::start(node, id, row);
                return Ok(());
            }
            let result = Box::pin(crate::control::execute(node, id, row.control.clone())).await;
            let deferred = result.as_ref().err().is_some_and(|error| error.code == Some(FailureCode::HostUnavailable));
            // How it ended is told in its conversation; the queue goes on.
            if let Err(error) = end(node, id, row, result).await
                && error.code != Some(FailureCode::Told)
            {
                return Err(error);
            }
            if deferred {
                return Ok(());
            }
        }
    })))
}

struct Scheduled {
    node: Arc<Node>,
    id: ResidentId,
    released: bool,
}
impl Drop for Scheduled {
    fn drop(&mut self) {
        if !self.released {
            self.node.controls_due.lock().expect("control scheduling").remove(&self.id);
        }
    }
}

pub(crate) fn schedule(node: &Arc<Node>, id: &ResidentId) {
    {
        use std::collections::hash_map::Entry;
        let mut due = node.controls_due.lock().expect("control scheduling");
        match due.entry(id.clone()) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() = true;
                return;
            }
            Entry::Vacant(entry) => {
                entry.insert(false);
            }
        }
    }
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        let mut scheduled = Scheduled { node: node.clone(), id: id.clone(), released: false };
        node.ready_for(&id).await;
        let (mut again, mut said) = (crate::again::Again::default(), String::new());
        loop {
            let failed = {
                let gate = gate(&node, &id);
                let _action = gate.lock().await;
                match node.residents.active(&id) {
                    Ok(()) => drain(&node, &id).await.err(),
                    Err(_) => None,
                }
            };
            // A pass that failed, as to keep how a control ended, is made
            // again later rather than left until something else asks.
            if let Some(error) = failed {
                if error.message != said {
                    node.log.line(format_args!("executing {id}'s queued controls; trying again: {error}"));
                    said = error.message;
                }
                tokio::time::sleep(again.next()).await;
                continue;
            }
            let mut due = node.controls_due.lock().expect("control scheduling");
            if due.get(&id) == Some(&true) {
                due.insert(id.clone(), false);
            } else {
                due.remove(&id);
                scheduled.released = true;
                break;
            }
        }
    });
}

/// Whether words handed to the host (before `before`, where given) have not
/// been taken into a turn yet: a control waits for that turn.
async fn words_ahead(node: &Node, id: &ResidentId, before: Option<jiff::Timestamp>) -> Result<bool, Failure> {
    let directory = node.world.conversation(id);
    tokio::task::spawn_blocking(move || {
        for words in crate::inbox::pending(&directory).map_err(failure)? {
            if words.delivery.notice.along().is_none()
                && before.is_none_or(|before| words.delivery.at < before)
                && crate::delivery::input_turn(&directory, &words.delivery.id).map_err(failure)?.is_none()
                && crate::delivery::input_uncertainty(&directory, &words.delivery.id).map_err(failure)?.is_none()
            {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
}

/// Drains the controls waiting, if any: what held them back may have gone.
pub(crate) fn poke(node: &Arc<Node>, id: &ResidentId) {
    schedule(node, id);
}

pub(crate) async fn recover(node: &Arc<Node>, id: &ResidentId) {
    let result:Result<(),Failure>=async {
        for mut row in pending(node,id).await? {
            if row.state==ControlState::Running {
                row.state=ControlState::Uncertain;
                row.error=Some("the node stopped while this control was entering the host; its outcome is unknown and it was not repeated".into());
                finish(node,id,&row).await?;
            } else if row.state!=ControlState::Waiting {finish(node,id,&row).await?;}
        }
        Ok(())
    }.await;
    if let Err(error) = result {
        node.log.line(format_args!("recovering {id}'s controls: {error}"));
    }
    schedule(node, id);
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(format!("recording conversation controls: {error}"))
}
