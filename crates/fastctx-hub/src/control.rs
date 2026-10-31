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
use fastctx_net::wire::{CallStatus, Change, Down, HEARTBEAT, Letters, Question, Refusal, SILENCE, Told, Up};
use fastctx_types::access::Purpose;
use fastctx_types::{AccountName, AgentName, MachineName};
use jiff::Timestamp;
use tokio::sync::mpsc;

mod encoding;
mod transport;

use crate::outq::Outq;
use crate::{Hub, Out, Session, Socket, refuse, storage_failure};

/// What a member says of itself when its session opens.
pub(crate) struct Said {
    pub replicated_records: bool,
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
    pub version_parts: bool,
    /// Whether it reads the records of updates due and made as asked.
    pub due: bool,
    /// Whether it reads the records of projects deleted.
    pub project_deletes: bool,
    pub project_entry: bool,
    pub conversation_deletions: bool,
    /// Whether it reads the records of profiles written.
    pub profiles: bool,
    pub account_records: bool,
    pub record_keys: bool,
    pub skill_files: bool,
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
    from_key: PublicKey,
    to_key: PublicKey,
    pub account: AccountName,
    pub to: MachineName,
    pub purpose: Purpose,
    pub project_entry: Option<(fastctx_types::ResidentId, fastctx_types::TownName)>,
    pub plan: Option<fastctx_types::ResidentId>,
}

impl Standing {
    pub(crate) fn identity_changed(&self, store: &crate::store::Snapshot) -> Option<String> {
        let index = store.indexed();
        (index.member(&self.from).is_none_or(|member| member.key != self.from_key)
            || index.member(&self.to).is_none_or(|member| member.key != self.to_key))
        .then(|| "a party to the call was removed or replaced".into())
    }

    /// Why this standing no longer holds in the caller's catalog snapshot.
    fn lapsed_in(&self, hub: &Hub, store: &crate::store::Snapshot) -> Option<String> {
        if let Some(why) = self.identity_changed(store) {
            return Some(why);
        }
        store
            .indexed()
            .authorize(
                &self.account,
                fastctx_net::authorization::Action::Call { from: &self.from, to: &self.to, purpose: self.purpose },
            )
            .map_err(|error| error.message)
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
                let left = left.clone();
                hub.blocking(move |hub| crate::manage::left(&hub, &key, &left)).await;
            }
            let tells = crate::dissolve::tells(&reason);
            if refuse(&mut socket, reason).await && tells {
                hub.blocking(move |hub| crate::dissolve::told(&hub, &key)).await;
            }
            return;
        }
    };
    let generation = hub.next();
    let reads_afoot = said.afoot;
    let out = Outq::new(said.parts);
    if let Err(reason) = open(&hub, &name, key, generation, out.clone(), letters, peer, said).await {
        hub.log.line(format_args!("{peer}: the session of {name} could not open: {reason}"));
        refuse(&mut socket, reason).await;
        return;
    }
    // A hub that began handing over after this session was let in, but
    // before it was registered, did not end it.
    if hub.moving.frozen() {
        let _ = out.send(Out::End(Refusal::ShuttingDown));
    }
    let arriving = name.clone();
    hub.blocking(move |hub| crate::moving::follow::came(&hub, &arriving, dialed)).await;
    crate::forget::opened(&hub, &name);
    match &through {
        Some(through) => hub.log.line(format_args!("{name} connected through {through}, from {peer}")),
        None => hub.log.line(format_args!("{name} connected from {peer}")),
    }
    // Whoever sees it sees it online now.
    hub.health.opened();
    hub.troubles.connected(&hub, &name, key);
    hub.present(&name);
    if reads_afoot {
        crate::afoot::came(&hub, &name);
    } else {
        out.afoot_started();
    }

    let mut transport = transport::Transport::start(socket, &hub.network);
    let output = transport.outgoing.clone();
    let (stopped, mut writer_stopped) = tokio::sync::oneshot::channel::<()>();
    let (telling, member, member_name, queue) = (hub.clone(), key, name.clone(), out.clone());
    let writer = tokio::spawn(async move {
        // Dropped when the writer returns, which the reader notices.
        let _stopped = stopped;
        loop {
            let next = queue.next().await;
            let Some(next) = next else { break };
            let (preparing, reader) = (telling.clone(), member_name.clone());
            let (frames, last) =
                match tokio::task::spawn_blocking(move || encoding::prepare(&preparing, &reader, member, next)).await {
                    Ok(Ok(prepared)) => prepared,
                    Ok(Err(error)) => {
                        telling.log.line(format_args!("{member_name}: {error}"));
                        return;
                    }
                    Err(error) => {
                        telling.log.line(format_args!("{member_name}: preparing outgoing frames: {error}"));
                        return;
                    }
                };
            for (bytes, dissolved) in frames {
                let (written, received) = tokio::sync::oneshot::channel();
                tokio::select! {
                    sent = output.send(transport::Packet { bytes, last, written: dissolved.then_some(written) }) => {
                        if sent.is_err() { return; }
                    }
                    why = queue.overflowed() => {
                        telling.log.line(format_args!("{member_name}: {why}"));
                        return;
                    }
                }
                // A machine on the network, told it was dissolved.
                if dissolved && received.await.is_ok() {
                    telling.blocking(move |hub| crate::dissolve::told(&hub, &member)).await;
                }
            }
            if last {
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
    let mut uploads = crate::question_parts::Uploads::default();
    let mut upload_clock = tokio::time::interval(HEARTBEAT);
    let mut heard = crate::strain::Heard::default();
    let (ended, refusing) = loop {
        let next = tokio::select! {
            next = transport.incoming.recv() => next.map(Ok),
            changed = transport.heartbeat.changed() => {
                if changed.is_err() { break (None, false); }
                let Some(frame) = transport.heartbeat.borrow_and_update().clone() else { continue };
                if let Up::Ping { health, .. } = &frame {
                    heard.heartbeat(&hub, &name, key, generation, health.as_ref());
                }
                Some(Err(frame))
            },
            _ = upload_clock.tick() => {
                for id in uploads.expired() {
                    let _ = out.send(Out::Frame(Down::Told { id, answer: Err("the ledger upload exceeded 60 seconds".into()) }));
                }
                continue;
            }
            _ = transport.ended.changed() => {
                break (transport.ended.borrow().as_ref().map(|ended| ended.reason.clone()), false);
            }
            // The writer stopped: the connection was replaced or ended.
            _ = &mut writer_stopped => break (None, false),
        };
        let frame = match next {
            None => break (transport.ended.borrow().as_ref().map(|ended| ended.reason.clone()), false),
            Some(Err(frame)) => frame,
            Some(Ok(input)) => {
                let bytes = input.bytes.clone();
                match tokio::task::spawn_blocking(move || serde_json::from_slice::<Up>(&bytes)).await {
                    Ok(Ok(frame)) => frame,
                    Ok(Err(error)) => {
                        let (reading, member_name, queue) = (hub.clone(), name.clone(), out.clone());
                        let _ = tokio::task::spawn_blocking(move || {
                            unreadable(&reading, &member_name, &queue, &input.bytes, &error)
                        })
                        .await;
                        continue;
                    }
                    Err(error) => break (Some(format!("decoding a frame: {error}")), false),
                }
            }
        };
        // The transport keeps reading heartbeats while this member's
        // durable intake catches up; no other member waits for its disk.
        if let Up::Send { seq, message } = frame {
            if letters_in.send((seq, message)).await.is_err() {
                break (Some("keeping its letters stopped".into()), false);
            }
            continue;
        }
        let (frame, reservation) = match frame {
            Up::QuestionPart { id, index, total, text } => {
                let pushed = tokio::task::spawn_blocking(move || {
                    let result = uploads.push(id, index, total, &text);
                    (uploads, result)
                })
                .await;
                let (returned, complete) = match pushed {
                    Ok(result) => result,
                    Err(error) => break (Some(format!("assembling a request: {error}")), false),
                };
                uploads = returned;
                match complete {
                    Ok(Some(complete)) => (Up::Ask { id, question: complete.question }, Some(complete.reservation)),
                    Ok(None) => continue,
                    Err(why) => {
                        let _ = out.send(Out::Frame(Down::Told { id, answer: Err(why) }));
                        continue;
                    }
                }
            }
            Up::CancelQuestion { id } => {
                uploads.cancel(id);
                continue;
            }
            frame => (frame, None),
        };
        let (handling, member_name, queue) = (hub.clone(), name.clone(), out.clone());
        let handled = tokio::task::spawn_blocking(move || {
            handle(&handling, &member_name, key, generation, &queue, frame, reservation)
        })
        .await;
        match handled {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => {
                let _ = out.send(Out::End(reason.clone()));
                break (Some(reason.to_string()), true);
            }
            Err(error) => break (Some(format!("handling a frame: {error}")), false),
        }
    };
    // The member counts as gone from this moment, and the calls waiting on
    // it are told so.
    let leaving = name.clone();
    hub.blocking(move |hub| close(&hub, &leaving, generation)).await;
    intake.abort();
    if refusing {
        // The writer returns once it has sent the refusal.
        let _ = writer.await;
        if transport.ended.borrow().is_none() {
            let _ = tokio::time::timeout(SILENCE, transport.ended.changed()).await;
        }
    } else {
        writer.abort();
    }
    out.shut();
    match ended {
        Some(reason) => hub.log.line(format_args!("{name} disconnected: {reason}")),
        None => hub.log.line(format_args!("{name} disconnected")),
    }
    hub.present(&name);
    // A machine meant to stay up that stopped answering is told of to those
    // who hold it (`crate::troubles`).
    if transport.ended.borrow().as_ref().is_some_and(|ended| ended.silent) {
        let stays_up = hub.store().indexed().by_key(&key).is_some_and(|member| member.managed)
            || crate::updates::reported_kind(&hub, &key) == Some(fastctx_types::update::Kind::Headless);
        hub.troubles.silent(&hub, &name, key, stays_up, SILENCE.as_secs());
    }
}

/// How many letters from a member may wait to be kept before its
/// connection is read no further.
const LETTERS_IN: usize = 1024;

/// How many durable letter receipts are combined before answering the sender.
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
    let mut last = None;
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
        let mut full = false;
        for (seq, message) in batch {
            // A letter the member passes over, sending what comes after it,
            // it holds no more: why the hub refused it was heard and kept.
            refusals_heard(&hub, &name, generation, last, seq.saturating_sub(1));
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
                }
                Ok(Kept::Refused(why)) => {
                    receipts.push((seq, Some(why)));
                }
                Ok(Kept::Again(refused)) => receipts.push((seq, refused)),
                Err(Stopped::Replaced) => return,
                Err(Stopped::Failed(reason)) => {
                    let _ = out.send(Out::End(reason));
                    return;
                }
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
fn refusals_heard(hub: &Hub, name: &MachineName, generation: u64, after: Option<u64>, through: u64) {
    if let Some(member) = hub.store().letters.member(name) {
        member.later(crate::post::Command::Heard { generation, after, through });
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
async fn open(
    hub: &Arc<Hub>,
    name: &MachineName,
    key: PublicKey,
    generation: u64,
    out: Outq,
    letters: Letters,
    peer: SocketAddr,
    said: Said,
) -> Result<(), Refusal> {
    let store = hub.store();
    if !said.any_names && crate::login::beyond_older_releases(&store) {
        return Err(Refusal::Broken { reason: crate::login::OLDER_RELEASE.into() });
    }
    let member = store.letters.member(name).ok_or(Refusal::NotMember)?;
    let versions = said.versions;
    let capabilities = (&said).into();
    let (registering, registered_name, registered_out) = (hub.clone(), name.clone(), out.clone());
    let register = Box::new(move |entry: &crate::store::Letters| -> Result<(), String> {
        let hub = registering;
        let name = &registered_name;
        let out = registered_out;
        let store = hub.edit_store();
        let current = store.metadata().indexed().by_key(&key).ok_or("the member was removed while connecting")?;
        if current.name.as_str() != name.as_str() || hub.moving.frozen() {
            return Err("the member or hub moved while connecting; reconnect with its current identity".into());
        }
        let now = entry.connected_at.expect("opened session");
        out.welcome(Down::Welcome {
            hub: store.hub.id.clone(),
            hub_name: store.hub.name.clone(),
            name: name.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            taken: counted_through(entry),
            letters: entry.out_epoch,
            afoot_pending: Some(said.afoot),
        });
        let session = Session {
            replicated_records: said.replicated_records,
            key,
            account_records: said.account_records,
            record_keys: said.record_keys.then(Default::default),
            skill_files: said.skill_files,
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
            strain: None,
            afoot: said.afoot,
            forgets: said.forgets,
            versions: said.versions,
            version_parts: said.version_parts,
            cancels: said.cancels,
            presence: said.presence,
            project_entry: said.project_entry,
            view_deltas: said.view_deltas,
        };
        let replaced = hub.sessions().insert(name.clone(), session);
        drop(store);
        if let Some(old) = replaced {
            let _ = old.out.send(Out::End(Refusal::Replaced));
            fail_calls_of(&hub, name, old.generation);
        }
        Ok(())
    });
    member
        .request(|reply| crate::post::Command::Open {
            generation,
            letters,
            capabilities,
            out: out.clone(),
            register,
            reply,
        })
        .await
        .map_err(|reason| Refusal::Broken { reason })?;
    crate::mailbox::opened(hub, name, generation);
    if versions {
        crate::updates::opened(hub, name);
    }
    Ok(())
}

pub(crate) fn close(hub: &Hub, name: &MachineName, generation: u64) {
    let removed = {
        let mut sessions = hub.sessions();
        if sessions.get(name).is_some_and(|session| session.generation == generation) {
            sessions.remove(name);
            true
        } else {
            false
        }
    };
    if removed {
        hub.forget_view(name, generation);
    }
    if let Some(member) = hub.store().letters.member(name) {
        member.later(crate::post::Command::Close { generation });
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
        let keys = calls.in_session(name, generation);
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
    frame: Up,
    reservation: Option<fastctx_net::reassembly::Reservation>,
) -> Result<(), Refusal> {
    if !hub.sessions().get(name).is_some_and(|session| session.generation == generation && session.key == key)
        || hub.store().indexed().by_key(&key).is_none_or(|member| member.name.as_str() != name.as_str())
    {
        return Err(Refusal::Replaced);
    }
    let origin = crate::origin::Origin { name: name.clone(), key };
    let reply = |frame: Down| {
        let _ = out.send(Out::Frame(frame));
    };
    match frame {
        Up::Ping { paths, held, .. } => {
            crate::assists::heard(hub);
            if let Some(held) = held {
                refusals_heard(hub, name, generation, None, held);
            }
            if let Some(paths) = paths {
                lines(hub, name, generation, paths);
            }
        }
        Up::Ask { id, question: Question::Manage { account, change: Change::Direct { machine, at } } } => {
            let (hub, out) = (hub.clone(), out.clone());
            let held = crate::moving::Moving::hold(&hub);
            tokio::spawn(async move {
                let _held = held;
                let answer = crate::direct::change(hub, origin, account, machine, at).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        // Letters are kept by the session's own task (`intake`).
        Up::Send { .. } => {}
        Up::Ack { seq } => crate::mailbox::taken(hub, name, generation, seq),
        Up::Call { id, message } => reply(Down::Status { id, status: call(hub, name, key, generation, id, message) }),
        Up::Answer { call, message } => answer(hub, name, generation, call, message),
        Up::Afoot { change } => crate::afoot::heard(hub, name, key, *change),
        Up::Forgot { id, outcome } => crate::forget::forgot(hub, name, id, outcome),
        Up::Versions { report } => crate::updates::heard(hub, name, generation, *report),
        Up::Unfetched { id, why } => crate::updates::unfetched(hub, name, id, why),
        Up::FetchProgress { id, progress } => crate::updates::progress(hub, name, generation, id, progress),
        Up::PackageDone { id } => crate::updates::done(hub, name, generation, id),
        Up::Ask { id, question: Question::Package { version, target, progress } } => {
            crate::updates::ask(hub, name, generation, id, version, target, progress, out.clone());
        }
        Up::Ask { id, question: Question::NetworkUpdates { account, request } } => {
            let held = crate::moving::Moving::hold(hub);
            let (hub, out) = (hub.clone(), out.clone());
            tokio::spawn(async move {
                let _held = held;
                let answer = crate::updates::view_request(&hub, &origin, &account, request).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: Question::PrepareUpdate { machine, version } } => {
            let held = crate::moving::Moving::hold(hub);
            let (hub, out) = (hub.clone(), out.clone());
            tokio::spawn(async move {
                let _held = held;
                let answer = crate::updates::prepare_update(&hub, &machine, &version).await.map(|()| Told::Done);
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask {
            id,
            question:
                question @ (Question::HistoryCatalog { .. }
                | Question::HistoryRead { .. }
                | Question::HistorySubmitted { .. }),
        } => {
            let (hub, out) = (hub.clone(), out.clone());
            tokio::spawn(async move {
                let _reservation = reservation;
                let answer = crate::ledger::history_requests::ask(&hub, &origin, question).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: question @ (Question::ProjectIndex { .. } | Question::MemoryRead { .. }) } => {
            crate::ledger::requests::ask(hub, &origin, out, id, question);
        }
        Up::Ask {
            id,
            question: Question::MemoryWrite { town, account, entry } | Question::HistoryWrite { town, account, entry },
        } => {
            let (hub, writer, out) = (hub.clone(), name.clone(), out.clone());
            let held = crate::moving::Moving::hold(&hub);
            tokio::spawn(async move {
                let (_held, _reservation) = (held, reservation);
                let answer = crate::ledger::write_as(&hub, &writer, town, entry, Some(account)).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: Question::LedgerProjection { town, after } } => {
            crate::ledger::projection(hub, &origin, out, id, town, after);
        }
        Up::Ask { id, question: Question::LedgerRead { town, after } } => {
            crate::ledger::ask(hub, &origin, out, id, town, after);
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
        Up::Ask { id, question: Question::OwnedRecords { account, request } } => {
            let (hub, from, out) = (hub.clone(), name.clone(), out.clone());
            let held = crate::moving::Moving::hold(&hub);
            tokio::spawn(async move {
                let _held = held;
                let _reservation = reservation;
                let answer = crate::owned_records::ask(&hub, from, key, account, request).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
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
                let _reservation = reservation;
                let answer = crate::account_records::ask(&hub, from, key, account, opened, request).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: Question::PlanCopies { account, request } } => {
            let opened = request.validate().and_then(|()| {
                crate::account_records::begin(
                    hub,
                    name,
                    &key,
                    &account,
                    &fastctx_types::account_records::Request::Read {
                        kind: fastctx_types::plan::KIND.into(),
                        key: request.owner().resident.to_string(),
                    },
                )
            });
            let opened = match opened {
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
                let answer = crate::plan_copies::ask(&hub, from, key, account, opened, request).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: Question::SkillFiles { account, request } } => {
            let opened = request.validate().and_then(|()| {
                crate::account_records::begin(
                    hub,
                    name,
                    &key,
                    &account,
                    &fastctx_types::account_records::Request::Read {
                        kind: fastctx_types::skills::KIND.into(),
                        key: request.name().unwrap_or("index").into(),
                    },
                )
            });
            let opened = match opened {
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
                let _reservation = reservation;
                let answer = crate::skill_files::ask(&hub, from, key, account, opened, request).await;
                let _ = out.send(Out::Frame(Down::Told { id, answer }));
            });
        }
        Up::Ask { id, question: Question::Manage { account, change } } => {
            let (answer, ended) = crate::manage::change(hub, &origin, &account, change);
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
            let view = matches!(question, Question::Directory);
            let answer = ask(hub, name, key, generation, question);
            let told = view && answer.is_ok();
            reply(Down::Told { id, answer });
            if told {
                crate::mailbox::view_told(hub, name, generation);
            }
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
fn own(store: &crate::store::Snapshot, name: &MachineName, message: &Envelope) -> Result<(), String> {
    let directory = store.indexed();
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
    hub.store().indexed().canonical(name).clone()
}

/// `address` with its machine named as it is now.
fn canonical_address(hub: &Hub, address: fastctx_types::Address) -> fastctx_types::Address {
    fastctx_types::Address { machine: canonical(hub, &address.machine), resident: address.resident }
}

/// The standing a call or a letter names, if the directory allows it: of
/// the machines as they are named now.
fn standing(hub: &Hub, store: &crate::store::Snapshot, message: &Envelope) -> Result<Standing, String> {
    let (account, purpose) = message.standing()?;
    let directory = store.indexed();
    let from = directory.member(&message.from).ok_or("the sender is not a member of the hub")?;
    let to = directory.member(&message.to).ok_or("the receiver is not a member of the hub")?;
    let (from_key, to_key) = (from.key, to.key);
    let (from, to) = (from.name.clone(), to.name.clone());
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
    let standing = Standing { from, from_key, to_key, account: account.clone(), to, purpose, project_entry, plan };
    match standing.lapsed_in(hub, store) {
        Some(why) => Err(why),
        None => Ok(standing),
    }
}

/// Keeps the letter `seq` from `name`, or refuses it: what the sender is to
/// be told, and its count once this letter is counted, which is kept before
/// it is told (`intake`). The letter is on disk before it counts: a sender
/// told the hub holds one only once the hub does.
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
    let store = hub.store();
    let member = store.letters.member(name).ok_or(Stopped::Replaced)?;
    if member.generation.load(std::sync::atomic::Ordering::Acquire) != generation {
        return Err(Stopped::Replaced);
    }
    if let Some(entry) = member.record.load_full()
        && seq <= entry.taken
    {
        return Ok(Kept::Again(entry.refused.get(&seq).cloned()));
    }
    let failed = |error: std::io::Error| Stopped::Failed(storage_failure(hub, "keeping a letter", &error));
    let (routing, sender) = (hub.clone(), name.clone());
    let (message, routed) = tokio::task::spawn_blocking(move || {
        let route = || {
            own(&store, &sender, &message)?;
            if message.kind != Kind::Letter {
                return Err("only letters are kept for later".to_string());
            }
            if store.indexed().member(&message.to).is_none() {
                return Err(format!("{} is not a member of the hub", message.to));
            }
            match standing(&routing, &store, &message) {
                Ok(_) => {}
                Err(_) if echoes_work(&store, &message) => {}
                Err(reason) => return Err(reason),
            }
            let to = store.indexed().canonical(&message.to);
            store.letters.member(to).ok_or_else(|| "the receiver was removed".to_string())
        };
        let routed = route();
        (message, routed)
    })
    .await
    .map_err(|e| failed(std::io::Error::other(e.to_string())))?;
    let refused = match routed {
        Ok(receiver) => {
            let held = crate::moving::Moving::hold(hub);
            let written = tokio::spawn(async move {
                let _held = held;
                receiver.request(|reply| crate::post::Command::Put { message: Box::new(message), reply }).await
            })
            .await
            .map_err(|e| failed(std::io::Error::other(e.to_string())))?;
            written.map_err(|e| failed(std::io::Error::other(e)))?;
            None
        }
        Err(reason) => Some(reason),
    };
    member
        .request(|reply| crate::post::Command::Count { generation, seq, refused: refused.clone(), reply })
        .await
        .map_err(|e| failed(std::io::Error::other(e)))?;
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
fn echoes_work(store: &crate::store::Snapshot, message: &Envelope) -> bool {
    let (Ok((account, Purpose::Echo)), Ok(body)) =
        (message.standing(), message.body::<fastctx_net::message::LetterBody>())
    else {
        return false;
    };
    let fastctx_types::remote::Letter::Notice { to, .. } = body.letter else { return false };
    let directory = store.indexed();
    let (from, machine) = (directory.canonical(&message.from).clone(), directory.canonical(&message.to).clone());
    let resident = fastctx_types::Address { machine, resident: to };
    store.book.directory.admits_echo(&message.from, account, &message.to).is_ok()
        && store.worked.contains(&(from, resident))
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
        let mut store = hub.edit_store();
        if let Some(why) = standing.lapsed_in(hub, &store) {
            return Err(why);
        }
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

fn call(
    hub: &Hub,
    name: &MachineName,
    member_key: PublicKey,
    generation: u64,
    id: u64,
    message: Envelope,
) -> CallStatus {
    let store = hub.store();
    if store.indexed().member(name).is_none_or(|member| member.key != member_key) {
        return CallStatus::Refused { reason: "the caller was removed or replaced".into() };
    }
    if let Err(reason) = own(&store, name, &message) {
        return CallStatus::Refused { reason };
    }
    if message.kind != Kind::Call {
        return CallStatus::Refused { reason: format!("a {:?} is not a call", message.kind) };
    }
    let to = store.indexed().canonical(&message.to).clone();
    let Some(target_key) = store.indexed().member(&to).map(|member| member.key) else { return CallStatus::NotMember };
    let last_seen = store.letters.get(&to).and_then(|entry| entry.last_seen);
    let standing = match standing(hub, &store, &message) {
        Ok(standing) => standing,
        Err(reason) => return CallStatus::Refused { reason },
    };
    let target = hub
        .sessions()
        .get(&to)
        .filter(|session| session.key == target_key)
        .map(|session| (session.generation, session.out.clone(), session.call_details));
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
        && let Err(error) = hub.store().worked.note(&to, resident)
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

fn ask(
    hub: &Arc<Hub>,
    name: &MachineName,
    public: PublicKey,
    generation: u64,
    question: Question,
) -> Result<Told, String> {
    let origin = crate::origin::Origin { name: name.clone(), key: public };
    match question {
        Question::HistorySubmitted { .. } => Err("history reads are dispatched asynchronously".into()),
        Question::RecordWatching { account, record_kind, watch, following } => {
            crate::account_records::begin(
                hub,
                name,
                &public,
                &account,
                &fastctx_types::account_records::Request::List { kind: record_kind.clone() },
            )?;
            crate::record_subscriptions::watch(hub, name, &public, &account, &record_kind, &watch, following)?;
            Ok(Told::Done)
        }
        Question::RecordFollowing { account, record_kind, key, following } => {
            crate::account_records::begin(
                hub,
                name,
                &public,
                &account,
                &fastctx_types::account_records::Request::Read { kind: record_kind.clone(), key: key.clone() },
            )?;
            if following {
                crate::record_subscriptions::follow(hub, name, &public, &account, &record_kind, &key)?;
            } else {
                crate::record_subscriptions::deleted(hub, name, &account, &record_kind, &key);
            }
            Ok(Told::Done)
        }
        Question::OwnedRecords { .. }
        | Question::AccountRecords { .. }
        | Question::PlanCopies { .. }
        | Question::SkillFiles { .. } => Err("account records are dispatched asynchronously".into()),
        Question::NetworkUpdates { .. } => Err("update management is dispatched asynchronously".into()),
        Question::MachineLog { account, before } => {
            let store = hub.store();
            origin.check(&store.book.directory)?;
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
        Question::Directory => Ok(Told::Directory { view: Box::new(hub.view_for(name, public, generation)?) }),
        Question::ProjectDeleted { town } => crate::project_deletions::finished(hub, name, &town),
        Question::Manage { .. } => Err("changes are made as they come".into()),
        Question::LedgerWrite { .. } | Question::MemoryWrite { .. } | Question::HistoryWrite { .. } => {
            Err("ledger entries are written as they come".into())
        }
        Question::ProjectIndex { .. }
        | Question::MemoryRead { .. }
        | Question::HistoryRead { .. }
        | Question::HistoryCatalog { .. } => Err("project reads are dispatched asynchronously".into()),
        Question::LedgerTowns => {
            let store = hub.store();
            origin.check(&store.book.directory)?;
            let towns = hub.ledgers.lock().expect("never poisoned").towns_of(&store.book.directory, name);
            Ok(Told::Towns { towns })
        }
        Question::LedgerCatalog => {
            let store = hub.store();
            origin.check(&store.book.directory)?;
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
        Question::LedgerRead { .. } | Question::LedgerProjection { .. } => {
            Err("ledger reads are dispatched asynchronously".into())
        }
        Question::Worked { to, account, purpose, resident } => {
            // What the hub notes of a call it passes on, for one made over
            // a direct link: only while that call is allowed.
            let mut store = hub.edit_store();
            origin.check(&store.book.directory)?;
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
