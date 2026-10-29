//! A member's control connection: letters, calls, questions and the
//! heartbeat.
//!
//! The hub says who a member's envelopes are from, by the session they come
//! on, and passes a call or a letter on only when the directory lets that
//! member say it to its receiver for the account it names. Every call in
//! flight keeps what it was allowed under, so a change to the directory
//! that takes it back ends the call (`Hub::changed`).

use std::net::SocketAddr;
use std::sync::Arc;

use fastctx_net::envelope::{Envelope, Kind};
use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{self, CallStatus, Change, Down, HEARTBEAT, Letters, Question, Refusal, SILENCE, Told, Up};
use fastctx_types::access::Purpose;
use fastctx_types::{AccountName, AgentName, MachineName};
use futures_util::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::outq::{Next, Outq};
use crate::{Hub, Out, Session, Socket, refuse, storage_failure};

/// What a member says of itself when its session opens.
pub(crate) struct Said {
    pub call_details: bool,
    /// The machine that forwards its connection here, if any.
    pub through: Option<String>,
    /// Whether it can take direct connections.
    pub direct: bool,
    /// The addresses of its network cards.
    pub interfaces: Vec<std::net::IpAddr>,
    /// Whether it reads machine and hub names of any language.
    pub any_names: bool,
    /// The address it dialed: a hub that moved tells by it whether the
    /// member still comes through the old place (`crate::moving`).
    pub dialed: Option<SocketAddr>,
    /// Whether it takes a new name the hub gives it.
    pub renames: bool,
    /// Whether it reads what is afoot on other machines.
    pub afoot: bool,
    /// Whether it deletes what it keeps of a conversation when told.
    pub forgets: bool,
    /// Whether it reads visitors among the accounts.
    pub roles: bool,
    /// Whether it reads the network's versions.
    pub versions: bool,
    /// Whether it reads the records of updates due and made as asked.
    pub due: bool,
    /// Whether it reads the records of projects deleted.
    pub project_deletes: bool,
    pub project_entry: bool,
    pub conversation_deletions: bool,
    /// Whether it reads the records of profiles written.
    pub profiles: bool,
    pub account_records: bool,
    pub conversation_plans: bool,
    pub scratch_locations: bool,
    /// What a machine taken off could not do of the uninstall it was asked.
    pub left: Option<String>,
    /// Whether it reads that a call it answers is over for its caller.
    pub cancels: bool,
    /// Whether it reads who is online as it changes.
    pub presence: bool,
    /// Whether it reads an answer too large for one frame in parts.
    pub parts: bool,
    /// Whether it reads what changed in its view (`Down::View`).
    pub view_deltas: bool,
}

/// What a call or a data connection was allowed under: `from` saying it to
/// `to` for `account`, for `purpose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Standing {
    pub from: MachineName,
    pub account: AccountName,
    pub to: MachineName,
    pub purpose: Purpose,
    pub project_entry: Option<(fastctx_types::ResidentId, fastctx_types::TownName)>,
    pub plan: Option<fastctx_types::ResidentId>,
}

impl Standing {
    /// Why it no longer holds, if it does not.
    pub fn lapsed(&self, hub: &Hub) -> Option<String> {
        self.lapsed_in(hub, &hub.store())
    }

    /// The same, against the directory in `store`, held by the caller.
    fn lapsed_in(&self, hub: &Hub, store: &crate::store::Store) -> Option<String> {
        store
            .book
            .directory
            .admits(&self.from, &self.account, &self.to, self.purpose)
            .and_then(|()| match &self.project_entry {
                Some((resident, town)) => {
                    crate::project_entry::check(store, &self.from, &self.account, &self.to, resident, town)
                }
                None => Ok(()),
            })
            .and_then(|()| match &self.plan {
                Some(resident) => crate::plan_calls::check(hub, store, &self.from, &self.account, &self.to, resident),
                None => Ok(()),
            })
            .err()
    }
}

mod in_flight;

pub(crate) use in_flight::InFlight;

/// Why `pending` may not go on its way now: its member, or its account,
/// has as many calls on their way as it may (`in_flight::shares`). A member
/// gone wrong fills only its own share, and the others are forwarded as
/// before (`world-concurrency.md`, 别的机器来的调用按来者公平地分).
fn crowded(calls: &InFlight, pending: &Pending) -> Option<String> {
    let (member_share, account_share) = in_flight::shares();
    let member = calls.of_member(&pending.caller);
    if member >= member_share {
        return Some(format!(
            "{} has {member} calls on their way through the hub already; one has to end first",
            pending.caller
        ));
    }
    let account = calls.of_account(&pending.standing.account);
    (account >= account_share).then(|| {
        format!(
            "the account {} has {account} calls on their way through the hub already; one has to end first",
            pending.standing.account
        )
    })
}

/// A call on its way: who asked, who is to answer, and what it was allowed
/// under.
pub(crate) struct Pending {
    caller: MachineName,
    caller_generation: u64,
    id: u64,
    target: MachineName,
    target_generation: u64,
    pub standing: Standing,
    /// The data connection the call came with, which waits no longer once
    /// the call is over.
    stream: Option<u64>,
    /// When the hub passed it on.
    pub(crate) at: tokio::time::Instant,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn session(
    hub: Arc<Hub>,
    mut socket: Socket,
    peer: SocketAddr,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
    letters: Letters,
    said: Said,
) {
    let through = said.through.clone();
    let dialed = said.dialed;
    let name = match hub.member(&key, &proof, &binding) {
        Ok(name) => name,
        Err(reason) => {
            hub.log.line(format_args!("{peer}: refused a session: {reason}"));
            if let (Refusal::Removed { .. }, Some(left)) = (&reason, &said.left) {
                crate::manage::left(&hub, &key, left);
            }
            let tells = crate::dissolve::tells(&reason);
            if refuse(&mut socket, reason).await && tells {
                crate::dissolve::told(&hub, &key);
            }
            return;
        }
    };
    let generation = hub.next();
    let out = Outq::new(said.parts);
    if let Err(reason) = open(&hub, &name, key, generation, out.clone(), letters, peer, said) {
        hub.log.line(format_args!("{peer}: the session of {name} could not open: {reason}"));
        refuse(&mut socket, reason).await;
        return;
    }
    // A hub that began handing over after this session was let in, but
    // before it was registered, did not end it.
    if hub.moving.frozen() {
        let _ = out.send(Out::End(Refusal::ShuttingDown));
    }
    crate::moving::follow::came(&hub, &name, dialed);
    crate::forget::opened(&hub, &name);
    match &through {
        Some(through) => hub.log.line(format_args!("{name} connected through {through}, from {peer}")),
        None => hub.log.line(format_args!("{name} connected from {peer}")),
    }
    // Whoever sees it sees it online now.
    hub.health.opened();
    hub.present(&name);
    out.afoot_started();

    let pulse = crate::pulse_of(&socket);
    let (mut sink, mut stream) = socket.split();
    let (stopped, mut writer_stopped) = tokio::sync::oneshot::channel::<()>();
    let (telling, member, member_name, queue) = (hub.clone(), key, name.clone(), out.clone());
    let writing = pulse.clone();
    let writer = tokio::spawn(async move {
        // Dropped when the writer returns, which the reader notices.
        let _stopped = stopped;
        loop {
            // Something goes at least every heartbeat, saying how much the
            // hub took of what the member sent: a member sending a long frame
            // on a slow line has its own heartbeats wait behind that frame,
            // and hears the hub only by what the hub sends of itself.
            let next = tokio::select! {
                next = queue.next() => next,
                () = tokio::time::sleep(HEARTBEAT) => {
                    if !matches!(writing.writing(sink.send(fastctx_net::pulse::ping(&writing))).await, Ok(Ok(()))) {
                        return;
                    }
                    continue;
                }
            };
            let Some(next) = next else { break };
            let (frames, last) = match next {
                Next::Frames(frames) => (frames, false),
                Next::End(reason) => (vec![Down::Refused { reason }], true),
                // A member that does not read what it is sent is let go: it
                // comes back and reads on from where it was
                // (`world-concurrency.md`, 机器之间).
                Next::Overflowed(why) => {
                    telling.log.line(format_args!("{member_name}: {why}"));
                    return;
                }
            };
            for frame in frames {
                if let Down::AccountRecordChanged { changed } = &frame {
                    let store = telling.store();
                    let directory = &store.book.directory;
                    if directory.member(&member_name).is_none_or(|found| found.key != member)
                        || directory
                            .authorize(&changed.account, fastctx_net::authorization::Action::ActFor(&member_name))
                            .is_err()
                    {
                        continue;
                    }
                }
                let bytes = wire::encode(&frame);
                if bytes.len() > wire::MAX_CONTROL_FRAME {
                    telling.log.line(format_args!(
                        "{member_name}: outgoing control frame is {} bytes, more than {} the member reads; closing the session",
                        bytes.len(),
                        wire::MAX_CONTROL_FRAME
                    ));
                    return;
                }
                // Written for as long as it moves (`fastctx_net::pulse`); given
                // up at once when so much waits for the member untaken that it
                // is let go, however long this write would still take.
                let written = tokio::select! {
                    written = writing.writing(sink.send(WsMessage::Binary(bytes.into()))) => written,
                    why = queue.overflowed() => {
                        telling.log.line(format_args!("{member_name}: {why}"));
                        return;
                    }
                };
                match written {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return,
                    Err(_) => {
                        telling.log.line(format_args!(
                            "{member_name}: took nothing in {} s; letting it go",
                            SILENCE.as_secs()
                        ));
                        return;
                    }
                }
                // A machine on the network, told it was dissolved.
                if let Down::Refused { reason } = &frame
                    && crate::dissolve::tells(reason)
                {
                    crate::dissolve::told(&telling, &member);
                }
            }
            if last {
                let _ = tokio::time::timeout(SILENCE, sink.close()).await;
                return;
            }
        }
    });
    // The letters it sends are kept in order by a task of their own, so a
    // slow disk holds up no heartbeat or answer on this connection.
    let (letters_in, letters_coming) = mpsc::channel(LETTERS_IN);
    let intake = tokio::spawn(intake(
        hub.clone(),
        name.clone(),
        generation,
        out.clone(),
        letters_coming,
        crate::moving::Moving::hold(&hub),
    ));

    // Whether the writer still has a refusal to deliver before closing.
    let mut heard = tokio::time::Instant::now();
    let mut uploads = crate::question_parts::Uploads::default();
    let mut upload_clock = tokio::time::interval(HEARTBEAT);
    let (ended, refusing) = loop {
        let next = tokio::select! {
            next = stream.next() => next,
            _ = upload_clock.tick() => {
                for id in uploads.expired() {
                    let _ = out.send(Out::Frame(Down::Told { id, answer: Err("the ledger upload exceeded 60 seconds".into()) }));
                }
                continue;
            }
            // A frame still coming in shows it is there, as one come whole
            // does.
            () = tokio::time::sleep_until(heard + SILENCE) => {
                if let Some(at) = pulse.since(heard) {
                    heard = at;
                    continue;
                }
                break (Some(format!("nothing was heard from it for {} s", SILENCE.as_secs())), false);
            }
            // The writer stopped: the connection was replaced or ended.
            _ = &mut writer_stopped => break (None, false),
        };
        heard = tokio::time::Instant::now();
        let frame = match next {
            None | Some(Ok(WsMessage::Close(_))) => break (None, false),
            Some(Err(error)) => break (Some(fastctx_net::dial::describe(&error)), false),
            Some(Ok(WsMessage::Binary(bytes))) => match serde_json::from_slice::<Up>(&bytes) {
                Ok(frame) => frame,
                // A newer machine's frame this hub does not know is passed
                // over, a question it asks answered that it is not known.
                Err(error) => {
                    unreadable(&hub, &name, &out, &bytes, &error);
                    continue;
                }
            },
            Some(Ok(_)) => continue,
        };
        // A letter waits its turn to be kept; more than `LETTERS_IN` waiting
        // and this connection is read no further until there is room, for
        // as long as a silence lasts at most.
        if let Up::Send { seq, message } = frame {
            let queued = tokio::select! {
                queued = letters_in.send((seq, message)) => queued.is_ok(),
                _ = &mut writer_stopped => break (None, false),
                () = tokio::time::sleep(SILENCE) => {
                    let why = format!("its letters came faster than the hub keeps them, for {} s", SILENCE.as_secs());
                    break (Some(why), false);
                }
            };
            if !queued {
                break (Some("keeping its letters stopped".to_string()), false);
            }
            continue;
        }
        // What it says it took of what the hub sent keeps the hub's writes
        // to it going.
        if let Up::Ping { took: Some(took), .. } = &frame {
            pulse.told(*took);
        }
        let (frame, reservation) = match frame {
            Up::QuestionPart { id, index, total, text } => match uploads.push(id, index, total, &text) {
                Ok(Some(complete)) => (Up::Ask { id, question: complete.question }, Some(complete.reservation)),
                Ok(None) => continue,
                Err(why) => {
                    let _ = out.send(Out::Frame(Down::Told { id, answer: Err(why) }));
                    continue;
                }
            },
            Up::CancelQuestion { id } => {
                uploads.cancel(id);
                continue;
            }
            frame => (frame, None),
        };
        if let Err(reason) = handle(&hub, &name, key, generation, &out, pulse.taken(), frame, reservation) {
            let _ = out.send(Out::End(reason.clone()));
            break (Some(reason.to_string()), true);
        }
    };
    // The member counts as gone from this moment, and the calls waiting on
    // it are told so.
    close(&hub, &name, generation);
    intake.abort();
    if refusing {
        // The writer returns once it has sent the refusal.
        let _ = writer.await;
    } else {
        writer.abort();
    }
    out.shut();
    match ended {
        Some(reason) => hub.log.line(format_args!("{name} disconnected: {reason}")),
        None => hub.log.line(format_args!("{name} disconnected")),
    }
    hub.present(&name);
}

/// How many letters from a member may wait to be kept before its
/// connection is read no further.
const LETTERS_IN: usize = 1024;

/// How many letters are kept before their sender is told the hub holds
/// them, their count written once for all of them.
const KEPT_AT_ONCE: usize = 64;

/// Keeps the letters the session `generation` of `name` sends, in order:
/// each written to disk, then its sender's count, then the sender told the
/// hub holds them. A failure of the hub's own disk ends the session, and
/// the member sends them again; a session replaced since keeps no more.
async fn intake(
    hub: Arc<Hub>,
    name: MachineName,
    generation: u64,
    out: Outq,
    mut coming: mpsc::Receiver<(u64, Envelope)>,
    _held: crate::moving::Active,
) {
    // The last letter this session sent; it sends them in order, from the
    // first the welcome did not count.
    let mut last: Option<u64> = None;
    while let Some(first) = coming.recv().await {
        let mut batch = vec![first];
        while batch.len() < KEPT_AT_ONCE {
            match coming.try_recv() {
                Ok(next) => batch.push(next),
                Err(_) => break,
            }
        }
        let mut receipts = Vec::new();
        // Whether the count moved: letters only sent again leave it as it is.
        let mut counted = false;
        let mut full = false;
        for (seq, message) in batch {
            // A letter the member passes over, sending what comes after it,
            // it holds no more: why the hub refused it was heard and kept.
            refusals_heard(&hub, &name, generation, |refused| refused < seq && last.is_none_or(|last| refused > last));
            last = Some(seq);
            // Old senders cannot report held in a heartbeat. Reconnecting proves
            // which refusal receipts they kept without discarding any unheard result.
            const UNHEARD: usize = 1024;
            if hub.store().letters.get(&name).is_some_and(|entry| seq > entry.taken && entry.refused.len() >= UNHEARD) {
                full = true;
                break;
            }
            let began = tokio::time::Instant::now();
            match keep(&hub, &name, generation, seq, message).await {
                Ok(Kept::Held) => {
                    hub.health.kept(began.elapsed());
                    receipts.push((seq, None));
                    counted = true;
                }
                Ok(Kept::Refused(why)) => {
                    receipts.push((seq, Some(why)));
                    counted = true;
                }
                Ok(Kept::Again(refused)) => receipts.push((seq, refused)),
                Err(Stopped::Replaced) => return,
                Err(Stopped::Failed(reason)) => {
                    let _ = out.send(Out::End(reason));
                    return;
                }
            }
        }
        let count = {
            let mut store = hub.store();
            if store.intake.get(&name) != Some(&generation) {
                return;
            }
            let dir = store.dir().to_path_buf();
            if counted { store.count_of(&name).map(|count| (dir, count)) } else { None }
        };
        if let Some((dir, (taken, revision, fence, numbering))) = count {
            let held = crate::moving::Moving::hold(&hub);
            let written = tokio::task::spawn_blocking(move || {
                let _held = held;
                crate::store::Store::write_count(&dir, numbering, &taken, &fence, revision)
            })
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))
            .and_then(|written| written);
            if let Err(error) = written {
                let _ = out.send(Out::End(storage_failure(&hub, "counting a letter", &error)));
                return;
            }
        }
        // A receipt counts for every letter before it: a refused letter is
        // told on its own, with why, and the run once, by its last.
        let last = receipts.last().map(|(seq, refused)| (*seq, refused.is_some()));
        for (seq, refused) in receipts {
            if refused.is_some() {
                let _ = out.send(Out::Frame(Down::Ack { seq, refused }));
            }
        }
        if let Some((seq, false)) = last {
            let _ = out.send(Out::Frame(Down::Ack { seq, refused: None }));
        }
        if full {
            let reason =
                Refusal::Broken { reason: "reconnecting to confirm the refusal receipts this member kept".into() };
            let _ = out.send(Out::End(reason));
            // Keep the intake open until the writer has sent the preceding receipts.
            out.closed().await;
            return;
        }
    }
}

/// Forgets why the hub refused those of `name`'s letters that `heard` says
/// its session `generation` heard of and recorded: they are not told again.
fn refusals_heard(hub: &Hub, name: &MachineName, generation: u64, heard: impl Fn(u64) -> bool) {
    let mut store = hub.store();
    if store.intake.get(name) != Some(&generation) {
        return;
    }
    if let Some(entry) = store.letters.get_mut(name) {
        entry.refused.retain(|refused, _| !heard(*refused));
    }
}

/// How far a member's session starts counted: its count, short of the first
/// of its letters the hub refused that it may not have heard why of, which
/// it then sends again and is told why again before it lets it go.
fn counted_through(entry: &crate::store::Letters) -> u64 {
    entry.refused.keys().next().map_or(entry.taken, |first| entry.taken.min(first.saturating_sub(1)))
}

/// A frame from `name` this hub cannot read: told once in the log for each
/// kind; a question is answered that it is not known here.
fn unreadable(hub: &Hub, name: &MachineName, out: &Outq, bytes: &[u8], error: &serde_json::Error) {
    let frame: serde_json::Value = serde_json::from_slice(bytes).unwrap_or_default();
    let kind = frame["f"].as_str().unwrap_or("?");
    hub.log.line(format_args!("{name}: a frame this hub cannot read ({kind}): {error}"));
    if kind == "ask"
        && let Some(id) = frame["id"].as_u64()
    {
        let answer = Err(format!(
            "this hub, fastctx {}, does not know this question; it may be older than the machine asking",
            env!("CARGO_PKG_VERSION")
        ));
        let _ = out.send(Out::Frame(Down::Told { id, answer }));
    }
}

/// Registers the session, replacing an older one of the same member, and
/// queues the welcome; the letters waiting for the member follow it, a
/// window at a time (`crate::mailbox`).
#[allow(clippy::too_many_arguments)]
fn open(
    hub: &Arc<Hub>,
    name: &MachineName,
    key: PublicKey,
    generation: u64,
    out: Outq,
    letters: Letters,
    peer: SocketAddr,
    said: Said,
) -> Result<(), Refusal> {
    let mut store = hub.store();
    let (world, hub_name) = (store.hub.id.clone(), store.hub.name.clone());
    let too_old = !said.any_names && crate::login::beyond_older_releases(&store);
    let entry = store.letters_of(name).map_err(|error| storage_failure(hub, "making letter records", &error))?;
    entry.any_names = said.any_names;
    entry.renames = said.renames;
    entry.forgets = said.forgets;
    entry.roles = said.roles;
    entry.versions = said.versions;
    entry.due = said.due;
    entry.project_deletes = said.project_deletes;
    entry.conversation_deletions = said.conversation_deletions;
    entry.profiles = said.profiles;
    if too_old {
        // A member back on a release from before names of any language,
        // which would fail on the first name it cannot read: it is told why
        // instead, and is counted as one such release when a name is asked.
        store.save_letters().map_err(|error| storage_failure(hub, "recording the connection", &error))?;
        hub.log.line(format_args!("{name}: refused a session: {}", crate::login::OLDER_RELEASE));
        return Err(Refusal::Broken { reason: crate::login::OLDER_RELEASE.to_string() });
    }
    let renumbered = entry.epoch != letters.epoch;
    if renumbered {
        // The machine numbers its letters afresh, having lost its records.
        entry.epoch = letters.epoch;
        entry.taken = 0;
        entry.refused.clear();
    }
    let taken_by_member = match letters.taken {
        Some((epoch, taken)) if epoch == entry.out_epoch => taken,
        _ => 0,
    };
    let now = Timestamp::now();
    entry.connected_at = Some(now);
    let numbering = entry.out_epoch;
    let welcome = Down::Welcome {
        hub: world,
        hub_name,
        name: name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        taken: counted_through(entry),
        letters: entry.out_epoch,
        afoot_pending: Some(said.afoot),
    };
    // Its new numbering is kept before any letter of it is taken: kept
    // later, a stop in between would take the new letters for ones held.
    if renumbered && let Some((taken, revision, fence, numbering)) = store.count_of(name) {
        crate::store::Store::write_count(store.dir(), numbering, &taken, &fence, revision)
            .map_err(|error| storage_failure(hub, "recording the machine's numbering", &error))?;
    }
    store.save_letters().map_err(|error| storage_failure(hub, "recording the connection", &error))?;
    // From now on only this session's letters move the member's count.
    store.intake.insert(name.clone(), generation);
    // The welcome is the first frame; the letters waiting follow from where
    // the member took them.
    out.welcome(welcome);
    crate::mailbox::opened(hub, &mut store, name, numbering, generation, out.clone(), taken_by_member);
    let session = Session {
        key,
        account_records: said.account_records,
        conversation_plans: said.conversation_plans,
        scratch_locations: said.scratch_locations,
        call_details: said.call_details,
        generation,
        out,
        since: now,
        peer,
        through: said.through,
        direct: said.direct,
        interfaces: said.interfaces,
        paths: None,
        afoot: said.afoot,
        forgets: said.forgets,
        versions: said.versions,
        cancels: said.cancels,
        presence: said.presence,
        project_entry: said.project_entry,
        view_deltas: said.view_deltas,
    };
    let replaced = hub.sessions().insert(name.clone(), session);
    drop(store);
    if said.versions {
        crate::updates::opened(hub, name);
    }
    if let Some(old) = replaced {
        let _ = old.out.send(Out::End(Refusal::Replaced));
        fail_calls_of(hub, name, old.generation);
    }
    Ok(())
}

pub(crate) fn close(hub: &Hub, name: &MachineName, generation: u64) {
    {
        let mut store = hub.store();
        let mut sessions = hub.sessions();
        if sessions.get(name).is_some_and(|session| session.generation == generation) {
            sessions.remove(name);
            hub.forget_view(name, generation);
            // Renamed while connected, it may be connected again under its
            // name now, which what is kept of it is under.
            let current = store.book.directory.canonical(name).clone();
            let back = sessions.contains_key(&current);
            drop(sessions);
            if !back && let Some(entry) = store.letters.get_mut(&current) {
                entry.connected_at = None;
                entry.last_seen = Some(Timestamp::now());
            }
            if let Err(error) = store.save_letters() {
                hub.log.line(format_args!("recording that {name} left: {error}"));
            }
        }
        if store.intake.get(name) == Some(&generation) {
            store.intake.remove(name);
        }
        crate::mailbox::closed(&mut store, name, generation);
    }
    // Data connections it registered and nobody paired wait no longer; a
    // session that replaced this one keeps its own.
    crate::data::member_left(hub, name, generation);
    crate::updates::closed(hub, name, generation);
    fail_calls_of(hub, name, generation);
}

/// Ends every call this session made or was to answer.
fn fail_calls_of(hub: &Hub, name: &MachineName, generation: u64) {
    let ended: Vec<(u64, Pending)> = {
        let mut calls = hub.calls.lock().expect("never poisoned");
        let keys: Vec<u64> = calls
            .iter()
            .filter(|(_, pending)| {
                (pending.target == *name && pending.target_generation == generation)
                    || (pending.caller == *name && pending.caller_generation == generation)
            })
            .map(|(key, _)| *key)
            .collect();
        keys.into_iter().filter_map(|key| calls.remove(&key).map(|pending| (key, pending))).collect()
    };
    for (key, pending) in ended {
        if let Some(stream) = pending.stream {
            crate::data::forget(hub, stream, &format!("{name}, a party to its call, left the hub"));
        }
        if pending.target == *name {
            hub.tell(&pending.caller, Down::Status { id: pending.id, status: CallStatus::Lost });
        } else {
            over(hub, key, &pending);
        }
    }
}

/// Tells the machine answering the call the hub numbered `key` that it is
/// over for its caller, when the session it came on is still there and
/// reads that.
fn over(hub: &Hub, key: u64, pending: &Pending) {
    let sessions = hub.sessions();
    if let Some(target) = sessions
        .get(&pending.target)
        .filter(|session| session.generation == pending.target_generation && session.cancels)
    {
        let _ = target.out.send(Out::Frame(Down::Cancelled { call: key }));
    }
}

/// Ends every call in flight whose standing lapsed, telling its caller.
pub(crate) fn cut_calls(hub: &Hub) {
    // No lock is held while another is taken: the calls are copied out,
    // judged against the directory, and then taken out.
    let standings: Vec<(u64, Standing)> = hub
        .calls
        .lock()
        .expect("never poisoned")
        .iter()
        .map(|(key, pending)| (*key, pending.standing.clone()))
        .collect();
    // Judged under one look at the directory, not one for each call.
    let keys: Vec<(u64, String)> = {
        let store = hub.store();
        standings
            .into_iter()
            .filter_map(|(key, standing)| standing.lapsed_in(hub, &store).map(|why| (key, why)))
            .collect()
    };
    let lapsed: Vec<(u64, Pending, String)> = {
        let mut calls = hub.calls.lock().expect("never poisoned");
        keys.into_iter().filter_map(|(key, why)| calls.remove(&key).map(|pending| (key, pending, why))).collect()
    };
    for (key, pending, why) in lapsed {
        hub.log.line(format_args!("cut a call from {} to {}: {why}", pending.caller, pending.target));
        let status = CallStatus::Cut { reason: format!("the right it was made under was taken back: {why}") };
        hub.tell(&pending.caller, Down::Status { id: pending.id, status });
        over(hub, key, &pending);
    }
}

/// Does what the member's frame asks; `took`, how many bytes of this
/// connection the hub took from it, goes back with its heartbeat.
#[allow(clippy::too_many_arguments)]
fn handle(
    hub: &Arc<Hub>,
    name: &MachineName,
    key: PublicKey,
    generation: u64,
    out: &Outq,
    took: u64,
    frame: Up,
    reservation: Option<fastctx_net::reassembly::Reservation>,
) -> Result<(), Refusal> {
    let reply = |frame: Down| {
        let _ = out.send(Out::Frame(frame));
    };
    match frame {
        Up::Ping { n, paths, held, .. } => {
            crate::assists::heard(hub);
            reply(Down::Pong { n, at: Some(Timestamp::now()), took: Some(took), afoot_ready: false });
            if let Some(held) = held {
                refusals_heard(hub, name, generation, |refused| refused <= held);
            }
            if let Some(paths) = paths {
                lines(hub, name, generation, paths);
            }
        }
        Up::Ask { id, question: Question::Manage { account, change: Change::Direct { machine, at } } } => {
            let (hub, asker, out) = (hub.clone(), name.clone(), out.clone());
            let held = crate::moving::Moving::hold(&hub);
            tokio::spawn(async move {
                let _held = held;
                let answer = crate::direct::change(hub, asker, account, machine, at).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        // Letters are kept by the session's own task (`intake`).
        Up::Send { .. } => {}
        Up::Ack { seq } => crate::mailbox::taken(hub, &mut hub.store(), name, generation, seq),
        Up::Call { id, message } => reply(Down::Status { id, status: call(hub, name, generation, id, message) }),
        Up::Answer { call, message } => answer(hub, name, generation, call, message),
        Up::Afoot { change } => crate::afoot::heard(hub, name, *change),
        Up::Forgot { id, outcome } => crate::forget::forgot(hub, name, id, outcome),
        Up::Versions { report } => crate::updates::heard(hub, name, generation, *report),
        Up::Unfetched { id, why } => crate::updates::unfetched(hub, name, id, why),
        Up::FetchProgress { id, progress } => crate::updates::progress(hub, name, generation, id, progress),
        Up::PackageDone { id } => crate::updates::done(hub, name, generation, id),
        Up::Ask { id, question: Question::Package { version, target, progress } } => {
            crate::updates::ask(hub, name, generation, id, version, target, progress, out.clone());
        }
        Up::Ask { id, question: Question::NetworkUpdates { account, request } } => {
            let (hub, name, out) = (hub.clone(), name.clone(), out.clone());
            tokio::spawn(async move {
                let answer = crate::updates::view_request(&hub, &name, &account, request).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: Question::PrepareUpdate { machine, version } } => {
            let (hub, out) = (hub.clone(), out.clone());
            tokio::spawn(async move {
                let answer = crate::updates::prepare_update(&hub, &machine, &version).await.map(|()| Told::Done);
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: question @ (Question::ProjectIndex { .. } | Question::MemoryRead { .. }) } => {
            crate::ledger::requests::ask(hub, name, out, id, question);
        }
        Up::Ask { id, question: Question::MemoryWrite { town, account, entry } } => {
            let (hub, writer, out) = (hub.clone(), name.clone(), out.clone());
            let held = crate::moving::Moving::hold(&hub);
            tokio::spawn(async move {
                let (_held, _reservation) = (held, reservation);
                let answer = crate::ledger::write_as(&hub, &writer, town, entry, Some(account)).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: Question::LedgerRead { town, after } } => {
            crate::ledger::ask(hub, name, out, id, town, after);
        }
        // Written off the store's lock, one entry of a project at a time.
        Up::Ask { id, question: Question::LedgerWrite { town, entry } } => {
            let (hub, writer, out) = (hub.clone(), name.clone(), out.clone());
            let held = crate::moving::Moving::hold(&hub);
            tokio::spawn(async move {
                let _held = held;
                let _reservation = reservation;
                let answer = crate::ledger::write(&hub, &writer, town, entry).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Cancel { id } => {
            let given_up: Vec<(u64, Pending)> = {
                let mut calls = hub.calls.lock().expect("never poisoned");
                calls
                    .asked_by(name, generation, id)
                    .and_then(|key| calls.remove(&key).map(|pending| (key, pending)))
                    .into_iter()
                    .collect()
            };
            for (key, pending) in given_up {
                if let Some(stream) = pending.stream {
                    crate::data::forget(hub, stream, "its caller gave the call up");
                }
                over(hub, key, &pending);
            }
        }
        // The answer goes out before any session the change ends, the
        // asker's own among them: a machine that renamed or removed itself
        // hears that it did.
        Up::Ask { id, question: Question::AccountRecords { account, request } } => {
            let opened = match crate::account_records::begin(hub, name, &key, &account, &request) {
                Ok(opened) => opened,
                Err(why) => {
                    reply(Down::Told { id, answer: Err(why) });
                    return Ok(());
                }
            };
            let (hub, from, out) = (hub.clone(), name.clone(), out.clone());
            let held = crate::moving::Moving::hold(&hub);
            tokio::spawn(async move {
                let _held = held;
                let answer = crate::account_records::ask(&hub, from, key, account, opened, request).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: Question::Manage { account, change } } => {
            let (answer, ended) = crate::manage::change(hub, name, &account, change);
            reply(Down::Told { id, answer });
            crate::manage::end(hub, ended);
            // The network's update policy, or an update kept for a machine,
            // may be what changed.
            crate::updates::prefetch(hub);
            crate::updates::tell(hub, None);
            crate::updates::prune(hub);
        }
        Up::Ask { id, question: Question::KernelRelease { version, allow_partial, wanted } } => {
            let releases = hub.releases.clone();
            let out = out.clone();
            let hub = hub.clone();
            tokio::spawn(async move {
                let read = releases.clone();
                let answer = match tokio::task::spawn_blocking(move || read.ready(&version, allow_partial)).await {
                    Ok(Ok(mut release)) => {
                        let version = release.version.clone();
                        for (archive, _) in fastctx_types::kernel::HELD_ARCHIVES {
                            let bring = if wanted.is_empty() {
                                archive == fastctx_types::kernel::WINDOWS_X86_64_ARCHIVE
                            } else {
                                wanted.iter().any(|wanted| wanted == archive)
                            };
                            if let Some(held) = release.held(archive)
                                && held.is_none()
                            {
                                *held = crate::updates::held_digest(&hub, &version, archive, bring).await;
                            }
                        }
                        Ok(Told::KernelRelease { release, certificate: releases.certificate.clone() })
                    }
                    Ok(Err(reason)) => Err(reason),
                    Err(error) => Err(format!("reading the kernel release: {error}")),
                };
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question } => {
            let answer = ask(hub, name, question);
            reply(Down::Told { id, answer });
        }
        Up::QuestionPart { .. } | Up::CancelQuestion { .. } => unreachable!("assembled by the connection"),
        Up::Hello { .. } => {
            return Err(Refusal::Broken { reason: "a second hello on one connection".to_string() });
        }
        Up::LoginFinish { .. } | Up::RegisterFinish { .. } => {
            return Err(Refusal::Broken { reason: "a member's session logs nobody in".to_string() });
        }
    }
    Ok(())
}

/// Checks an envelope from the member `name`: it says it is from `name`
/// and for this hub. A name the member had before it was renamed is its
/// own all the same, as on a letter it kept since.
fn own(hub: &Hub, name: &MachineName, message: &Envelope) -> Result<(), String> {
    let store = hub.store();
    let directory = &store.book.directory;
    if !directory.same(&message.from, name) {
        return Err(format!("it says it is from {}, on the connection of {name}", message.from));
    }
    if message.hub != store.hub.id {
        return Err(format!("it is for the hub {}", message.hub));
    }
    if directory.same(&message.to, name) {
        return Err("a machine's own calls and letters do not go through the hub".to_string());
    }
    Ok(())
}

/// What a name stands for now, by the directory (`Directory::canonical`).
fn canonical(hub: &Hub, name: &MachineName) -> MachineName {
    hub.store().book.directory.canonical(name).clone()
}

/// `address` with its machine named as it is now.
fn canonical_address(hub: &Hub, address: fastctx_types::Address) -> fastctx_types::Address {
    fastctx_types::Address { machine: canonical(hub, &address.machine), resident: address.resident }
}

/// The standing a call or a letter names, if the directory allows it: of
/// the machines as they are named now.
fn standing(hub: &Hub, message: &Envelope) -> Result<Standing, String> {
    let (account, purpose) = message.standing()?;
    let (from, to) = (canonical(hub, &message.from), canonical(hub, &message.to));
    let project_entry = fastctx_net::message::project_entry(message);
    if project_entry.is_some() && purpose != Purpose::Echo {
        return Err("a project entry must return to its conversation as a job request".into());
    }
    let plan = fastctx_net::message::plan_request(message);
    if plan.is_some() && purpose != Purpose::Echo {
        return Err(fastctx_types::plan::words::CONTROL.into());
    }
    if message.body.get("call").and_then(|call| call.get("call")).and_then(serde_json::Value::as_str) == Some("scratch")
        && hub.sessions().get(&to).is_some_and(|session| !session.scratch_locations)
    {
        return Err(fastctx_types::scratch::words::OLD_NODE.into());
    }
    let standing = Standing { from, account: account.clone(), to, purpose, project_entry, plan };
    match standing.lapsed(hub) {
        Some(why) => Err(why),
        None => Ok(standing),
    }
}

/// Keeps the letter `seq` from `name`, or refuses it: what the sender is to
/// be told, and its count once this letter is counted, which is kept before
/// it is told (`intake`). The letter is on disk before it counts: a sender
/// told the hub holds one only once the hub does.
/// Writes the letter `message`, numbered for `to` as `numbered`, and
/// settles its number: kept when it is written for the member it was
/// numbered for, a gap when it is not written or that member is gone.
async fn write(
    hub: Arc<Hub>,
    to: MachineName,
    numbered: crate::store::Numbered,
    message: Envelope,
) -> std::io::Result<()> {
    let path = numbered.path.clone();
    let written = tokio::task::spawn_blocking(move || crate::store::write_letter(&path, &message))
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))
        .and_then(|written| written);
    let gone = {
        let mut store = hub.store();
        // Renamed meanwhile, its receiver is known by its name now; its
        // letters stay where they were numbered for.
        let to = store.book.directory.canonical(&to).clone();
        let size = match written {
            Ok(size) => size,
            Err(error) => {
                crate::mailbox::abandoned(&hub, &mut store, &to, numbered.epoch, numbered.seq);
                return Err(error);
            }
        };
        // Its receiver was taken off meanwhile, or another member took its
        // name and numbers its letters anew: the letter goes nowhere, and
        // nothing of the other's is touched.
        let now = store.book.directory.member(&to).map(|member| member.key);
        let numbering = store.letters.get(&to).map(|entry| entry.out_epoch);
        if now.is_some() && now == numbered.key && numbering == Some(numbered.epoch) {
            crate::mailbox::kept(&hub, &mut store, &to, numbered.epoch, numbered.seq, size);
            false
        } else {
            crate::mailbox::abandoned(&hub, &mut store, &to, numbered.epoch, numbered.seq);
            true
        }
    };
    if gone {
        let _ = tokio::fs::remove_file(&numbered.path).await;
        if let Some(directory) = numbered.path.parent() {
            let _ = tokio::fs::remove_dir(directory).await;
        }
    }
    Ok(())
}

/// What became of a letter a member sent.
enum Kept {
    /// The hub holds it.
    Held,
    /// The hub refused it, saying why.
    Refused(String),
    /// Sent again after a reconnect: the hub holds it already, or refused it
    /// then, saying why.
    Again(Option<String>),
}

/// Why keeping a session's letters stopped.
enum Stopped {
    /// A newer session of the member keeps them now.
    Replaced,
    /// The hub's own disk failed.
    Failed(Refusal),
}

/// Keeps the letter `seq` the session `generation` of `name` sent: written
/// to disk before its sender's count moves past it. Only the member's
/// current session moves its count: a letter a session replaced since was
/// writing is kept all the same, and the new session sends it again, which
/// its receiver's receipt of it takes once.
async fn keep(
    hub: &Arc<Hub>,
    name: &MachineName,
    generation: u64,
    seq: u64,
    message: Envelope,
) -> Result<Kept, Stopped> {
    {
        let store = hub.store();
        if store.intake.get(name) != Some(&generation) {
            return Err(Stopped::Replaced);
        }
        if let Some(entry) = store.letters.get(name)
            && seq <= entry.taken
        {
            return Ok(Kept::Again(entry.refused.get(&seq).cloned()));
        }
    }
    let failed = |error: std::io::Error| Stopped::Failed(storage_failure(hub, "keeping a letter", &error));
    let refused = match own(hub, name, &message).and_then(|()| {
        if message.kind != Kind::Letter {
            return Err("only letters are kept for later".to_string());
        }
        if hub.store().book.directory.member(&message.to).is_none() {
            return Err(format!("{} is not a member of the hub", message.to));
        }
        match standing(hub, &message) {
            Ok(_) => Ok(()),
            Err(_) if echoes_work(hub, &message) => Ok(()),
            Err(reason) => Err(reason),
        }
    }) {
        Ok(()) => {
            let to = canonical(hub, &message.to);
            let numbered = {
                let mut store = hub.store();
                if store.intake.get(name) != Some(&generation) {
                    return Err(Stopped::Replaced);
                }
                store.next_letter(&to).map_err(failed)?
            };
            // Written and settled whatever becomes of this session: a
            // number given out and left unsettled holds back every letter
            // after it.
            let writing = write(hub.clone(), to, numbered, message);
            let held = crate::moving::Moving::hold(hub);
            let written = tokio::spawn(async move {
                let _held = held;
                writing.await
            })
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))
            .and_then(|written| written);
            written.map_err(failed)?;
            None
        }
        Err(reason) => Some(reason),
    };
    {
        let mut store = hub.store();
        if store.intake.get(name) != Some(&generation) {
            return Err(Stopped::Replaced);
        }
        let Some(entry) = store.letters.get_mut(name) else { return Err(Stopped::Replaced) };
        entry.taken = entry.taken.max(seq);
        if let Some(reason) = &refused {
            entry.refused.insert(seq, reason.clone());
        }
        let _ = store.save_letters();
    }
    Ok(match refused {
        Some(reason) => {
            hub.log.line(format_args!("refused letter {seq} from {name}: {reason}"));
            Kept::Refused(reason)
        }
        None => Kept::Held,
    })
}

/// The same authorization facts for an inline call and one carried over a data connection.
fn call_details(message: &Envelope, target_verifies: bool) -> Result<fastctx_net::call_details::CallDetails, String> {
    use fastctx_net::{call_details::CallDetails, message::Body};
    match message.body::<Body>().map_err(|error| error.to_string())? {
        Body::Call { call } | Body::StreamCall { call } => Ok(CallDetails::of(&message.from, &call)),
        Body::LargeCall { details, .. } => {
            let details = details.ok_or("upgrade the sender: its large call has no authorization facts")?;
            if message.purpose != Some(details.purpose) {
                return Err("the large call's purpose differs from its authorization facts".into());
            }
            if !target_verifies {
                return Err(format!("upgrade {}: it cannot verify a large call's authorization facts", message.to));
            }
            Ok(details)
        }
        _ => Err("the envelope does not carry a call".into()),
    }
}

/// Keeps what `name` says of how its lines go, and tells the members to
/// look again when which machines it is linked with directly changed.
fn lines(hub: &Hub, name: &MachineName, generation: u64, paths: fastctx_net::directory::Paths) {
    let linked = |paths: &fastctx_net::directory::Paths| -> Vec<(MachineName, bool)> {
        paths.links.iter().map(|link| (link.peer.clone(), link.up)).collect()
    };
    let changed = {
        let mut sessions = hub.sessions();
        let Some(session) = sessions.get_mut(name).filter(|session| session.generation == generation) else {
            return;
        };
        let changed = session.paths.as_ref().map(linked) != Some(linked(&paths));
        session.paths = Some(paths);
        changed
    };
    if changed {
        hub.present(name);
    }
}

/// Whether `message` is a letter telling a resident what came of work its
/// machine was called to do for it, such as a job's end or a colleague's
/// answer: that goes on after the account's right on the machine is taken
/// back, as long as the account stands and the resident's machine still
/// hosts it (`world-network.md`, 收回立刻生效).
fn echoes_work(hub: &Hub, message: &Envelope) -> bool {
    let (Ok((account, Purpose::Echo)), Ok(body)) =
        (message.standing(), message.body::<fastctx_net::message::LetterBody>())
    else {
        return false;
    };
    let fastctx_types::remote::Letter::Notice { to, .. } = body.letter else { return false };
    let store = hub.store();
    let directory = &store.book.directory;
    let (from, machine) = (directory.canonical(&message.from).clone(), directory.canonical(&message.to).clone());
    let resident = fastctx_types::Address { machine, resident: to };
    directory.admits_echo(&message.from, account, &message.to).is_ok() && store.worked.contains(&(from, resident))
}

/// The call a message carries, when it carries one.
fn carried_call(message: &Envelope) -> Option<fastctx_types::remote::RemoteCall> {
    match message.body::<fastctx_net::message::Body>().ok()? {
        fastctx_net::message::Body::Call { call } | fastctx_net::message::Body::StreamCall { call } => Some(call),
        _ => None,
    }
}

/// Preparing a draft's non-image attachment already works for its account:
/// the host forwards it to that account's storage before a model starts.
fn upload_login(hub: &Hub, message: &Envelope, standing: &Standing) -> Option<(AgentName, Option<std::path::PathBuf>)> {
    let fastctx_types::remote::RemoteCall::Upload { target, file, .. } = carried_call(message)? else { return None };
    if file.media_type.starts_with("image/") {
        return None;
    }
    let agent = match target.conversation_target() {
        fastctx_types::attachments::UploadTarget::New { agent, .. } => agent.clone(),
        fastctx_types::attachments::UploadTarget::Draft { resident, .. } => resident.agent(),
        _ => return None,
    };
    if !matches!(agent.as_str(), "codex" | "claude") {
        return None;
    }
    let store = hub.store();
    let directory = &store.book.directory;
    if directory.rights(&standing.account, &standing.to).hands {
        return Some((agent, None));
    }
    directory
        .assisting(&standing.account, &standing.to)
        .find(|entry| entry.agent == agent)
        .map(|entry| (agent.clone(), entry.home.clone()))
}

/// Checks that a call starting a session may use that agent's login on the
/// machine it goes to, and notes that machine acts for the account from now
/// on when it did not: an agent of another's machine that works for an
/// account has to reach the account's machines, and using it is the
/// account's word for that (`world-network.md`, 一台机器什么时候能代一个
/// 账号访问别的机器). Being shared a machine or assisted by a login does not
/// do it.
fn use_agent(
    hub: &Hub,
    message: &Envelope,
    standing: &Standing,
    details: &fastctx_net::call_details::CallDetails,
) -> Result<(), String> {
    let Some((agent, home)) = details.login.clone().or_else(|| upload_login(hub, message, standing)) else {
        return Ok(());
    };
    let (account, to) = (&standing.account, &standing.to);
    let noted = {
        let mut store = hub.store();
        let directory = &store.book.directory;
        if directory.member(to).is_some_and(|member| member.managed) {
            return Err(format!("{to} is a managed machine: no conversation lives there"));
        }
        if !directory.may_use_login(account, to, &agent, home.as_deref()) {
            let login = home.as_ref().map(|home| home.display().to_string()).unwrap_or_else(|| "default".to_string());
            return Err(format!("the account {account} may not use the {agent} login {login} on {to}"));
        }
        if directory.may_act_for(account, to) {
            false
        } else {
            let before = store.book.clone();
            store.book.directory.acting.push(std::sync::Arc::new(fastctx_net::directory::ActsFor {
                machine: to.clone(),
                account: account.clone(),
                since: Timestamp::now(),
            }));
            let actor = fastctx_types::machine_log::Actor {
                account: Some(account.clone()),
                authenticated: true,
                member: Some(message.from.clone()),
                peer: None,
            };
            let action = fastctx_types::machine_log::Action::AllowActing { machine: to.clone(), agent, home };
            if let Err(reason) = crate::machine_log::keep(
                &mut store,
                &before.directory,
                actor,
                action,
                fastctx_types::machine_log::Outcome::Done,
            ) {
                store.book = before;
                return Err(reason);
            }
            true
        }
    };
    if noted {
        hub.log.line(format_args!("{to} acts for {account} from now on: it used an agent there"));
        hub.changed();
    }
    Ok(())
}

fn call(hub: &Hub, name: &MachineName, generation: u64, id: u64, message: Envelope) -> CallStatus {
    if let Err(reason) = own(hub, name, &message) {
        return CallStatus::Refused { reason };
    }
    if message.kind != Kind::Call {
        return CallStatus::Refused { reason: format!("a {:?} is not a call", message.kind) };
    }
    let to = canonical(hub, &message.to);
    let last_seen = {
        let store = hub.store();
        if store.book.directory.member(&to).is_none() {
            return CallStatus::NotMember;
        }
        store.letters.get(&to).and_then(|entry| entry.last_seen)
    };
    let standing = match standing(hub, &message) {
        Ok(standing) => standing,
        Err(reason) => return CallStatus::Refused { reason },
    };
    let target = hub.sessions().get(&to).map(|session| (session.generation, session.out.clone(), session.call_details));
    let Some((target_generation, target, target_verifies)) = target else {
        return CallStatus::Offline { last_seen };
    };
    let details = match call_details(&message, target_verifies) {
        Ok(details) => details,
        Err(reason) => return CallStatus::Refused { reason },
    };
    if let Err(reason) = use_agent(hub, &message, &standing, &details) {
        return CallStatus::Refused { reason };
    }
    if let Some(resident) = details.resident.map(|resident| canonical_address(hub, resident))
        && let Err(error) = hub.store().note_worked(&to, resident)
    {
        hub.log.line(format_args!("noting that {to} works for a resident: {error}"));
    }
    if standing.project_entry.is_some() && hub.sessions().get(&to).is_some_and(|session| !session.project_entry) {
        return CallStatus::Refused {
            reason: format!(
                "{to} runs an older Fastctx that cannot enter projects from another machine; update it first"
            ),
        };
    }
    let key = hub.next();
    if let Some(ticket) = &message.stream {
        crate::data::register(hub, ticket, &standing, name, generation);
    }
    let stream = message.stream.as_ref().map(|ticket| ticket.stream);
    let pending = Pending {
        caller: name.clone(),
        caller_generation: generation,
        id,
        target: to.clone(),
        target_generation,
        standing,
        stream,
        at: tokio::time::Instant::now(),
    };
    {
        let mut calls = hub.calls.lock().expect("never poisoned");
        if let Some(reason) = crowded(&calls, &pending) {
            drop(calls);
            if let Some(stream) = stream {
                crate::data::forget(hub, stream, &reason);
            }
            return CallStatus::Refused { reason };
        }
        // Kept only while the session it is for is there, as seen under
        // this lock (the sessions are locked after the calls, never
        // before): one that closes after this finds it and ends it
        // (`fail_calls_of`); one closed before is not called.
        if hub.sessions().get(&to).is_none_or(|session| session.generation != target_generation) {
            drop(calls);
            if let Some(stream) = stream {
                crate::data::forget(hub, stream, &format!("{to} left the hub before it was called"));
            }
            return CallStatus::Offline { last_seen: Some(Timestamp::now()) };
        }
        calls.insert(key, pending);
    }
    if target.send(Out::Frame(Down::Incoming { call: key, message })).is_err() {
        hub.calls.lock().expect("never poisoned").remove(&key);
        if let Some(stream) = stream {
            crate::data::forget(hub, stream, &format!("{to} left the hub before it was called"));
        }
        return CallStatus::Offline { last_seen: Some(Timestamp::now()) };
    }
    CallStatus::Delivered
}

fn answer(hub: &Hub, name: &MachineName, generation: u64, call: u64, message: Envelope) {
    let pending = {
        let mut calls = hub.calls.lock().expect("never poisoned");
        match calls.get(&call) {
            Some(pending) if pending.target == *name && pending.target_generation == generation => calls.remove(&call),
            _ => None,
        }
    };
    // The caller may have given up or left; an answer nobody waits for is
    // dropped, as a call does not outlive its caller.
    let Some(pending) = pending else { return };
    // Its data connection, if it is not paired by now, never is: the call
    // is over.
    if let Some(stream) = pending.stream {
        crate::data::forget(hub, stream, "its call was answered");
    }
    let fits = {
        let store = hub.store();
        let directory = &store.book.directory;
        directory.same(&message.from, name) && message.kind.is_answer() && directory.same(&message.to, &pending.caller)
    };
    if !fits {
        hub.log.line(format_args!("{name} answered call {call} with something that is not its answer"));
        hub.tell(&pending.caller, Down::Status { id: pending.id, status: CallStatus::Lost });
        return;
    }
    if let Some(ticket) = &message.stream {
        crate::data::register(hub, ticket, &pending.standing, name, generation);
    }
    let stream = message.stream.as_ref().map(|ticket| ticket.stream);
    let queued = {
        let sessions = hub.sessions();
        sessions
            .get(&pending.caller)
            .filter(|session| session.generation == pending.caller_generation)
            .is_some_and(|caller| caller.out.send(Out::Frame(Down::Answered { id: pending.id, message })).is_ok())
    };
    if !queued && let Some(stream) = stream {
        crate::data::forget(hub, stream, "its caller left before the answer could be queued");
    }
}

fn ask(hub: &Arc<Hub>, name: &MachineName, question: Question) -> Result<Told, String> {
    match question {
        Question::AccountRecords { .. } => Err("account records are dispatched asynchronously".into()),
        Question::NetworkUpdates { .. } => Err("update management is dispatched asynchronously".into()),
        Question::MachineLog { account, before } => {
            let store = hub.store();
            let reads =
                store.letters.get(name).map_or_else(Default::default, |letters| fastctx_types::machine_log::Reads {
                    renames: letters.renames,
                    forgets: letters.forgets,
                    versions: letters.versions,
                    due: letters.due,
                    project_deletes: letters.project_deletes,
                    profiles: letters.profiles,
                });
            store
                .book
                .machine_log
                .read(&store.book.directory, name, &account, before, reads)
                .map(|page| Told::MachineLog { page })
        }
        Question::KernelRelease { .. } | Question::Package { .. } | Question::PrepareUpdate { .. } => {
            Err("release reads are dispatched asynchronously".into())
        }
        Question::Directory => Ok(Told::Directory { view: Box::new(hub.view_for(name)) }),
        Question::ProjectDeleted { town } => crate::project_deletions::finished(hub, name, &town),
        Question::Manage { .. } => Err("changes are made as they come".into()),
        Question::LedgerWrite { .. } | Question::MemoryWrite { .. } => {
            Err("ledger entries are written as they come".into())
        }
        Question::ProjectIndex { .. } | Question::MemoryRead { .. } => {
            Err("project reads are dispatched asynchronously".into())
        }
        Question::LedgerTowns => {
            let store = hub.store();
            let towns = hub.ledgers.lock().expect("never poisoned").towns_of(&store.book.directory, name);
            Ok(Told::Towns { towns })
        }
        Question::LedgerCatalog => {
            let store = hub.store();
            let directory = &store.book.directory;
            let towns = hub
                .ledgers
                .lock()
                .expect("never poisoned")
                .catalog()
                .into_iter()
                .filter(|(town, _)| directory.member_in_project(name, town))
                .collect();
            Ok(Told::Towns { towns })
        }
        Question::HubMove { account, request } => crate::moving::requests::ask(hub, name, &account, request),
        Question::HubArrival { request } => crate::moving::requests::arrival(hub, name, request),
        Question::LedgerRead { .. } => Err("ledger reads are dispatched asynchronously".into()),
        Question::Worked { to, account, purpose, resident } => {
            // What the hub notes of a call it passes on, for one made over
            // a direct link: only while that call is allowed.
            let mut store = hub.store();
            store.book.directory.admits(name, &account, &to, purpose)?;
            let directory = &store.book.directory;
            let to = directory.canonical(&to).clone();
            let resident = fastctx_types::Address {
                machine: directory.canonical(&resident.machine).clone(),
                resident: resident.resident,
            };
            store.note_worked(&to, resident).map_err(|error| format!("noting the work: {error}"))?;
            Ok(Told::Done)
        }
        Question::Listening { at, outcome } => hub.listening(name, &at, outcome),
        Question::Forget { account, conversation } => crate::forget::ask(hub, name, account, conversation),
    }
}
