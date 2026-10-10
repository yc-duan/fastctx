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
use fastctx_net::wire::{self, CallStatus, Down, Letters, Question, Refusal, SILENCE, Told, Up};
use fastctx_types::access::Purpose;
use fastctx_types::remote::RemoteCall;
use fastctx_types::{AccountName, AgentName, MachineName};
use futures_util::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::{Hub, Out, Session, Socket, refuse, storage_failure};

/// What a call or a data connection was allowed under: `from` saying it to
/// `to` for `account`, for `purpose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Standing {
    pub from: MachineName,
    pub account: AccountName,
    pub to: MachineName,
    pub purpose: Purpose,
}

impl Standing {
    /// Why it no longer holds, if it does not.
    pub fn lapsed(&self, hub: &Hub) -> Option<String> {
        let store = hub.store();
        store.book.directory.admits(&self.from, &self.account, &self.to, self.purpose).err()
    }
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
    through: Option<String>,
) {
    let name = match hub.member(&key, &proof, &binding) {
        Ok(name) => name,
        Err(reason) => {
            hub.log.line(format_args!("{peer}: refused a session: {reason}"));
            refuse(&mut socket, reason).await;
            return;
        }
    };
    let generation = hub.next();
    let (out, mut outgoing) = mpsc::unbounded_channel();
    if let Err(reason) = open(&hub, &name, generation, out.clone(), letters, through.clone()) {
        refuse(&mut socket, reason).await;
        return;
    }
    match &through {
        Some(relay) => hub.log.line(format_args!("{name} connected from {peer} through the relay {relay}")),
        None => hub.log.line(format_args!("{name} connected from {peer}")),
    }
    // Whoever sees it sees it online now.
    hub.changed();

    let (mut sink, mut stream) = socket.split();
    let (stopped, mut writer_stopped) = tokio::sync::oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        // Dropped when the writer returns, which the reader notices.
        let _stopped = stopped;
        while let Some(out) = outgoing.recv().await {
            let (frame, last) = match out {
                Out::Frame(frame) => (frame, false),
                Out::End(reason) => (Down::Refused { reason }, true),
            };
            if sink.send(WsMessage::Binary(wire::encode(&frame).into())).await.is_err() {
                return;
            }
            if last {
                let _ = sink.close().await;
                return;
            }
        }
    });

    // Whether the writer still has a refusal to deliver before closing.
    let (ended, refusing) = loop {
        let next = tokio::select! {
            next = tokio::time::timeout(SILENCE, stream.next()) => next,
            // The writer stopped: the connection was replaced or ended.
            _ = &mut writer_stopped => break (None, false),
        };
        let frame = match next {
            Err(_) => {
                break (Some(format!("nothing was heard from it for {} s", SILENCE.as_secs())), false);
            }
            Ok(None | Some(Ok(WsMessage::Close(_)))) => break (None, false),
            Ok(Some(Err(error))) => break (Some(fastctx_net::dial::describe(&error)), false),
            Ok(Some(Ok(WsMessage::Binary(bytes)))) => match serde_json::from_slice::<Up>(&bytes) {
                Ok(frame) => frame,
                Err(error) => break (Some(format!("an unreadable frame: {error}")), false),
            },
            Ok(Some(Ok(_))) => continue,
        };
        if let Err(reason) = handle(&hub, &name, generation, &out, frame) {
            let _ = out.send(Out::End(reason.clone()));
            break (Some(reason.to_string()), true);
        }
    };
    // The member counts as gone from this moment, and the calls waiting on
    // it are told so.
    close(&hub, &name, generation);
    drop(out);
    if refusing {
        // The writer returns once it has sent the refusal.
        let _ = writer.await;
    } else {
        writer.abort();
    }
    match ended {
        Some(reason) => hub.log.line(format_args!("{name} disconnected: {reason}")),
        None => hub.log.line(format_args!("{name} disconnected")),
    }
    hub.changed();
}

/// Registers the session, replacing an older one of the same member, and
/// queues the welcome and every letter waiting for the member, in order.
fn open(
    hub: &Hub,
    name: &MachineName,
    generation: u64,
    out: mpsc::UnboundedSender<Out>,
    letters: Letters,
    through: Option<String>,
) -> Result<(), Refusal> {
    // The store's lock is held while the session is registered, so a letter
    // kept meanwhile is either in the backlog below or pushed after it.
    let mut store = hub.store();
    let (world, hub_name) = (store.hub.id.clone(), store.hub.name.clone());
    let entry = store.letters_of(name).map_err(|error| storage_failure(hub, "making letter records", &error))?;
    if entry.epoch != letters.epoch {
        // The machine numbers its letters afresh, having lost its records.
        entry.epoch = letters.epoch;
        entry.taken = 0;
    }
    let taken_by_member = match letters.taken {
        Some((epoch, taken)) if epoch == entry.out_epoch => taken,
        _ => 0,
    };
    let now = Timestamp::now();
    entry.connected_at = Some(now);
    let welcome = Down::Welcome {
        hub: world,
        hub_name,
        name: name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        taken: entry.taken,
        letters: entry.out_epoch,
    };
    store.save_letters().map_err(|error| storage_failure(hub, "recording the connection", &error))?;
    store
        .letters_taken(name, taken_by_member)
        .map_err(|error| storage_failure(hub, "dropping taken letters", &error))?;
    let backlog =
        store.letters_after(name, taken_by_member).map_err(|error| storage_failure(hub, "reading letters", &error))?;
    let _ = out.send(Out::Frame(welcome));
    for (seq, message) in backlog {
        let _ = out.send(Out::Frame(Down::Deliver { seq, message }));
    }
    let replaced = hub.sessions().insert(name.clone(), Session { generation, out, since: now, through });
    drop(store);
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
            drop(sessions);
            if let Some(entry) = store.letters.get_mut(name) {
                entry.connected_at = None;
                entry.last_seen = Some(Timestamp::now());
            }
            if let Err(error) = store.save_letters() {
                hub.log.line(format_args!("recording that {name} left: {error}"));
            }
        }
    }
    // Data connections it registered and nobody paired wait no longer; a
    // session that replaced this one keeps its own.
    crate::data::member_left(hub, name, generation);
    fail_calls_of(hub, name, generation);
}

/// Ends every call this session made or was to answer.
fn fail_calls_of(hub: &Hub, name: &MachineName, generation: u64) {
    let ended: Vec<Pending> = {
        let mut calls = hub.calls.lock().expect("never poisoned");
        let keys: Vec<u64> = calls
            .iter()
            .filter(|(_, pending)| {
                (pending.target == *name && pending.target_generation == generation)
                    || (pending.caller == *name && pending.caller_generation == generation)
            })
            .map(|(key, _)| *key)
            .collect();
        keys.into_iter().filter_map(|key| calls.remove(&key)).collect()
    };
    for pending in ended {
        if pending.target == *name {
            hub.tell(&pending.caller, Down::Status { id: pending.id, status: CallStatus::Lost });
        }
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
    let keys: Vec<(u64, String)> = {
        let store = hub.store();
        standings
            .into_iter()
            .filter_map(|(key, standing)| {
                let directory = &store.book.directory;
                directory
                    .admits(&standing.from, &standing.account, &standing.to, standing.purpose)
                    .err()
                    .map(|why| (key, why))
            })
            .collect()
    };
    let lapsed: Vec<(Pending, String)> = {
        let mut calls = hub.calls.lock().expect("never poisoned");
        keys.into_iter().filter_map(|(key, why)| calls.remove(&key).map(|pending| (pending, why))).collect()
    };
    for (pending, why) in lapsed {
        hub.log.line(format_args!("cut a call from {} to {}: {why}", pending.caller, pending.target));
        let status = CallStatus::Cut { reason: format!("the right it was made under was taken back: {why}") };
        hub.tell(&pending.caller, Down::Status { id: pending.id, status });
    }
}

fn handle(
    hub: &Hub,
    name: &MachineName,
    generation: u64,
    out: &mpsc::UnboundedSender<Out>,
    frame: Up,
) -> Result<(), Refusal> {
    let reply = |frame: Down| {
        let _ = out.send(Out::Frame(frame));
    };
    match frame {
        Up::Ping { n } => reply(Down::Pong { n }),
        Up::Send { seq, message } => letter(hub, name, seq, message, &reply)?,
        Up::Ack { seq } => {
            let store = hub.store();
            store.letters_taken(name, seq).map_err(|error| storage_failure(hub, "dropping taken letters", &error))?;
        }
        Up::Call { id, message } => reply(Down::Status { id, status: call(hub, name, generation, id, message) }),
        Up::Answer { call, message } => answer(hub, name, generation, call, message),
        Up::Cancel { id } => {
            hub.calls
                .lock()
                .expect("never poisoned")
                .retain(|_, pending| !(pending.caller == *name && pending.id == id));
        }
        Up::Ask { id, question: Question::KernelRelease { version } } => {
            let releases = hub.releases.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let read = releases.clone();
                let answer = match tokio::task::spawn_blocking(move || read.ready(&version)).await {
                    Ok(Ok(release)) => Ok(Told::KernelRelease { release, certificate: releases.certificate.clone() }),
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
/// and for this hub.
fn own(hub: &Hub, name: &MachineName, message: &Envelope) -> Result<(), String> {
    if message.from != *name {
        return Err(format!("it says it is from {}, on the connection of {name}", message.from));
    }
    let world = hub.store().hub.id.clone();
    if message.hub != world {
        return Err(format!("it is for the hub {}", message.hub));
    }
    if message.to == *name {
        return Err("a machine's own calls and letters do not go through the hub".to_string());
    }
    Ok(())
}

/// The standing a call or a letter names, if the directory allows it.
fn standing(hub: &Hub, message: &Envelope) -> Result<Standing, String> {
    let (account, purpose) = message.standing()?;
    let standing = Standing { from: message.from.clone(), account: account.clone(), to: message.to.clone(), purpose };
    match standing.lapsed(hub) {
        Some(why) => Err(why),
        None => Ok(standing),
    }
}

fn letter(hub: &Hub, name: &MachineName, seq: u64, message: Envelope, reply: &impl Fn(Down)) -> Result<(), Refusal> {
    let taken = hub.store().letters.get(name).map(|entry| entry.taken).unwrap_or(0);
    if seq <= taken {
        // Sent again after a reconnect; the hub holds it already.
        reply(Down::Ack { seq, refused: None });
        return Ok(());
    }
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
            let to = message.to.clone();
            let kept = {
                let mut store = hub.store();
                store.keep_letter(&to, &message).map_err(|error| storage_failure(hub, "keeping a letter", &error))?
            };
            hub.tell(&to, Down::Deliver { seq: kept, message });
            None
        }
        Err(reason) => Some(reason),
    };
    let mut store = hub.store();
    if let Some(entry) = store.letters.get_mut(name) {
        entry.taken = seq;
    }
    store.save_letters().map_err(|error| storage_failure(hub, "counting a letter", &error))?;
    drop(store);
    if let Some(reason) = &refused {
        hub.log.line(format_args!("refused letter {seq} from {name}: {reason}"));
    }
    reply(Down::Ack { seq, refused });
    Ok(())
}

/// The resident a call asks the machine it goes to to work for, when it
/// names one: a hop of an `on`, a run handed over to where its resident
/// works, a colleague asked, a file placed.
fn worked_for(message: &Envelope) -> Option<fastctx_types::Address> {
    let call = match message.body::<fastctx_net::message::Body>().ok()? {
        fastctx_net::message::Body::Call { call } | fastctx_net::message::Body::StreamCall { call } => call,
        _ => return None,
    };
    let from = |resident| fastctx_types::Address { machine: message.from.clone(), resident };
    match call {
        RemoteCall::On { origin, .. } | RemoteCall::Follow { origin, .. } => Some(origin.resident),
        RemoteCall::Run { resident, .. } | RemoteCall::Place { resident, .. } => Some(from(resident)),
        RemoteCall::Ask { asker, .. } => Some(from(asker)),
        _ => None,
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
    let resident = fastctx_types::Address { machine: message.to.clone(), resident: to };
    store.book.directory.admits_echo(&message.from, account, &message.to).is_ok()
        && store.worked.contains(&(message.from.clone(), resident))
}

/// The agent's login a call starts a session of, if it starts one: a
/// session opened for the user, or a colleague asked; none is the agent's
/// default login there.
fn login_asked(message: &Envelope) -> Option<(AgentName, Option<std::path::PathBuf>)> {
    let call = match message.body::<fastctx_net::message::Body>().ok()? {
        fastctx_net::message::Body::Call { call } | fastctx_net::message::Body::StreamCall { call } => call,
        _ => return None,
    };
    match call {
        RemoteCall::Open { agent, home, .. } => Some((agent, home)),
        RemoteCall::Ask { home, .. } => Some(("codex".parse().expect("an agent name"), home)),
        _ => None,
    }
}

/// Checks that a call starting a session may use that agent's login on the
/// machine it goes to, and notes that machine acts for the account from now
/// on when it did not: an agent of another's machine that works for an
/// account has to reach the account's machines, and using it is the
/// account's word for that (`world-network.md`, 一台机器什么时候能代一个
/// 账号访问别的机器). Being shared a machine or lent a login does not do it.
fn use_agent(hub: &Hub, message: &Envelope, standing: &Standing) -> Result<(), String> {
    let Some((agent, home)) = login_asked(message) else { return Ok(()) };
    let (account, to) = (&standing.account, &standing.to);
    let noted = {
        let mut store = hub.store();
        let directory = &store.book.directory;
        if !directory.may_use_login(account, to, &agent, home.as_deref()) {
            let login = home.as_ref().map(|home| home.display().to_string()).unwrap_or_else(|| "default".to_string());
            return Err(format!("the account {account} may not use the {agent} login {login} on {to}"));
        }
        if directory.trusts(account, to) {
            false
        } else {
            store.book.directory.acting.push(fastctx_net::directory::ActsFor {
                machine: to.clone(),
                account: account.clone(),
                since: Timestamp::now(),
            });
            store
                .save_book()
                .map_err(|error| format!("the hub could not keep that {to} acts for {account}: {error}"))?;
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
    let to = message.to.clone();
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
    if let Err(reason) = use_agent(hub, &message, &standing) {
        return CallStatus::Refused { reason };
    }
    if let Some(resident) = worked_for(&message)
        && let Err(error) = hub.store().note_worked(&to, resident)
    {
        hub.log.line(format_args!("noting that {to} works for a resident: {error}"));
    }
    let target = hub.sessions().get(&to).map(|session| (session.generation, session.out.clone()));
    let Some((target_generation, target)) = target else {
        return CallStatus::Offline { last_seen };
    };
    let key = hub.next();
    if let Some(ticket) = &message.stream {
        crate::data::register(hub, ticket, &standing, name, generation);
    }
    let pending = Pending {
        caller: name.clone(),
        caller_generation: generation,
        id,
        target: to.clone(),
        target_generation,
        standing,
    };
    hub.calls.lock().expect("never poisoned").insert(key, pending);
    if target.send(Out::Frame(Down::Incoming { call: key, message })).is_err() {
        hub.calls.lock().expect("never poisoned").remove(&key);
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
    let fits = message.from == *name && message.kind.is_answer() && message.to == pending.caller;
    if !fits {
        hub.log.line(format_args!("{name} answered call {call} with something that is not its answer"));
        hub.tell(&pending.caller, Down::Status { id: pending.id, status: CallStatus::Lost });
        return;
    }
    if let Some(ticket) = &message.stream {
        crate::data::register(hub, ticket, &pending.standing, name, generation);
    }
    let sessions = hub.sessions();
    if let Some(caller) =
        sessions.get(&pending.caller).filter(|session| session.generation == pending.caller_generation)
    {
        let _ = caller.out.send(Out::Frame(Down::Answered { id: pending.id, message }));
    }
}

fn ask(hub: &Hub, name: &MachineName, question: Question) -> Result<Told, String> {
    match question {
        Question::KernelRelease { .. } => Err("release reads are dispatched asynchronously".into()),
        Question::Directory => Ok(Told::Directory { view: Box::new(hub.view_for(name)) }),
        Question::Manage { account, change } => crate::manage::change(hub, name, &account, change),
        Question::LedgerWrite { town, entry } => {
            let (seq, tell) = {
                let store = hub.store();
                hub.ledgers.lock().expect("never poisoned").write(&store, name, &town, entry)?
            };
            hub.log.line(format_args!("{name} wrote entry {seq} of {town}"));
            for machine in tell {
                hub.tell(&machine, Down::LedgerChanged { town: town.clone() });
            }
            Ok(Told::Written { seq })
        }
        Question::LedgerTowns => Ok(Told::Towns { towns: hub.ledgers.lock().expect("never poisoned").towns_of(name) }),
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
        Question::LedgerRead { town, after } => {
            if !hub.store().book.directory.member_in_project(name, &town) {
                return Err(format!("{name} acts for no account that may read {town}"));
            }
            let (entries, more) = hub.ledgers.lock().expect("never poisoned").read(&town, after)?;
            Ok(Told::Entries { entries, more })
        }
    }
}
