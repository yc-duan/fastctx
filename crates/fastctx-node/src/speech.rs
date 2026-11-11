//! Words and facts on their way to a resident (`world-colleagues.md`, 话怎样
//! 送到居民眼前).
//!
//! What is for a resident of this machine is kept first, with who is owed
//! the end of the turn it goes into (`crate::inbox`), and then goes the way
//! its host lets the World in: into the thread of a Codex resident the World
//! holds, where it starts a turn or joins the running one at its next tool
//! boundary; to the World's plugin in a Claude session, which says which
//! turn took it; otherwise it waits, and the World's tools hand it over with
//! their next answer. It stays kept until the World knows which turn took
//! it. What is for a resident of another machine goes there as a letter,
//! which that machine takes the same way.
//!
//! Words that go into a session make someone owed the end of the turn they
//! go into: whoever said them, or, for a fact of the resident's own work
//! such as a job's end, whoever it is working for.

use std::sync::Arc;

use fastctx_hands::told;
use fastctx_hosts::codex::{Input, SendError};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ConversationDir;
use fastctx_types::remote::Letter;
use fastctx_types::residents::{
    About, Delivery, FIRST_WORDS_CHARS, Landed, MessageKind, MessageLine, Notice, Origin, Outcome, Pending,
    PendingState,
};
use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent, Speaker, TurnHow};
use fastctx_types::{Address, NoticeId, ResidentId};
use jiff::Timestamp;

use crate::Node;

/// Said when something is to be told to a session the user opened outside
/// Fastctx: the World speaks only into the conversations it holds.
pub(crate) const OWN_SESSION: &str =
    "this is a session the user opened outside Fastctx; the World answers its calls and says nothing into it";

/// Who is owed the end of the turn words go into.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Owed {
    /// Whoever said them.
    Speaker(Address),
    /// Whoever the resident is working for.
    Working,
    /// Nobody: the user's own words, which the user watches.
    Nobody,
}

/// Tells `to` something. `id` names it, here and in the letter that carries
/// it to another machine; the same id told again is the same notice.
pub async fn tell(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    to: &Address,
    id: NoticeId,
    notice: Notice,
    owed: Owed,
) -> Result<Landed, Failure> {
    tell_at(node, acting, to, &(&to.machine).into(), id, notice, owed).await
}

/// Tells a resident through the hub that answered a location query.
pub(crate) async fn tell_at(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    to: &Address,
    machine: &fastctx_types::access::Written,
    id: NoticeId,
    notice: Notice,
    owed: Owed,
) -> Result<Landed, Failure> {
    if node.is_here(machine) {
        let delivery = Delivery { id, at: Timestamp::now(), notice };
        if parked(node, &to.resident, &delivery)
            .await
            .map_err(|error| Failure::new(format!("keeping the on completion: {error}")))?
        {
            return Ok(Landed::Waiting { why: None, answered: false });
        }
        return tell_here(node, &to.resident, delivery, owed).await;
    }
    let letter = match notice {
        // Words a resident said make it owed the turn they go into, which
        // the receiving machine knows from who sent them.
        Notice::Words {
            from: Speaker::Resident { address },
            text,
            goal_report: None,
            goal_question: None,
            told: None,
        } if node.is_me(&address.machine) && matches!(owed, Owed::Speaker(_)) => {
            Letter::Speak { to: to.resident.clone(), from: address.resident, text, accounts: Vec::new() }
        }
        notice => Letter::Notice { to: to.resident.clone(), notice: Box::new(notice) },
    };
    // Posting writes the letter to the hub's outbox.
    let (sending, acting, machine) = (node.clone(), acting.clone(), machine.clone());
    fastctx_platform::task::blocking(move || crate::letters::send(&sending, &acting, &machine, id, letter))
        .await
        .map_err(|error| Failure::new(format!("sending to {}: {error}", to.machine)))?;
    Ok(Landed::Sent)
}

/// Tells a resident of this machine something: kept first, then handed over
/// the way its host lets the World in.
pub async fn tell_here(node: &Arc<Node>, to: &ResidentId, delivery: Delivery, owed: Owed) -> Result<Landed, Failure> {
    // Keeping and handing over go on to their end if the teller goes away.
    let (node, to) = (node.clone(), to.clone());
    crate::to_end::run(async move { tell_here_now(&node, &to, delivery, owed).await }).await
}

async fn tell_here_now(node: &Arc<Node>, to: &ResidentId, delivery: Delivery, owed: Owed) -> Result<Landed, Failure> {
    let actions = crate::control_queue::gate(node, to);
    let _action = if delivery.notice.along().is_none() {
        Some(crate::gate_waits::action(node, to, &actions).await?)
    } else {
        None
    };
    if _action.is_some() && node.residents.active(to).is_ok() {
        crate::control_queue::drain(node, to).await?;
    }
    let gate = node.residents.management(to)?;
    // Someone's words are asked from outside: one being deleted refuses
    // them at once, and they wait for what changes its life no longer than
    // anything else asked. What comes of work waits as long as it takes.
    let _reading = if matches!(
        delivery.notice,
        Notice::Words { goal_report: None, goal_question: None, told: None, .. } | Notice::Attached { .. }
    ) {
        if node.residents.is_deleting(to) {
            return Err(Failure::new(format!("{to} is being deleted")));
        }
        crate::gate_waits::shared(node, to, &gate).await?
    } else {
        gate.read().await
    };
    tell_here_locked(node, to, delivery, owed).await
}

pub(crate) async fn tell_here_locked(
    node: &Arc<Node>,
    to: &ResidentId,
    delivery: Delivery,
    owed: Owed,
) -> Result<Landed, Failure> {
    let (node, to) = (node.clone(), to.clone());
    crate::to_end::run(async move { tell_here_locked_now(&node, &to, delivery, owed).await }).await
}

async fn tell_here_locked_now(
    node: &Arc<Node>,
    to: &ResidentId,
    delivery: Delivery,
    owed: Owed,
) -> Result<Landed, Failure> {
    if parked(node, to, &delivery).await.map_err(|error| Failure::new(format!("keeping the completion: {error}")))? {
        return Ok(Landed::Waiting { why: None, answered: false });
    }
    receive_here_locked(node, to, delivery, owed).await
}

/// Holds the end of a job for an `on` that waits for it (`on_delivery::park`),
/// where holding it may block; nothing else is held there.
async fn parked(node: &Arc<Node>, to: &ResidentId, delivery: &Delivery) -> std::io::Result<bool> {
    if !matches!(delivery.notice, Notice::JobEnded { .. }) {
        return Ok(false);
    }
    let (node, to, delivery) = (node.clone(), to.clone(), delivery.clone());
    fastctx_platform::task::blocking(move || crate::on_delivery::park(&node, &to, &delivery)).await
}

/// Delivers a completion whose follower decision is already settled. The
/// caller retains its durable copy until this handover has taken custody.
pub(crate) async fn tell_completion(node: &Arc<Node>, to: &ResidentId, delivery: Delivery) -> Result<Landed, Failure> {
    let (node, to) = (node.clone(), to.clone());
    crate::to_end::run(async move { tell_completion_now(&node, &to, delivery).await }).await
}

async fn tell_completion_now(node: &Arc<Node>, to: &ResidentId, delivery: Delivery) -> Result<Landed, Failure> {
    let actions = crate::control_queue::gate(node, to);
    let _action = actions.lock().await;
    if node.residents.active(to).is_ok() {
        crate::control_queue::drain(node, to).await?;
    }
    let gate = node.residents.management(to)?;
    let _reading = gate.read().await;
    receive_here_locked(node, to, delivery, Owed::Working).await
}

/// Takes custody after completion routing, with the conversation gates held.
async fn receive_here_locked(
    node: &Arc<Node>,
    to: &ResidentId,
    delivery: Delivery,
    owed: Owed,
) -> Result<Landed, Failure> {
    // What still comes for one removed or being deleted, as the end of a
    // command its deletion stopped, is kept with its records and wakes it
    // no more; words for it are refused.
    let deleting = node.residents.is_deleting(to);
    let removed = deleting || node.residents.removed(to).is_some();
    if !removed
        || matches!(
            delivery.notice,
            Notice::Words { goal_report: None, goal_question: None, told: None, .. } | Notice::Attached { .. }
        )
    {
        node.residents.active(to)?;
    }
    let (about, _) = node
        .residents
        .about(to)
        .ok_or_else(|| Failure::new(format!("no resident {to} lives on {}", node.machine())))?;
    // A session the user opened outside Fastctx and never brought in is
    // only heard through its own calls (`world-layers.md`): the World says
    // nothing into it. The end of a job it started waits for its next call
    // of a World tool, which carries it back with the answer.
    if !about.world_managed() && !matches!(delivery.notice, Notice::JobEnded { .. }) {
        return Err(Failure::refused(OWN_SESSION));
    }
    let (reading, recipient, input) = (node.clone(), to.clone(), delivery.id.clone());
    let refusal =
        fastctx_platform::task::spawn_blocking(move || crate::lifecycle::refused::get(&reading, &recipient, &input))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(crate::record_migration::failure)?;
    if let Some(error) = refusal {
        return crate::lifecycle::refused::reject(node, to, &delivery.id, error).await;
    }

    let working = matches!(owed, Owed::Working);
    let readers = match owed {
        Owed::Speaker(address) => vec![address],
        Owed::Working => Vec::new(),
        Owed::Nobody => Vec::new(),
    };
    let directory = node.world.conversation(to);
    if let Some(kind) = delivery.notice.along() {
        // Told again before it went in: what has not gone anywhere yet goes
        // no more; what a mod holds, it drops (`claude::into_inbox`).
        let replaced: Vec<NoticeId> = {
            let (directory, told, at, kind) = (directory.clone(), delivery.id.clone(), delivery.at, kind.to_string());
            fastctx_platform::task::spawn_blocking(move || {
                crate::inbox::those(&directory, |kept| {
                    kept.state == PendingState::Waiting
                        && kept.delivery.id != told
                        && kept.delivery.notice.along() == Some(kind.as_str())
                        && kept.delivery.at <= at
                })
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("reading what waits: {error}")))?
            .into_iter()
            .map(|kept| kept.delivery.id)
            .collect()
        };
        if !replaced.is_empty() {
            crate::delivery::local(&directory, move |directory| {
                replaced.iter().try_for_each(|id| crate::inbox::withdraw(directory, id))
            })
            .await
            .map_err(|error| Failure::new(format!("replacing what waits: {error}")))?;
        }
    }
    let pending = Pending { delivery, readers, state: PendingState::Waiting, working, session: None, since: None };
    let kept = {
        let (directory, pending) = (directory.clone(), pending.clone());
        fastctx_platform::task::spawn_blocking(move || crate::inbox::keep(&directory, &pending))
            .await
            .map_err(|error| Failure::new(format!("keeping it: {error}")))?
            .map_err(|error| Failure::new(format!("keeping it: {error}")))?
    };
    if kept {
        node.idle_probes.touched(to);
        crate::goals::engine::input_changed(node, to);
    }
    if removed {
        let why = if deleting {
            "the resident is being deleted, and this with it"
        } else {
            "the resident was removed; this fact is preserved in its archived inbox and will not wake it"
        };
        return Ok(Landed::Waiting { why: Some(why.into()), answered: false });
    }
    if !kept {
        if let Some(why) = uncertainty(&directory, &pending.delivery.id).await? {
            return Ok(Landed::Uncertain { why });
        }
        if matches!(pending.delivery.notice, Notice::Sentinel { .. })
            && let Some(current) = read(&directory, &pending.delivery.id).await?
            && current.state == PendingState::Waiting
        {
            return hand_over_locked(node, to, &about, &directory, current).await;
        }
        // The same thing told again: it is on its way or went in already.
        return Ok(Landed::Waiting { why: None, answered: answered(node, to, &about) });
    }
    // What rides with the next turn waits while none runs; a Claude
    // session's mod holds it for the next one (`along`).
    if pending.delivery.notice.along().is_some() {
        return hand_over_locked(node, to, &about, &directory, pending).await;
    }
    // The end of the resident's own job waits for whatever call is waiting
    // now; words for it end the wait (`world-run.md`).
    if !matches!(pending.delivery.notice, Notice::JobEnded { .. }) {
        node.words.raise(&fastctx_types::Address { machine: node.machine(), resident: to.clone() });
    }
    hand_over_locked(node, to, &about, &directory, pending).await
}

/// A turn of `id` began: what waited to ride with the next turn goes into
/// it now (`Notice::along`).
pub async fn along(node: &Arc<Node>, id: &ResidentId) {
    let Some((about, _)) = node.residents.about(id) else { return };
    let directory = node.world.conversation(id);
    let waiting = {
        let directory = directory.clone();
        let riding = move || {
            crate::inbox::those(&directory, |kept| {
                kept.state == PendingState::Waiting && kept.delivery.notice.along().is_some()
            })
        };
        match fastctx_platform::task::spawn_blocking(riding).await {
            Ok(Ok(waiting)) => waiting,
            Ok(Err(error)) => {
                node.log.line(format_args!("reading what waits for {id}: {error}"));
                return;
            }
            Err(_) => return,
        }
    };
    for pending in waiting {
        if let Err(failure) = hand_over(node, id, &about, &directory, pending).await {
            node.log.line(format_args!("what rides with {id}'s turn waits: {}", failure.message));
        }
    }
}

/// Puts something kept into the resident's session, if its host lets the
/// World in now; otherwise it waits.
pub async fn hand_over(
    node: &Arc<Node>,
    to: &ResidentId,
    about: &About,
    directory: &ConversationDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    let (node, to, about, directory) = (node.clone(), to.clone(), about.clone(), directory.clone());
    crate::to_end::run(async move { hand_over_now(&node, &to, &about, &directory, pending).await }).await
}

/// What waits as `id` in `directory`, read where reading may block.
async fn read(directory: &ConversationDir, id: &NoticeId) -> Result<Option<Pending>, Failure> {
    let id = id.clone();
    crate::delivery::local(directory, move |directory| crate::inbox::read(directory, &id))
        .await
        .map_err(|error| Failure::new(error.to_string()))
}

/// Whether words other than `besides` wake the host in `directory`, read
/// where reading may block.
async fn waking(directory: &ConversationDir, besides: &NoticeId) -> Result<bool, Failure> {
    let besides = besides.clone();
    crate::delivery::local(directory, move |directory| {
        crate::inbox::any(directory, |row| row.state == PendingState::Waking && row.delivery.id != besides)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))
}

/// Why it is not known whether `id` went in, if it is not, read where
/// reading may block.
async fn uncertainty(directory: &ConversationDir, id: &NoticeId) -> Result<Option<String>, Failure> {
    let id = id.clone();
    crate::delivery::local(directory, move |directory| crate::delivery::input_uncertainty(directory, &id))
        .await
        .map_err(|error| Failure::new(error.to_string()))
}

/// The turn `id` went into, if it went into one, read where reading may block.
async fn input_turn(directory: &ConversationDir, id: &NoticeId) -> Result<Option<String>, Failure> {
    let id = id.clone();
    crate::delivery::local(directory, move |directory| crate::delivery::input_turn(directory, &id))
        .await
        .map_err(|error| Failure::new(error.to_string()))
}

/// Keeps `pending` in `directory`, where writing may block: whether it was
/// new there.
async fn keep(directory: &ConversationDir, pending: &Pending) -> Result<bool, Failure> {
    let pending = pending.clone();
    crate::delivery::local(directory, move |directory| crate::inbox::keep(directory, &pending))
        .await
        .map_err(|error| Failure::new(error.to_string()))
}

/// Takes back what waits as `id`, where writing may block.
async fn withdraw(directory: &ConversationDir, id: &NoticeId) -> Result<(), Failure> {
    let id = id.clone();
    crate::delivery::local(directory, move |directory| crate::inbox::withdraw(directory, &id))
        .await
        .map_err(|error| Failure::new(error.to_string()))
}

/// Whether the World holds `about`'s native thread now, as a delivery asks
/// (`codex::native::requested`), where asking may block.
async fn native_requested(node: &Arc<Node>, about: &About) -> Result<bool, Failure> {
    let (node, about) = (node.clone(), about.clone());
    fastctx_platform::task::blocking(move || crate::codex::native::requested(&node, &about)).await
}

async fn hand_over_now(
    node: &Arc<Node>,
    to: &ResidentId,
    about: &About,
    directory: &ConversationDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    let _doing = fastctx_platform::doing::doing(format!("handing {} to {to}", pending.delivery.id));
    // Startup's recovery may need the gate alone, as to finish a deletion:
    // nothing waits for it holding the gate.
    node.ready_for(to).await;
    if crate::composer::restore_before_delivery(node, to).await?
        && read(directory, &pending.delivery.id).await?.is_none()
    {
        return Ok(Landed::Waiting { why: None, answered: true });
    }
    let actions = crate::control_queue::gate(node, to);
    let _action = actions.lock().await;
    crate::control_queue::drain(node, to).await?;
    let gate = node.residents.management(to)?;
    let _reading = gate.read().await;
    hand_over_locked(node, to, about, directory, pending).await
}

/// Closed foreground sessions leave facts for the next visit. Those facts
/// cannot stand in front of a user request that is allowed to resume it.
pub(crate) fn ready_for_delivery(node: &Node, about: &About, pending: &Pending) -> bool {
    about.world_managed()
        || pending.delivery.notice.wakes_closed()
        || node.residents.host_present(&about.id)
        || matches!(pending.delivery.notice, Notice::Words { from: Speaker::User { .. }, .. } | Notice::Attached { .. })
        || crate::lifecycle::retry::progress_now(node, &about.id, &pending.delivery.id)
            .map_or(true, |progress| progress.is_some())
}

async fn hand_over_locked(
    node: &Arc<Node>,
    to: &ResidentId,
    about: &About,
    directory: &ConversationDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    // Background delivery can load a thread before it submits a turn. Keep
    // that opening and its durable outcome inside the handover boundary too.
    let Some(_answering) = node.handoff.call() else {
        return Ok(Landed::Waiting { why: Some(crate::handoff::WAITING.into()), answered: answered(node, to, about) });
    };
    node.ready_for(to).await;
    node.residents.active(to)?;
    // While a long control runs in the host, words wait in the inbox; they
    // go in as it ends (`control_long`).
    if node.long_controls.of(to) {
        return Ok(Landed::Waiting {
            why: Some(crate::control_long::WAITING.into()),
            answered: answered(node, to, about),
        });
    }
    if matches!(pending.delivery.notice, Notice::Sentinel { .. })
        && read(directory, &pending.delivery.id).await?.is_none_or(|current| current.state != PendingState::Waiting)
    {
        return Ok(Landed::Waiting { why: None, answered: answered(node, to, about) });
    }
    if pending.delivery.notice.along().is_some()
        && about.host.agent.as_str() != "claude"
        && node.residents.now(to).is_none_or(|now| now.turn.is_none())
    {
        return Ok(Landed::Waiting { why: None, answered: true });
    }
    let refusal = {
        let (reading, recipient, input) = (node.clone(), to.clone(), pending.delivery.id.clone());
        fastctx_platform::task::blocking(move || crate::lifecycle::refused::get(&reading, &recipient, &input))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
    };
    if let Some(error) = refusal {
        return crate::lifecycle::refused::reject(node, to, &pending.delivery.id, error).await;
    }
    if let Err(error) = crate::lifecycle::wake::check(node, to, about).await {
        return crate::lifecycle::retry::unavailable(node, to, &pending.delivery.id, error).await;
    }
    if pending.delivery.notice.along().is_none() {
        if node.residents.now(to).is_none_or(|now| now.turn.is_none())
            && crate::control_queue::pending(node, to).await?.iter().any(|row| {
                row.state == fastctx_types::session::control::ControlState::Waiting && row.at < pending.delivery.at
            })
        {
            crate::lifecycle::retry::kick(node, to);
            return Ok(Landed::Waiting { why: None, answered: answered(node, to, about) });
        }
        // The CLI's resume prompt has not reached a consuming turn yet.
        // A ready plugin can submit a later envelope ahead of that prompt.
        if waking(directory, &pending.delivery.id).await? {
            crate::lifecycle::retry::kick(node, to);
            return Ok(Landed::Waiting { why: None, answered: answered(node, to, about) });
        }
        // The first that may go in now, and how far its host got with it,
        // read where reading may block.
        let (reading, recipient, host, place) = (node.clone(), to.clone(), about.clone(), directory.clone());
        let first = fastctx_platform::task::blocking(move || {
            let first = crate::inbox::first(&place, |row| {
                row.state == PendingState::Waiting
                    && row.delivery.notice.along().is_none()
                    && ready_for_delivery(&reading, &host, row)
            })?;
            Ok::<_, std::io::Error>(first.map(|first| {
                let progress = crate::lifecycle::retry::progress_now(&reading, &recipient, &first.delivery.id);
                (first, progress)
            }))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?;
        if let Some((first, progress)) = first {
            let progress = match progress {
                Ok(progress) => progress,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    crate::lifecycle::retry::kick(node, to);
                    return Ok(Landed::Waiting { why: Some(error.to_string()), answered: answered(node, to, about) });
                }
                Err(error) => return Err(Failure::new(error.to_string())),
            };
            let delayed = progress.is_some_and(|progress| progress.next > jiff::Timestamp::now());
            if first.delivery.id != pending.delivery.id || delayed {
                crate::lifecycle::retry::kick(node, to);
                return Ok(Landed::Waiting { why: None, answered: answered(node, to, about) });
            }
        }
    }
    crate::goals::colleagues::incoming(node, to, about, &pending).await?;
    if let Err(error) = crate::focus::refresh(node, to).await {
        return crate::lifecycle::retry::defer(node, to, &pending.delivery.id, &error.message).await;
    }
    // Claude's delivery reaches its session's files throughout, where that
    // may block.
    let (delivering, recipient, host, place, claimed) =
        (node.clone(), to.clone(), about.clone(), directory.clone(), pending.clone());
    if let Some(landed) =
        crate::blocking::on(
            async move { crate::claude::deliver(&delivering, &recipient, &host, &place, &claimed).await },
        )
        .await
    {
        return landed;
    }
    let why = None;
    let sentinel = matches!(pending.delivery.notice, Notice::Sentinel { .. });
    if about.world_managed()
        && about.host.agent.as_str() == "codex"
        && let Some(thread) = &about.session
    {
        if !about.world_started && !native_requested(node, about).await? {
            return Ok(Landed::Waiting {
                why: Some("the native host still runs; this message waits at its next World tool boundary".into()),
                answered: false,
            });
        }
        match into_thread(node, to, thread, directory, pending.clone()).await {
            Ok(landed) => {
                if sentinel {
                    crate::sentinels::report(node, &format!("delivery {to}"), None);
                }
                return Ok(landed);
            }
            Err(failure) => {
                if sentinel {
                    crate::sentinels::report(node, &format!("delivery {to}"), Some(failure.message.clone()));
                }
                if crate::lifecycle::retry::still_waiting(directory, &pending.delivery.id).await {
                    return crate::lifecycle::retry::defer(node, to, &pending.delivery.id, &failure.message).await;
                }
                return Err(failure);
            }
        }
    }
    Ok(Landed::Waiting { why, answered: answered(node, to, about) })
}

/// Whether the end of the turn words go into will be told: the World sees
/// the turns of the Codex threads it holds and of the Claude sessions its
/// plugin runs in, no others.
fn answered(node: &Node, to: &ResidentId, about: &About) -> bool {
    match about.host.agent.as_str() {
        "codex" => about.world_managed(),
        "claude" => about.world_started || about.session.is_some() || crate::claude::paired(node, to),
        _ => false,
    }
}

/// Puts what is kept into the thread of a Codex resident the World holds:
/// what waited for a way in first, in order, with `pending`. Whoever each
/// is for is owed the turn they go into.
async fn into_thread(
    node: &Arc<Node>,
    id: &ResidentId,
    thread: &str,
    directory: &ConversationDir,
    pending: Pending,
) -> Result<Landed, Failure> {
    submit_thread(node, id, thread, directory, pending, false, false)
        .await
        .map(|landed| landed.expect("ordinary submissions do not defer to a composer"))
}

/// Submits exactly one composer message. Automatic release rechecks idle while holding the speech lock.
pub async fn send_composer(
    node: &Arc<Node>,
    id: &ResidentId,
    delivery: Delivery,
    idle_only: bool,
) -> Result<Option<Landed>, Failure> {
    node.ready_for(id).await;
    crate::composer::restore_before_delivery(node, id).await?;
    let actions = crate::control_queue::gate(node, id);
    let _action = actions.lock().await;
    crate::control_queue::drain(node, id).await?;
    let gate = node.residents.management(id)?;
    let _reading = gate.read().await;
    node.residents.active(id)?;
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new("unknown resident"))?;
    let directory = node.world.conversation(id);
    if about.world_managed()
        && about.host.agent.as_str() == "codex"
        && let Some(thread) = &about.session
        && (about.world_started || native_requested(node, &about).await?)
    {
        let Some(_answering) = node.handoff.call() else {
            return Err(Failure::new(crate::handoff::WAITING));
        };
        let pending = Pending {
            delivery,
            readers: Vec::new(),
            state: PendingState::Waiting,
            working: false,
            session: None,
            since: None,
        };
        if keep(&directory, &pending).await? {
            node.idle_probes.touched(id);
            crate::goals::engine::input_changed(node, id);
        }
        return submit_thread(node, id, thread, &directory, pending, true, idle_only).await;
    }
    if idle_only && node.residents.now(id).is_some_and(|now| now.activity == fastctx_types::residents::Activity::Busy) {
        return Ok(None);
    }
    tell_here_locked(node, id, delivery, Owed::Nobody).await.map(Some)
}

async fn submit_thread(
    node: &Arc<Node>,
    id: &ResidentId,
    thread: &str,
    directory: &ConversationDir,
    pending: Pending,
    single: bool,
    idle_only: bool,
) -> Result<Option<Landed>, Failure> {
    // Its home's app-server holds its thread.
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::new(format!("{id} has no record here")))?;
    let finding = {
        // Finding its host may read its host mode from its disk.
        let (node, about) = (node.clone(), about.clone());
        fastctx_platform::task::blocking(move || node.codex.of(&node, &about)).await
    };
    let codex = match finding {
        Ok(host) => host,
        Err(error) => {
            return crate::lifecycle::retry::defer(node, id, &pending.delivery.id, &error.message).await.map(Some);
        }
    };
    // Its tools start before the words go in, outside the lock others' words
    // and its turns' ends wait on.
    if single {
        if idle_only
            && (node.residents.now(id).is_some_and(|now| now.turn.is_some()) || codex.running_turn(thread).is_some())
        {
            withdraw(directory, &pending.delivery.id).await?;
            return Ok(None);
        }
        if keep(directory, &pending).await? {
            node.idle_probes.touched(id);
            crate::goals::engine::input_changed(node, id);
        }
    }
    // A turn these words would start waits for room in its login
    // (`turn_slots`); words for a running turn go in with it.
    if pending.delivery.notice.along().is_none()
        && codex.running_turn(thread).is_none()
        && node.residents.now(id).is_none_or(|now| now.turn.is_none())
        && let Err(queued) = crate::turn_slots::admit(node, id, &about)
    {
        crate::lifecycle::retry::kick(node, id);
        return Ok(Some(queued.landed()));
    }
    let missing = match crate::lifecycle::wake::wake(
        node,
        id,
        crate::lifecycle::wake::Source::of(&pending.delivery.notice),
        None,
    )
    .await
    {
        Ok(missing) => missing,
        Err(error) => {
            return crate::lifecycle::retry::unavailable(node, id, &pending.delivery.id, error).await.map(Some);
        }
    };
    let speaking = node.residents.speaking(id).ok_or_else(|| Failure::new(format!("{id} has no record here")))?;
    let _speaking = speaking.lock().await;
    if single
        && idle_only
        && (node.residents.now(id).is_some_and(|now| now.activity == fastctx_types::residents::Activity::Busy)
            || codex.running_turn(thread).is_some())
    {
        withdraw(directory, &pending.delivery.id).await?;
        return Ok(None);
    }
    // A node handing the World on starts no turn; the words wait for its
    // successor, which hands them over when it serves.
    let Some(_starting) = node.handoff.start_turn() else {
        if single {
            return Err(Failure::new(crate::handoff::Handoff::waiting_why(node)));
        }
        return Ok(Some(Landed::Waiting { why: Some(crate::handoff::Handoff::waiting_why(node)), answered: true }));
    };
    if single && keep(directory, &pending).await? {
        crate::goals::engine::input_changed(node, id);
    }
    let Batch { mut batch, after, waits } = {
        let (reading, whose, own) = (node.clone(), id.clone(), pending.delivery.id.clone());
        crate::delivery::local(directory, move |directory| next_batch(&reading, &whose, directory, &own))
            .await
            .map_err(|error| Failure::new(format!("reading what waits: {error}")))?
    };
    if !waits {
        if let Some(why) = uncertainty(directory, &pending.delivery.id).await? {
            return Ok(Some(Landed::Uncertain { why }));
        }
        return match input_turn(directory, &pending.delivery.id).await? {
            Some(turn) => Ok(Some(Landed::JoinedTurn { turn })),
            None => Ok(Some(Landed::Waiting {
                why: Some("this delivery is already recorded; it was not submitted again".to_string()),
                answered: true,
            })),
        };
    }
    let audience = if batch.iter().any(|pending| pending.working)
        && crate::goals::replies::inherits(node, id).await.map_err(|error| Failure::new(error.to_string()))?
    {
        node.residents.working_for(id)
    } else {
        Vec::new()
    };
    for kept in &mut batch {
        if kept.working {
            kept.readers = audience.clone();
            kept.working = false;
        }
    }
    let (current, _) = node.residents.about(id).ok_or_else(crate::authorization::no_subject)?;
    {
        // Whether its home is still there is read from the disk.
        let (node, current) = (node.clone(), current.clone());
        fastctx_platform::task::blocking(move || crate::homes::available(&node, &current)).await?;
    }
    crate::assists::may_turn(node, id, &current).await?;
    let along_only = batch.iter().all(|input| input.delivery.notice.along().is_some());
    let opening = !along_only
        && node.residents.now(id).is_none_or(|now| now.turn.is_none())
        && codex.running_turn(thread).is_none();
    let (notes, context) = if opening {
        let (notes, receipt) = crate::context::turn::prepare(node, id).await?;
        let receipt = (!notes.is_empty()).then_some(receipt);
        (notes, receipt)
    } else {
        (Vec::new(), None)
    };
    let (kept, stamp, held) = (
        batch.clone(),
        crate::context::input::stamp(&notes),
        node.residents.keep_speaking(id).map_err(|error| Failure::new(error.to_string()))?,
    );
    let mut handover = crate::delivery::local(directory, move |directory| {
        let _held = held;
        crate::delivery::begin_with_context(directory, kept, context, stamp)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?;
    let client = Some(format!("{}{}", fastctx_hosts::codex::WORLD_CLIENT, handover.id));
    let (mut input, client) = match batch.as_slice() {
        // A fact of its own work reads best as the output of the tool that
        // started it.
        [only] if matches!(only.delivery.notice, Notice::JobEnded { .. }) => (
            Input::ToolOutput {
                name: "run".to_string(),
                namespace: None,
                output: told::told(&only.delivery.notice, Some(&node.machine())),
            },
            None,
        ),
        batch if batch.iter().any(|kept| matches!(kept.delivery.notice, Notice::Attached { .. })) => (
            Input::Messages(
                batch
                    .iter()
                    .map(|kept| {
                        let images = match &kept.delivery.notice {
                            Notice::Attached { message, .. } => message
                                .attachments
                                .iter()
                                .filter(|attachment| attachment.is_image() && node.is_me(&attachment.owner.machine))
                                .map(|attachment| attachment.path.clone())
                                .collect(),
                            _ => Vec::new(),
                        };
                        let text = told::told(&kept.delivery.notice, Some(&node.machine()));
                        fastctx_hosts::codex::InputMessage { text, images }
                    })
                    .collect(),
            ),
            client,
        ),
        batch => {
            let here = node.machine();
            (
                Input::Text(
                    batch.iter().map(|kept| said(&kept.delivery.notice, &here)).collect::<Vec<_>>().join("\n\n"),
                ),
                client,
            )
        }
    };
    crate::context::turn::prepend(&mut input, &notes);
    for kept in &batch {
        set_state(directory, &kept.delivery.id, PendingState::Sending).await?;
    }
    let sending = if along_only {
        let Input::Text(text) = input else { unreachable!("non-waking notices are text facts") };
        codex.send_to_running(thread, text, client).await
    } else {
        codex.send(thread, input, client).await.map(Some)
    };
    let sent = match sending {
        Ok(Some(sent)) => sent,
        Ok(None) => {
            let (mut kept, held) =
                (handover.clone(), node.residents.keep_speaking(id).map_err(|error| Failure::new(error.to_string()))?);
            crate::delivery::local(directory, move |directory| {
                let _held = held;
                crate::delivery::refused(directory, &mut kept)
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))?;
            return Ok(Some(Landed::Waiting { why: None, answered: true }));
        }
        Err(SendError::NotSent(why)) => {
            let (mut kept, held) =
                (handover.clone(), node.residents.keep_speaking(id).map_err(|error| Failure::new(error.to_string()))?);
            crate::delivery::local(directory, move |directory| {
                let _held = held;
                crate::delivery::refused(directory, &mut kept)
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))?;
            return crate::lifecycle::retry::defer(node, id, &pending.delivery.id, &why).await.map(Some);
        }
        Err(SendError::Unknown(why)) => {
            // Whether they went in is not known, and a turn they went into
            // ended as the app-server stopped: each reader hears that.
            crate::turns::unresolved(node, id, directory, &mut handover, TurnHow::Failed, why.clone(), None)
                .await
                .map_err(|error| Failure::new(error.to_string()))?;
            return Ok(Some(Landed::Uncertain { why }));
        }
    };
    if let Err(error) = bind(node, id, directory, &mut handover, &sent.turn).await {
        let accepted = input_turn(directory, &pending.delivery.id).await?;
        if accepted.as_deref() != Some(&sent.turn) {
            return Err(error);
        }
        node.log.line(format_args!(
            "{} accepted the words in {}; its durable receipt awaits reconciliation: {error}",
            id, sent.turn
        ));
    }
    if let Some(missing) = missing {
        let health = Health { kind: HealthKind::ToolsMissing, text: missing };
        let (node, id, turn) = (node.clone(), id.clone(), sent.turn.clone());
        fastctx_platform::task::blocking(move || crate::turns::health(&node, &id, Some(turn), health))
            .await
            .map_err(|error| Failure::new(error.to_string()))?;
    }

    // What waited and could not go in with these goes in next, as they would
    // have, rather than waiting for something else to wake it.
    if let Some(next) = after {
        hand_over_later(node.clone(), id.clone(), about, directory.clone(), next);
    }
    Ok(Some(if !batch.iter().any(|kept| kept.delivery.id == pending.delivery.id) {
        Landed::Waiting { why: None, answered: true }
    } else if sent.joined {
        Landed::JoinedTurn { turn: sent.turn }
    } else {
        Landed::NewTurn { turn: sent.turn }
    }))
}

/// Hands `pending` over in a task of its own.
fn hand_over_later(node: Arc<Node>, id: ResidentId, about: About, directory: ConversationDir, pending: Pending) {
    fastctx_platform::task::spawn(async move {
        if let Err(failure) = hand_over(&node, &id, &about, &directory, pending).await {
            node.log.line(format_args!("words for {id} wait: {}", failure.message));
        }
    });
}

/// A notice as it goes into a thread: the user's words as they typed them,
/// everything else as the World tells it.
fn said(notice: &Notice, here: &fastctx_types::MachineName) -> String {
    match notice {
        Notice::Words { from: Speaker::User { .. }, text, .. } => text.clone(),
        notice => told::told(notice, Some(here)),
    }
}

/// Commits the consuming turn for the entire batch before changing either
/// its inbox entries or the turn's debt. The caller holds the speech lock.
pub async fn bind(
    node: &Arc<Node>,
    id: &ResidentId,
    directory: &ConversationDir,
    handover: &mut crate::delivery::Handover,
    turn: &str,
) -> Result<(), Failure> {
    let (mut kept, key, working, resident, held) = (
        handover.clone(),
        turn.to_string(),
        node.clone(),
        id.clone(),
        node.residents.keep_speaking(id).map_err(|error| Failure::new(error.to_string()))?,
    );
    *handover = crate::delivery::local(directory, move |directory| {
        let _held = held;
        crate::delivery::accept(directory, &mut kept, &key)?;
        if let Some(receipt) = &kept.context {
            crate::context::turn::accepted(&working, &resident, receipt)?;
        }
        Ok(kept)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?;
    let readers: Vec<Address> = handover.inputs.iter().flat_map(|input| input.readers.iter().cloned()).collect();
    if crate::delivery::terminal_header(directory, turn)
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .is_some()
    {
        crate::turns::add_readers_locked(node, id, turn, readers)
            .await
            .map_err(|error| Failure::new(error.to_string()))?;
    } else {
        let (working, resident, turn, held) = (
            node.clone(),
            id.clone(),
            turn.to_string(),
            node.residents.keep_speaking(id).map_err(|error| Failure::new(error.to_string()))?,
        );
        crate::delivery::local(directory, move |directory| {
            let _held = held;
            let owner = crate::delivery::session(directory, &turn)?;
            let active =
                owner.is_none() || working.residents.about(&resident).is_some_and(|(about, _)| about.session == owner);
            working.residents.owe(&working.world, &resident, &turn, &readers, active)
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?;
    }
    // A stop asked for while these words were on their way stops the turn
    // they started, now that it is known.
    if let Some(source) = node
        .stops
        .started(node, id, handover.inputs.iter().map(|input| &input.delivery.id))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
    {
        crate::turn_evidence::remember(node, id, turn, source)
            .await
            .map_err(|error| Failure::new(error.to_string()))?;
        crate::stops::deliver(node, id, turn, handover.inputs.iter().map(|input| input.delivery.id.clone()).collect());
    }
    for input in &handover.inputs {
        crate::composer::bound(node, id, &input.delivery.id, turn).await?;
        let (working, resident, delivery, turn, held) = (
            node.clone(),
            id.clone(),
            input.delivery.clone(),
            turn.to_string(),
            node.residents.keep_speaking(id).map_err(|error| Failure::new(error.to_string()))?,
        );
        crate::delivery::local(directory, move |directory| {
            let _held = held;
            heard(&working, &resident, &delivery, &turn)?;
            crate::inbox::settle(&working, directory, &delivery.id)?;
            crate::lifecycle::retry::clear_now(&working, &resident, &delivery.id)
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?;
    }
    let (key, held) =
        (handover.id.clone(), node.residents.keep_speaking(id).map_err(|error| Failure::new(error.to_string()))?);
    crate::delivery::local(directory, move |directory| {
        let _held = held;
        crate::delivery::handovers::bound(directory, &key)
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?;
    crate::lifecycle::retry::changed(node, id);
    Ok(())
}

/// What went into `turn`: everyone watching sees who said it, and a
/// colleague's records keep the words said to it. Words are their
/// speaker's; a colleague's answer is that colleague's, as the resident read
/// it; the rest is the World's telling, also as read.
pub fn heard(node: &Arc<Node>, id: &ResidentId, delivery: &Delivery, turn: &str) -> std::io::Result<()> {
    let (from, text) = match &delivery.notice {
        Notice::Words { from, text, .. } => (from.clone(), text.clone()),
        Notice::Attached { message, text } => (message.speaker(), text.clone()),
        Notice::Answer { colleague, .. } | Notice::Question { colleague, .. } => {
            (Speaker::Resident { address: colleague.clone() }, told::told(&delivery.notice, Some(&node.machine())))
        }
        Notice::JobEnded { .. } | Notice::Health { .. } | Notice::Sentinel { .. } => {
            (Speaker::World, told::told(&delivery.notice, Some(&node.machine())))
        }
        Notice::World { text, .. } => (Speaker::World, text.clone()),
    };
    // What an interface shows of the World's telling and a colleague's
    // answer is the notice itself, not the words written for the model.
    let notice = match &delivery.notice {
        Notice::Words { goal_report: Some(_), .. } | Notice::Words { goal_question: Some(_), .. } => {
            Some(Box::new(delivery.notice.clone()))
        }
        Notice::Words { .. } | Notice::Attached { .. } => None,
        Notice::JobEnded { .. }
        | Notice::Answer { .. }
        | Notice::Health { .. }
        | Notice::Question { .. }
        | Notice::World { .. }
        | Notice::Sentinel { .. } => Some(Box::new(delivery.notice.clone())),
    };
    let event = SessionEvent {
        at: Timestamp::now(),
        happened: Happened::Heard {
            message: match &delivery.notice {
                Notice::Attached { message, text } => Some(Box::new(message.message(text))),
                _ => None,
            },
            delivery: Some(delivery.id.clone()),
            turn: Some(turn.to_string()),
            from: Some(from.clone()),
            text: text.clone(),
            notice,
        },
    };
    if !node.residents.knows(id) {
        return Ok(());
    }
    let directory = node.world.conversation(id);
    if crate::delivery::event(&directory, &format!("heard:{}", delivery.id), &event)? {
        node.watchers.publish(id, &event);
    }
    // The user's first words name the conversation where its host does not,
    // and a Codex conversation's login may make its title from them now,
    // beside its first turn (`codex_title`).
    if matches!(from, Speaker::User { .. }) {
        let words = crate::turns::opening(&text, FIRST_WORDS_CHARS);
        if !words.is_empty() {
            node.residents.change_now(&node.world, id, |now| {
                now.first_words.get_or_insert(words);
            })?;
        }
        if let Some((about, _)) = node.residents.about(id) {
            crate::codex_title::maybe(node, id, &about);
        }
    }
    if let Some((about, _)) = node.residents.about(id)
        && matches!(about.origin, Origin::Colleague { .. })
        && matches!(delivery.notice, Notice::Words { .. })
        && !delivery.notice.is_question()
    {
        let line = MessageLine {
            at: delivery.at,
            what: MessageKind::Heard { from: from.clone(), turn: Some(turn.to_string()), text: text.clone() },
        };
        let directory = node.world.conversation(id);
        crate::residents::append_message(&directory, &line)?;
    }
    Ok(())
}

async fn set_state(directory: &ConversationDir, id: &NoticeId, state: PendingState) -> Result<(), Failure> {
    let (directory, id) = (directory.clone(), id.clone());
    fastctx_platform::task::spawn_blocking(move || crate::inbox::set_state(&directory, &id, state))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("keeping what is on its way: {error}")))
}

/// Something kept went in: it is no longer kept, and its id is known.
pub async fn settle(node: &Arc<Node>, directory: &ConversationDir, id: &NoticeId) -> Result<Option<Pending>, Failure> {
    let (directory, id) = (directory.clone(), id.clone());
    let node = node.clone();
    fastctx_platform::task::spawn_blocking(move || crate::inbox::settle(&node, &directory, &id))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("settling what went in: {error}")))
}

/// Finishes what a node that stopped left on its way: what was being put
/// into a Codex thread as it stopped went in or not, nobody knows, and its
/// readers hear so; what waited for a Codex thread goes in now; the ends of
/// turns not yet told are told.
pub async fn restore(node: &Arc<Node>, residents: &[crate::residents::OnRecord]) {
    for resident in residents {
        if resident.removed.is_some() {
            continue;
        }
        let directory = node.world.conversation(&resident.id);
        let result = async {
            let (working, id) = (node.clone(), resident.id.clone());
            fastctx_platform::task::spawn_blocking(move || crate::delivery::recover_outgoing::run(&working, &id))
                .await
                .map_err(std::io::Error::other)??;
            // Deleted since it was listed: nothing of it is left to restore.
            let Some(speaking) = node.residents.speaking(&resident.id) else { return Ok(()) };
            let _speaking = speaking.lock().await;
            for mut handover in crate::delivery::local(&directory, crate::delivery::handovers::active).await? {
                if let Some(turn) = handover.turn.clone() {
                    bind(node, &resident.id, &directory, &mut handover, &turn)
                        .await
                        .map_err(|error| std::io::Error::other(error.message))?;
                }
            }
            crate::delivery::work::notification(node, &resident.id)?();
            Ok::<(), std::io::Error>(())
        }
        .await;
        if let Err(error) = result {
            node.log.line(format_args!("restoring delivery records for {}: {error}", resident.id));
        }
    }
}

/// Classifies interrupted submissions before new submissions are enabled.
/// Delivery itself waits for the node to open, in separate tasks. A
/// conversation whose records could not be read for a passing reason, as
/// when no descriptor was left, is tried again each minute, as of what the
/// node before this one left: the records last written before this one
/// started, never a submission begun since.
pub async fn recover(node: &Arc<Node>, residents: Vec<crate::residents::OnRecord>) {
    let mut failed = Vec::new();
    for resident in residents {
        // What a Codex conversation was handed is settled from its thread,
        // which its host may be slow to read: that waits for no other
        // conversation and never holds the node from opening
        // (`world-concurrency.md`, 收尾从不挡节点起来). Its own words wait
        // meanwhile, its speaking held from now until it is settled.
        if resident.about.host.agent.as_str() == "codex"
            && resident.removed.is_none()
            && crate::delivery::local(&node.world.conversation(&resident.id), crate::delivery::handovers::has_active)
                .await
                .unwrap_or(true)
            && let Some(speaking) = node.residents.speaking(&resident.id)
        {
            let held = speaking.lock_owned().await;
            let node = node.clone();
            fastctx_platform::task::spawn(async move {
                // A few threads are read at once, however many wait.
                let result = match SETTLING.acquire().await {
                    Ok(_permit) => recover_held(&node, &resident, None).await,
                    Err(error) => Err(std::io::Error::other(error.to_string())),
                };
                drop((held, speaking));
                match result {
                    Ok(waiting) => recovered(&node, resident, waiting),
                    Err(error) => {
                        node.log.line(format_args!(
                            "recovering deliveries for {}; tried again in a minute: {error}",
                            resident.id
                        ));
                        let before = std::time::SystemTime::from(node.started);
                        recover_later(node, vec![resident.id], before).await;
                    }
                }
            });
            continue;
        }
        match recover_one(node, &resident, None).await {
            Ok(waiting) => recovered(node, resident, waiting),
            Err(error) => {
                node.log
                    .line(format_args!("recovering deliveries for {}; tried again in a minute: {error}", resident.id));
                failed.push(resident.id);
            }
        }
    }
    if !failed.is_empty() {
        let (node, before) = (node.clone(), std::time::SystemTime::from(node.started));
        fastctx_platform::task::spawn(async move { recover_later(node, failed, before).await });
    }
}

/// How many Codex conversations are settled from their threads at once
/// after a restart.
static SETTLING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);

/// How long a conversation whose deliveries could not be recovered waits
/// before they are tried again.
const RECOVER_AGAIN: std::time::Duration = std::time::Duration::from_secs(60);

/// What one handover takes of what waits, and the first of what waits after
/// it; and whether the input being delivered still waits to go in.
struct Batch {
    batch: Vec<Pending>,
    after: Option<Pending>,
    waits: bool,
}

/// What waits for `id` to go in now with `own`: the arrivals still waiting
/// and not being taken, in the order they came. The first arrival starts or
/// steers the turn; later ones are sent separately, preserving both order
/// and each message's speaker framing; facts that only ride with a turn may
/// precede its first actual input. One handover takes as much as it holds,
/// a longer wait goes over several turns, and only as much of the wait is
/// read as goes in, with the one after.
fn next_batch(node: &Node, id: &ResidentId, directory: &ConversationDir, own: &NoticeId) -> std::io::Result<Batch> {
    use crate::delivery::handovers::{BYTES, INPUTS, accepted_inputs};
    let (mut batch, mut after, mut bytes, mut closed, mut waits) = (Vec::new(), None, 0, false, false);
    let eligible = |kept: &Pending, accepted: &std::collections::HashSet<NoticeId>| {
        kept.state == PendingState::Waiting
            && !accepted.contains(&kept.delivery.id)
            && !node.taking.lock().expect("never poisoned").contains(&(id.clone(), kept.delivery.id.clone()))
    };
    crate::inbox::pages(directory, |page| {
        let accepted = accepted_inputs(directory, &page, true)?;
        for kept in page {
            if !eligible(&kept, &accepted) {
                continue;
            }
            // The handover's own fields and framing keep a tenth of its budget.
            let size = serde_json::to_vec(&kept).map_or(0, |encoded| encoded.len());
            if closed || !batch.is_empty() && (batch.len() == INPUTS || bytes + size > BYTES / 10 * 9) {
                after = Some(kept);
                return Ok(std::ops::ControlFlow::Break(()));
            }
            waits |= kept.delivery.id == *own;
            bytes += size;
            closed = kept.delivery.notice.along().is_none();
            batch.push(kept);
        }
        Ok(std::ops::ControlFlow::Continue(()))
    })?;
    if !waits && let Some(kept) = crate::inbox::unsettled(directory, own)? {
        waits = eligible(&kept, &accepted_inputs(directory, std::slice::from_ref(&kept), true)?);
    }
    Ok(Batch { batch, after, waits })
}

async fn recover_later(node: Arc<Node>, mut failed: Vec<ResidentId>, before: std::time::SystemTime) {
    while !failed.is_empty() {
        tokio::time::sleep(RECOVER_AGAIN).await;
        let mut still = Vec::new();
        for id in failed {
            // One deleted meanwhile has nothing left to recover.
            let Some(resident) = node.residents.everyone().find(|resident| resident.id == id) else { continue };
            match recover_one(&node, &resident, Some(before)).await {
                Ok(waiting) => {
                    node.log.line(format_args!("recovered deliveries for {id}"));
                    recovered(&node, resident, waiting);
                }
                Err(_) => still.push(id),
            }
        }
        failed = still;
    }
}

/// Settles what the node before this one left half done for `resident`,
/// and finds what waits first. With `before`, only a submission recorded
/// before then is taken for interrupted.
async fn recover_one(
    node: &Arc<Node>,
    resident: &crate::residents::OnRecord,
    before: Option<std::time::SystemTime>,
) -> std::io::Result<Option<Pending>> {
    if resident.removed.is_some() {
        return Ok(None);
    }
    let Some(speaking) = node.residents.speaking(&resident.id) else { return Ok(None) };
    let _guard = speaking.lock().await;
    recover_held(node, resident, before).await
}

/// As `recover_one`, with the conversation's speaking held already.
async fn recover_held(
    node: &Arc<Node>,
    resident: &crate::residents::OnRecord,
    before: Option<std::time::SystemTime>,
) -> std::io::Result<Option<Pending>> {
    if resident.removed.is_some() {
        return Ok(None);
    }
    let directory = node.world.conversation(&resident.id);
    let handovers = crate::delivery::local(&directory, crate::delivery::handovers::active).await?;
    for mut handover in handovers {
        let key = handover.id.clone();
        let written_before = crate::delivery::local(&directory, move |directory| {
            crate::delivery::written_before(directory, &key, before)
        })
        .await?;
        // Tried again, what binding left half done before is finished: a
        // submission the node before this one began, its turn found but its
        // settling cut short. Binding again changes nothing done already; a
        // Codex turn found in its thread, which nothing runs since the
        // restart, is ended as its thread says.
        if let Some(turn) = handover.turn.clone()
            && before.is_some()
            && handover.unresolved.is_none()
            && written_before
        {
            if resident.about.host.agent.as_str() == "codex"
                && crate::delivery::terminal_header(&directory, &turn).await?.is_none()
            {
                // Its thread unread this time, it is tried again later.
                if !crate::codex::submitted::found(node, resident, &directory, &mut handover).await? {
                    return Err(std::io::Error::other(format!(
                        "the Codex thread holding turn {turn} could not be read"
                    )));
                }
            } else {
                bind(node, &resident.id, &directory, &mut handover, &turn)
                    .await
                    .map_err(|error| std::io::Error::other(error.message))?;
            }
            continue;
        }
        if handover.turn.is_none() && (handover.attempting || handover.unresolved.is_some()) && written_before {
            // A stop before all of its inputs were marked as being sent came
            // before any of them went: they wait again, as after a refusal.
            let id = handover.id.clone();
            if handover.unresolved.is_none()
                && crate::delivery::local(&directory, move |directory| {
                    crate::delivery::handovers::unsent(directory, &id)
                })
                .await?
            {
                let mut released = handover.clone();
                crate::delivery::local(&directory, move |directory| crate::delivery::refused(directory, &mut released))
                    .await?;
                continue;
            }
            // Codex keeps words with the id they were sent under: found in
            // the thread, they are bound to the turn that took them.
            if handover.unresolved.is_none()
                && crate::codex::submitted::found(node, resident, &directory, &mut handover).await?
            {
                continue;
            }
            let why = format!(
                "the node on {} restarted during a submission; whether {} read it is unknown; \
                 its exact contents remain in its handovers record",
                node.machine(),
                resident.id
            );
            crate::turns::unresolved(
                node,
                &resident.id,
                &directory,
                &mut handover,
                TurnHow::Interrupted,
                why,
                Some(serde_json::json!({"world":"node_restarted"})),
            )
            .await?;
        }
    }
    // Sending entries from versions without batch records are retained and
    // settled as one legacy attempt per resident, at the start alone.
    let legacy = crate::delivery::local(&directory, |directory| {
        let mut legacy = Vec::new();
        for input in crate::inbox::those(directory, |input| input.state == PendingState::Sending)? {
            if !crate::delivery::covers_input(directory, &input.delivery.id)? {
                legacy.push(input);
            }
        }
        Ok(legacy)
    })
    .await?;
    if before.is_none() && !legacy.is_empty() {
        let mut handover = crate::delivery::begin_held(node, &resident.id, &directory, legacy).await?;
        crate::turns::unresolved(
            node,
            &resident.id,
            &directory,
            &mut handover,
            TurnHow::Interrupted,
            "the prior node stopped during a submission; acceptance was not recorded".to_string(),
            Some(serde_json::json!({"world":"node_restarted"})),
        )
        .await?;
    }
    // What can open a turn first, a sentinel's finding before the rest.
    crate::delivery::local(&directory, |directory| {
        first_waiting(directory, |input| {
            (input.delivery.notice.along().is_some(), !matches!(input.delivery.notice, Notice::Sentinel { .. }))
        })
    })
    .await
}

/// The first of what waits whose `rank` is least: what waits is read only
/// until one of the least possible rank, `(false, false)`, is found.
fn first_waiting(
    directory: &ConversationDir,
    rank: impl Fn(&Pending) -> (bool, bool),
) -> std::io::Result<Option<Pending>> {
    let mut best: Option<((bool, bool), Pending)> = None;
    crate::inbox::pages(directory, |page| {
        for input in page.into_iter().filter(|input| input.state == PendingState::Waiting) {
            let ranked = rank(&input);
            if best.as_ref().is_none_or(|(least, _)| ranked < *least) {
                best = Some((ranked, input));
                if ranked == (false, false) {
                    return Ok(std::ops::ControlFlow::Break(()));
                }
            }
        }
        Ok(std::ops::ControlFlow::Continue(()))
    })?;
    Ok(best.map(|(_, input)| input))
}

/// What was recovered goes on: the words waiting first ask to be delivered,
/// and what the conversation owes is paid.
fn recovered(node: &Arc<Node>, resident: crate::residents::OnRecord, waiting: Option<Pending>) {
    let node = node.clone();
    let directory = node.world.conversation(&resident.id);
    fastctx_platform::task::spawn(async move {
        if waiting.is_some() && resident.about.world_managed() {
            crate::lifecycle::retry::kick(&node, &resident.id);
        }
        crate::turns::pay(&node, &resident.id, &directory).await;
    });
}

/// A cancelled handoff reopens delivery of waiting words. Unlike startup
/// recovery, this never treats an in-flight submission as interrupted.
pub fn resume(node: &Arc<Node>, runtime: &tokio::runtime::Handle) {
    for resident in node.residents.everyone() {
        if resident.removed.is_some() {
            continue;
        }
        if !resident.about.world_managed() {
            continue;
        }
        let node = node.clone();
        fastctx_platform::task::spawn_on(runtime, async move {
            let Some(_answering) = node.handoff.call() else { return };
            let directory = node.world.conversation(&resident.id);
            let result = async {
                let Some(speaking) = node.residents.speaking(&resident.id) else { return Ok(()) };
                let guard = speaking.lock().await;
                let waiting = crate::delivery::local(&directory, |directory| {
                    first_waiting(directory, |input| (input.delivery.notice.along().is_some(), false))
                })
                .await?;
                drop(guard);
                if let Some(input) = waiting {
                    hand_over(&node, &resident.id, &resident.about, &directory, input)
                        .await
                        .map_err(|error| std::io::Error::other(error.message))?;
                }
                Ok::<(), std::io::Error>(())
            }
            .await;
            if let Err(error) = result {
                node.log.line(format_args!("resuming delivery for {}: {error}", resident.id));
            }
        });
    }
}

/// What a told answer came to, for the colleague's `messages`.
pub fn outcome(landed: &Result<Landed, Failure>) -> Outcome {
    match landed {
        Ok(Landed::NewTurn { .. } | Landed::JoinedTurn { .. } | Landed::Handed { .. }) => Outcome::Delivered,
        Ok(Landed::Waiting { why, .. }) => Outcome::Waiting { why: why.clone() },
        Ok(Landed::Sent) => Outcome::Sent,
        Ok(Landed::Uncertain { why }) => Outcome::Failed { reason: why.clone() },
        Err(failure) => Outcome::Failed { reason: failure.message.clone() },
    }
}

/// A new notice id: the time and eight random hex digits.
pub fn new_id(prefix: &str) -> NoticeId {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("notice identity needs system randomness");
    let at = Timestamp::now().as_millisecond();
    NoticeId::try_from(format!("{prefix}-{at}-{serial:x}-{:016x}", u64::from_be_bytes(bytes)))
        .expect("a word, digits and hex")
}

/// A notice id that is the same whenever it is made from the same parts:
/// the same thing told again is known for what it is.
pub fn fixed_id(prefix: &str, parts: &[&str]) -> NoticeId {
    use sha2::{Digest as _, Sha256};
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    let digest: String = hash.finalize().iter().take(10).map(|byte| format!("{byte:02x}")).collect();
    NoticeId::try_from(format!("{prefix}-{digest}")).expect("a word and hex")
}
