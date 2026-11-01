//! How a turn of a session the World holds ends, the one way for all three
//! ways it can (`world-colleagues.md`, 一轮怎样结束): its host reports the
//! end; the host's process stops under it; or the node stops while it runs
//! and finds it unfinished when it starts again. Each ends here: the
//! resident's records say how, everyone watching sees it, and everyone owed
//! the turn's end gets the agent's last message, verbatim.
//!
//! The answers are kept in the resident's outbox before the turn's debt is
//! cleared, and each leaves it once its reader's machine has it
//! (`crate::inbox`); an answer's id comes from the turn and the reader, so
//! however often a turn's end is told, each reader hears it once.
//!
//! What the host reports about the session itself, rather than its work,
//! goes into the resident's `now` and to everyone watching; what must be
//! heard at once also goes to everyone owed the running turn's end.

pub(crate) mod answers;
mod settlement;
pub(crate) use settlement::settle_terminal_locked;

use std::sync::Arc;

use fastctx_types::layout::ConversationDir;
use fastctx_types::residents::{Delivery, LAST_SAID_CHARS, Notice, Origin, Owing, Pending, TellStatus, Trouble};
use fastctx_types::session::{Happened, Health, SessionChange, SessionEvent, TurnEnd, TurnHow};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;

use crate::Node;
use crate::speech;

/// A turn whose end is told this long after it is not given the login's
/// windows: those read since are not the ones it ended with.
const LIMITS_AT_END: jiff::SignedDuration = jiff::SignedDuration::from_mins(10);

/// What a report of a resident no longer on record meets: its
/// conversation was deleted while the report was on its way.
fn deleted(id: &ResidentId) -> std::io::Error {
    std::io::Error::other(format!("{id} is no longer on record"))
}

/// Something a host reported about a resident's session.
/// Applies a report before the source acknowledges or deletes it.
pub async fn apply(node: &Arc<Node>, id: &ResidentId, event: SessionEvent) -> std::io::Result<()> {
    static WORKERS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> = std::sync::LazyLock::new(|| {
        Arc::new(tokio::sync::Semaphore::new(std::thread::available_parallelism().map_or(2, usize::from).clamp(2, 8)))
    });
    let permit = WORKERS.clone().acquire_owned().await.map_err(std::io::Error::other)?;
    let (node, id, runtime) = (node.clone(), id.clone(), tokio::runtime::Handle::current());
    fastctx_platform::task::spawn_blocking(move || {
        let _permit = permit;
        runtime.block_on(apply_inner(&node, &id, event))
    })
    .await
    .map_err(std::io::Error::other)?
}

async fn apply_inner(node: &Arc<Node>, id: &ResidentId, mut event: SessionEvent) -> std::io::Result<()> {
    let Some((about, _)) = node.residents.about(id) else { return Err(deleted(id)) };
    if let Happened::Heard { message: Some(message), .. } = &event.happened
        && (about.host.agent.as_str() == "codex" && about.world_managed() && message.detail["type"] == "userMessage"
            || about.host.agent.as_str() == "claude" && message.detail["uuid"].is_string())
    {
        return crate::user_images::live(node, id, event).await;
    }
    if matches!(&event.happened, Happened::Heard { text, message: Some(message), .. } if fastctx_hands::told::is_told(text) && message.images.is_empty())
    {
        return Ok(());
    }
    let directory = node.world.conversation(id);
    let question_key = match &event.happened {
        Happened::Finished { turn, item } => crate::goals::questions::key(turn, item),
        _ => None,
    };
    if let Some(key) = question_key.clone()
        && crate::delivery::local(&directory, move |directory| crate::delivery::has_event(directory, &key)).await?
    {
        return Ok(());
    }
    if about.host.agent.as_str() == "claude" {
        event = crate::claude::turn_facts::observe(&directory, event).await?;
    }
    // Claude's durable event remains pending until its accounting row is committed.
    if let Err(error) = crate::usage::measurement(node, id, &about, &event).await {
        node.log.line(format_args!("recording {id}'s usage: {error}"));
        if about.host.agent.as_str() == "claude" {
            return Err(error);
        }
    }
    // 2026-10-20: hidden title threads feed only the ledger; publishing their
    // internal-only usage would replace the main conversation's usage display.
    if about.host.agent.as_str() == "codex"
        && matches!(&event.happened, Happened::Changed { change: SessionChange::Usage { usage }, .. }
            if usage.as_object().is_some_and(|value| value.len() == 1 && value.contains_key("counted")))
    {
        return Ok(());
    }
    // Account windows are held by the login's ledger. Internal accounting
    // metadata is removed from both main and nested agent events before storage.
    let mut happened = &mut event.happened;
    loop {
        match happened {
            Happened::Changed { change: SessionChange::AgentEvent { happened: inner, .. }, .. } => happened = inner,
            Happened::Changed { change: SessionChange::Usage { usage }, .. } => {
                if let Some(usage) = usage.as_object_mut() {
                    usage.remove("counted");
                    if about.host.agent.as_str() == "claude" {
                        usage.remove("rateLimits");
                    }
                }
                break;
            }
            _ => break,
        }
    }
    if !matches!(event.happened, Happened::TurnStarted { .. }) {
        event = crate::turn_evidence::observe(node, id, event).await?;
    }
    event = crate::delivery::local(&directory, move |directory| {
        crate::run_facts::attach(directory, &mut event)?;
        Ok(event)
    })
    .await?;
    if let Happened::Changed { change: SessionChange::Status { waiting_for_user: Some(waiting), .. }, .. } =
        &event.happened
    {
        node.residents.change_now(&node.world, id, |now| {
            if *waiting && !now.waiting_for_user {
                now.waiting_since = Some(event.at);
            }
            if !waiting {
                now.waiting_since = None;
            }
            now.waiting_for_user = *waiting;
        })?;
        if !waiting {
            crate::composer::changed(node, id);
            crate::asked::settle(node, id, |asked| asked.waits)?;
        }
    }
    // What it asks the user, for whoever calls them about it.
    match &event.happened {
        Happened::Changed { change: SessionChange::Request { id: request_id, request, params }, .. } => {
            if let Some(asked) = crate::asked::of_request(request_id, request, params, event.at) {
                crate::asked::ask(node, id, asked)?;
            }
        }
        Happened::Changed { change: SessionChange::Resolved { id: request_id, .. }, .. } => {
            let request_id = request_id.as_str().map(str::to_string).unwrap_or_else(|| request_id.to_string());
            crate::asked::settle(node, id, |asked| asked.waits && asked.id == request_id)?;
        }
        Happened::Finished { item, .. } => {
            // What it asked without stopping goes where its words go: to the
            // conversation it works for, or to the user, as `message_user`'s.
            if let Some(said) = crate::asked::asked_without_stopping(item)
                && !crate::goals::questions::route(node, id, item, event.at).await?
                && crate::voice::speaks(node, id)
                && let Err(error) = crate::voice::say(node, id, &said, TellStatus::Info, &[])
            {
                node.log.line(format_args!("keeping what {id} asked without stopping: {}", error.message));
            }
            // A Codex thread keeps its first instructions through compacting
            // its conversation; a change they no longer say is told again,
            // its project's record with the next answer of a World tool. A
            // Claude session's plugin gives it its project's record again.
            // A turn that compacts and says nothing after is told as one.
            if matches!(item.what, fastctx_types::session::What::Compaction) {
                node.residents.compacted_at(id, event.at);
                node.residents.change_now(&node.world, id, |now| now.last_compacted = true)?;
            }
            if matches!(item.what, fastctx_types::session::What::Compaction)
                && about.host.agent.as_str() == "codex"
                && about.world_managed()
            {
                crate::context::compacted(node, id, &item.id)?;
            }
        }
        Happened::TurnEnded { .. } => crate::asked::settle(node, id, |asked| asked.waits)?,
        _ => {}
    }
    if let Happened::Began { item, .. } | Happened::Updated { item, .. } = &event.happened
        && matches!(item.what, fastctx_types::session::What::Tool(_))
    {
        let at = item.started_at.unwrap_or(event.at);
        node.residents.change_now(&node.world, id, |now| {
            now.last_tool_at = Some(now.last_tool_at.map_or(at, |prior| prior.max(at)))
        })?;
    }
    // Its name, for those who list the World rather than watch it.
    if let Happened::Titled { title } = &event.happened {
        node.residents.change_now(&node.world, id, |now| now.title = Some(title.clone()))?;
    }
    if let Happened::Changed { change: SessionChange::ModelResponse { usage: Some(usage), .. }, .. } = &event.happened
        && let Some(session) = &about.session
    {
        node.residents.set_session(&node.world, id, session, Some(usage.model.clone()), None)?;
    }
    if let Happened::Changed { turn, change } = &event.happened
        && let Some(report) = presentation_health(change)
    {
        let turn = turn.clone();
        let speaking = node.residents.speaking(id).ok_or_else(|| deleted(id))?;
        let _speaking = speaking.lock().await;
        return health_event(node, id, turn, report, event);
    }
    match event.happened {
        Happened::TurnEnded { turn, end } => {
            // What a notification of this turn says: its last words, or
            // none, never an earlier turn's.
            let said =
                end.last_message.as_deref().map(|said| opening(said, LAST_SAID_CHARS)).filter(|said| !said.is_empty());
            node.residents.change_now(&node.world, id, |now| {
                if said.is_some() {
                    now.last_compacted = false;
                }
                now.last_said = said;
            })?;
            let speaking = node.residents.speaking(id).ok_or_else(|| deleted(id))?;
            let _speaking = speaking.lock().await;
            finish_at(node, id, &turn, end, &[], Some(event.at)).await
        }
        Happened::Health { turn, health: report } => {
            let speaking = node.residents.speaking(id).ok_or_else(|| deleted(id))?;
            let _speaking = speaking.lock().await;
            health_at(node, id, turn, report, event.at)
        }
        _ if !recorded(about.host.agent.as_str(), &event.happened) => {
            node.watchers.publish(id, &event);
            Ok(())
        }
        _ => {
            let key = match &event.happened {
                Happened::TurnStarted { turn } => {
                    if crate::delivery::terminal_header(&directory, turn).await?.is_some() {
                        return Ok(());
                    }
                    node.residents.turn_started(&node.world, id, turn, event.at)?;
                    let key = format!("started:{turn}");
                    node.idle_probes.touched(id);
                    event = crate::turn_evidence::observe(node, id, event).await?;
                    // What waited to ride with the next turn goes into this one.
                    let (node, id) = (node.clone(), id.clone());
                    fastctx_platform::task::spawn(async move { crate::speech::along(&node, &id).await });
                    key
                }
                Happened::Finished { .. } if question_key.is_some() => question_key.expect("known question"),
                _ => serde_json::to_string(&event).map_err(std::io::Error::other)?,
            };
            let (recorded, kept) = crate::delivery::local(&directory, move |directory| {
                let recorded = crate::delivery::event(directory, &key, &event)?;
                Ok((recorded, event))
            })
            .await?;
            if recorded {
                node.watchers.publish(id, &kept);
            }
            Ok(())
        }
    }
}

/// Whether the World keeps an event in a resident's record of its session
/// (`crate::talk::history`). What grows an item is only for those watching
/// live. What the agent did is kept by the World only where the host keeps
/// no record of it the World may read: Codex keeps it in the thread it reads
/// back, Claude Code in its own session file (`crate::claude::history`).
/// What the World itself saw, such as who said words and how a turn ended,
/// is always kept.
/// The start of `text` on one line, at most `chars` characters: what a list
/// or a notification shows of it.
pub fn opening(text: &str, chars: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(chars) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

fn recorded(agent: &str, happened: &Happened) -> bool {
    match happened {
        happened if !happened.kept() => false,
        Happened::Finished { turn, item } => {
            crate::goals::questions::key(turn, item).is_some() || !matches!(agent, "codex" | "claude")
        }
        _ => true,
    }
}

/// A turn of a resident's session ended. An empty `turn` is one whose id
/// was never known.
pub async fn ended(node: &Arc<Node>, id: &ResidentId, turn: &str, end: TurnEnd) {
    let Some(speaking) = node.residents.speaking(id) else { return };
    let _speaking = speaking.lock().await;
    if let Err(error) = finish_locked(node, id, turn, end, &[]).await {
        node.log.line(format_args!("settling {id}'s turn {turn}: {error}"));
    }
}

/// The caller holds the resident's speech lock. Acceptance that arrives
/// after completion adds its readers to the same immutable outcome.
pub async fn finish_locked(
    node: &Arc<Node>,
    id: &ResidentId,
    turn: &str,
    end: TurnEnd,
    additional: &[Address],
) -> std::io::Result<()> {
    finish_at(node, id, turn, end, additional, None).await
}

async fn finish_at(
    node: &Arc<Node>,
    id: &ResidentId,
    turn: &str,
    mut end: TurnEnd,
    additional: &[Address],
    ended_at: Option<Timestamp>,
) -> std::io::Result<()> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    let directory = node.world.conversation(id);
    if crate::delivery::terminal_header(&directory, turn).await?.is_some() {
        return add_readers_locked(node, id, turn, additional.to_vec()).await;
    }
    // Claude's host says what the turn cost; its plugin's observations of
    // each request say the rest.
    if about.host.agent.as_str() == "claude" && end.facts.as_ref().is_none_or(|facts| facts.tokens.is_none()) {
        let observed = crate::claude::turn_facts::finish(&directory, turn).await?;
        end.facts = match (end.facts.take(), observed) {
            (Some(mut facts), Some(observed)) => {
                facts.models = observed.models;
                facts.efforts = observed.efforts;
                facts.tokens = observed.tokens;
                Some(facts)
            }
            (facts, observed) => facts.or(observed.map(Box::new)),
        };
    }
    if about.host.agent.as_str() == "codex" && end.facts.is_none() && !matches!(end.how, TurnHow::Completed) {
        // Recovery is exceptional; normal host completions carry their facts
        // without reading the conversation's accumulated event record.
        let events = crate::delivery::local(&directory, crate::delivery::history).await?;
        let initially_empty = about.world_started
            && !events
                .iter()
                .any(|event| matches!(event.happened, Happened::Changed { change: SessionChange::Forked { .. }, .. }));
        end.facts = fastctx_hosts::codex::recorded_turn_facts(&events, turn, initially_empty).map(Box::new);
    }
    // The login's windows as they stood when the turn ended, for a turn
    // ending now.
    if ended_at.is_none_or(|at| Timestamp::now().duration_since(at) < LIMITS_AT_END)
        && end.facts.as_ref().is_none_or(|facts| facts.limits.is_none())
        && let Some(limits) = crate::usage::turn_limits(node, id, &about)
    {
        end.facts.get_or_insert_with(Default::default).limits = Some(limits);
    }
    crate::turn_evidence::freeze(node, id, turn, &mut end);
    let mut readers = node.residents.readers(id, turn);
    readers.extend_from_slice(additional);
    let key = turn.to_string();
    let accepted =
        crate::delivery::local(&directory, move |directory| crate::delivery::handovers::for_turn(directory, &key))
            .await?;
    for handover in &accepted {
        readers.extend(handover.inputs.iter().flat_map(|input| input.readers.iter().cloned()));
    }
    let current = node.residents.now(id).is_some_and(|now| {
        now.turn.as_deref() == Some(turn)
            || (turn.is_empty() && now.turn.is_none() && now.activity == fastctx_types::residents::Activity::Busy)
    });
    let settles_queue = current || crate::composer::awaits_turn(node, id, turn).await.map_err(std::io::Error::other)?;
    if end.how == TurnHow::Interrupted
        && settles_queue
        && crate::delivery::terminal(&directory, turn)?.is_none()
        && crate::turn_evidence::source(node, id, turn)?.is_none()
        && crate::turn_evidence::classify(&end, None, about.host.agent.as_str()).kind
            == fastctx_types::turn_evidence::StopKind::UserStopped
    {
        crate::goals::holding::stop(node, id, Some(turn.to_string())).await.map_err(std::io::Error::other)?;
    }
    let (working, resident, identity, ending) = (node.clone(), id.clone(), about.clone(), end.clone());
    let accounting_turn = turn.to_string();
    let usage = fastctx_platform::task::spawn_blocking(move || {
        crate::usage::terminal_record(
            &working,
            &resident,
            &identity,
            &accounting_turn,
            &ending,
            ended_at.unwrap_or_else(Timestamp::now),
        )
    })
    .await
    .map_err(std::io::Error::other)?;
    let published = crate::delivery::terminals::publish(
        directory.clone(),
        turn.to_string(),
        Some(crate::delivery::terminals::Initial { end, ended_at, settles_queue, imported: None, usage }),
        readers,
        crate::delivery::work::notification(node, id)?,
    )
    .await?;
    settle_terminal_locked(node, id, &published.header.turn, published.first).await
}

/// Late acceptance repairs this handover and adds only its readers. The first
/// outcome's facts and completed turn-wide effects remain unchanged.
pub(crate) async fn add_readers_locked(
    node: &Arc<Node>,
    id: &ResidentId,
    turn: &str,
    readers: Vec<Address>,
) -> std::io::Result<()> {
    let directory = node.world.conversation(id);
    crate::delivery::terminals::publish(
        directory,
        turn.to_string(),
        None,
        readers,
        crate::delivery::work::notification(node, id)?,
    )
    .await?;
    settle_terminal_locked(node, id, turn, false).await
}

/// Keeps a turn's end in the outbox, an answer for each reader. `key`
/// names what ended among all the resident's ends, as the answers' ids do:
/// its turn, or the words whose turn is not known.
#[allow(clippy::too_many_arguments)]
async fn keep_answers(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    directory: &ConversationDir,
    key: &str,
    turn: &str,
    end: &TurnEnd,
    readers: &[Address],
) -> std::io::Result<()> {
    let colleague = Address { machine: node.machine(), resident: id.clone() };
    let label = match origin {
        Origin::Colleague { label, .. } => label.clone(),
        Origin::User => String::new(),
    };
    let answers: Vec<Owing> = readers
        .iter()
        .map(|reader| Owing {
            to: reader.clone(),
            delivery: Delivery {
                id: speech::fixed_id("answer", &[&colleague.key(), key, &reader.key()]),
                at: Timestamp::now(),
                notice: Notice::Answer {
                    colleague: colleague.clone(),
                    label: label.clone(),
                    turn: turn.to_string(),
                    end: end.clone(),
                },
            },
        })
        .collect();
    let directory = directory.clone();
    fastctx_platform::task::spawn_blocking(move || {
        for answer in &answers {
            crate::inbox::owe(&directory, answer)?;
        }
        Ok(())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Starts or resumes durable delivery of this conversation's answers.
pub fn pay<'a>(
    node: &'a Arc<Node>,
    id: &'a ResidentId,
    directory: &'a ConversationDir,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    crate::outbox::pay(node, id, directory)
}

/// Words kept for a resident whose fate will not be told by any turn's end:
/// they did not go in (`how` failed), or whether they did is not known.
/// Whoever they were for hears that, once, and they are kept no more.
pub async fn unsettled(
    node: &Arc<Node>,
    id: &ResidentId,
    directory: &ConversationDir,
    pending: Pending,
    how: TurnHow,
    why: String,
) {
    let result = async {
        let speaking = node.residents.speaking(id).ok_or_else(|| deleted(id))?;
        let _speaking = speaking.lock().await;
        let held = node.residents.keep_speaking(id)?;
        let mut handover = crate::delivery::local(directory, move |directory| {
            let _held = held;
            crate::delivery::begin(directory, vec![pending])
        })
        .await?;
        unresolved(node, id, directory, &mut handover, how, why, None).await
    }
    .await;
    if let Err(error) = result {
        node.log.line(format_args!("settling uncertain words for {id}: {error}"));
    }
}

/// An unknown submission is one outcome for the whole attempted batch,
/// not one outcome per message or per retry.
pub async fn unresolved(
    node: &Arc<Node>,
    id: &ResidentId,
    directory: &ConversationDir,
    handover: &mut crate::delivery::Handover,
    how: TurnHow,
    why: String,
    error: Option<serde_json::Value>,
) -> std::io::Result<()> {
    let inputs = handover.inputs.clone();
    let accepted = crate::delivery::local(directory, move |directory| {
        crate::delivery::handovers::accepted_inputs(directory, &inputs, false)
    })
    .await?;
    if handover.inputs.iter().all(|input| accepted.contains(&input.delivery.id)) {
        return Ok(());
    }
    let end = TurnEnd { facts: None, error, how, reason: Some(why), duration_ms: None, last_message: None };
    let (mut kept, held) = (handover.clone(), node.residents.keep_speaking(id)?);
    *handover = crate::delivery::local(directory, move |directory| {
        let _held = held;
        crate::delivery::unresolved(directory, &mut kept, end)?;
        Ok(kept)
    })
    .await?;
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    let readers: std::collections::BTreeSet<Address> = handover
        .inputs
        .iter()
        .filter(|input| !accepted.contains(&input.delivery.id))
        .flat_map(|input| input.readers.iter().cloned())
        .collect();
    let readers: Vec<Address> = readers.into_iter().collect();
    keep_answers(
        node,
        id,
        &about.origin,
        directory,
        handover.id.as_str(),
        "",
        handover.unresolved.as_ref().expect("recorded above"),
        &readers,
    )
    .await?;
    for pending in &handover.inputs {
        if accepted.contains(&pending.delivery.id) {
            continue;
        }
        let event = SessionEvent {
            at: Timestamp::now(),
            happened: Happened::DeliveryUncertain {
                delivery: pending.delivery.id.clone(),
                why: handover.unresolved.as_ref().and_then(|end| end.reason.clone()).unwrap_or_default(),
            },
        };
        if crate::delivery::event(directory, &format!("uncertain:{}", pending.delivery.id), &event)? {
            node.watchers.publish(id, &event);
        }
        let why = handover.unresolved.as_ref().and_then(|end| end.reason.as_deref()).unwrap_or_default();
        crate::composer::uncertain(node, id, &pending.delivery.id, why).await.map_err(std::io::Error::other)?;
        crate::inbox::retain_uncertain(directory, pending)?;
    }
    // The handover retains the exact payload and the uncertainty even when
    // no resident is owed an answer, as with a delivery to a user's session.
    let trouble = Trouble {
        at: Timestamp::now(),
        text: handover.unresolved.as_ref().and_then(|end| end.reason.clone()).unwrap_or_default(),
    };
    node.residents.change_now(&node.world, id, |now| {
        now.trouble = Some(trouble);
        if now.turn.is_none() {
            now.activity = fastctx_types::residents::Activity::Idle;
            now.since = Timestamp::now();
        }
    })?;
    let key = handover.id.clone();
    crate::delivery::local(directory, move |directory| crate::delivery::handovers::complete(directory, &key)).await?;
    let (node, id, directory) = (node.clone(), id.clone(), directory.clone());
    fastctx_platform::task::spawn(async move { pay(&node, &id, &directory).await });
    Ok(())
}

/// The host reported something about a resident's session itself.
pub fn health(node: &Arc<Node>, id: &ResidentId, turn: Option<String>, report: Health) -> std::io::Result<()> {
    health_at(node, id, turn, report, Timestamp::now())
}

fn health_at(
    node: &Arc<Node>,
    id: &ResidentId,
    turn: Option<String>,
    report: Health,
    at: Timestamp,
) -> std::io::Result<()> {
    let event = SessionEvent { at, happened: Happened::Health { turn: turn.clone(), health: report.clone() } };
    health_event(node, id, turn, report, event)
}

/// Provider state retains the World's urgent delivery semantics without
/// adding a second transcript notice beside the original presentation event.
/// The World's own tools not starting is told with the words that find them
/// missing instead.
fn presentation_health(change: &SessionChange) -> Option<Health> {
    if matches!(change, SessionChange::ToolServer { server, .. } if server == fastctx_hosts::codex::SERVER_NAME) {
        return None;
    }
    change.health()
}

fn health_event(
    node: &Arc<Node>,
    id: &ResidentId,
    turn: Option<String>,
    report: Health,
    event: SessionEvent,
) -> std::io::Result<()> {
    let at = event.at;
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    let directory = node.world.conversation(id);
    let key = serde_json::to_string(&event).map_err(std::io::Error::other)?;
    let first = crate::delivery::event(&directory, &key, &event)?;
    let finished = turn.as_deref().map(|turn| crate::delivery::terminal(&directory, turn)).transpose()?.flatten();
    if finished.is_none() {
        let trouble = Trouble { at, text: report.text.clone() };
        node.residents.change_now(&node.world, id, |now| now.trouble = Some(trouble))?;
    }
    if first {
        node.watchers.publish(id, &event);
    }
    if !report.kind.urgent() {
        return Ok(());
    }
    let turn = turn.or_else(|| node.residents.now(id).and_then(|now| now.turn)).unwrap_or_default();
    let readers = node.residents.readers(id, &turn);
    let colleague = Address { machine: node.machine(), resident: id.clone() };
    let label = match about.origin {
        Origin::Colleague { label, .. } => label,
        Origin::User => String::new(),
    };
    for reader in readers {
        let id = speech::fixed_id("health", &[&colleague.key(), &turn, &format!("{:?}", report.kind), &reader.key()]);
        let notice = Notice::Health { colleague: colleague.clone(), label: label.clone(), health: report.clone() };
        crate::inbox::owe(&directory, &Owing { to: reader, delivery: Delivery { id, at, notice } })?;
    }
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn(async move { pay(&node, &id, &directory).await });
    Ok(())
}

/// Ends the turns the node stopped under, now that it is up again, and
/// tells whoever was owed them. A Claude session runs on under Claude Code's
/// own supervisor while the node is away: its turn goes on and ends as it
/// ends, its reports followed again; only one whose session is gone ended
/// with the node (`specs/world-concurrency.md`, 宿主的会话比节点活得久).
pub async fn recover(node: &Arc<Node>, unfinished: Vec<(ResidentId, String)>) {
    for (id, turn) in unfinished {
        if let Err(error) = recover_one(node, &id, &turn).await {
            node.log.line(format_args!("recovering {id}'s turn {turn}; will retry: {error}"));
            let node = node.clone();
            fastctx_platform::task::spawn(async move {
                let (mut again, mut told) = (crate::again::Again::default(), error.to_string());
                loop {
                    tokio::time::sleep(again.next().max(std::time::Duration::from_secs(1))).await;
                    let Err(error) = recover_one(&node, &id, &turn).await else { break };
                    if error.to_string() != told {
                        told = error.to_string();
                        node.log.line(format_args!("recovering {id}'s turn {turn}; will retry: {told}"));
                    }
                }
            });
        }
    }
}

/// Only the turn observed at startup is retried; later turns are never
/// classified as interrupted by this worker.
async fn recover_one(node: &Arc<Node>, id: &ResidentId, turn: &str) -> std::io::Result<()> {
    let Some(speaking) = node.residents.speaking(id) else { return Ok(()) };
    let _speaking = speaking.lock().await;
    if let Some((about, _)) = node.residents.about(id)
        && about.host.agent.as_str() == "claude"
        && node.residents.now(id).and_then(|now| now.turn).as_deref() == Some(turn)
        && let Some(session) = &about.session
        && claude_runs(node, session)?
    {
        return Ok(());
    }
    let end = TurnEnd {
        facts: None,
        error: Some(serde_json::json!({"world":"node_restarted"})),
        how: TurnHow::Interrupted,
        reason: Some(format!("the node on {} restarted while it ran", node.machine())),
        duration_ms: None,
        last_message: None,
    };
    finish_locked(node, id, turn, end, &[]).await
}

/// An unreadable registration is not evidence of death. A process this
/// user may not look at is another's that took its number: Claude Code runs
/// as the user.
fn claude_runs(node: &Node, session: &str) -> std::io::Result<bool> {
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
    match fastctx_hosts::claude::exchange::registration(&directory)?.and_then(|registration| registration.process) {
        Some(process) => match fastctx_platform::is_alive(&process) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(false),
            alive => alive,
        },
        None => Ok(false),
    }
}
