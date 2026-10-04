//! A member's control connection: letters, calls, questions and the
//! heartbeat.

use std::net::SocketAddr;
use std::sync::Arc;

use fastctx_net::message::{Body, Message};
use fastctx_net::sign::{Domain, PublicKey, Signature, Signed};
use fastctx_net::trust::{Endorsement, Revocation};
use fastctx_net::wire::{self, CallStatus, Down, Letters, Presence, Question, Refusal, SILENCE, Told, Up};
use fastctx_types::MachineName;
use futures_util::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::store::Invitation;
use crate::{Hub, Out, Session, Socket, refuse, storage_failure};

/// A call on its way: who asked, and who is to answer.
pub(crate) struct Pending {
    caller: MachineName,
    caller_generation: u64,
    id: u64,
    target: MachineName,
    target_generation: u64,
}

pub(crate) async fn session(
    hub: Arc<Hub>,
    mut socket: Socket,
    peer: SocketAddr,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
    letters: Letters,
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
    if let Err(reason) = open(&hub, &name, generation, out.clone(), letters) {
        refuse(&mut socket, reason).await;
        return;
    }
    hub.log.line(format_args!("{name} connected from {peer}"));

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
}

/// Registers the session, replacing an older one of the same member, and
/// queues the welcome and every letter waiting for the member, in order.
fn open(
    hub: &Hub,
    name: &MachineName,
    generation: u64,
    out: mpsc::UnboundedSender<Out>,
    letters: Letters,
) -> Result<(), Refusal> {
    // The store's lock is held while the session is registered, so a letter
    // kept meanwhile is either in the backlog below or pushed after it.
    let mut store = hub.store();
    let world = store.hub.world.clone();
    let entry = store
        .letters
        .get_mut(name)
        .ok_or_else(|| Refusal::Broken { reason: format!("the hub has no letter records for {name}") })?;
    if entry.epoch != letters.epoch {
        // The machine numbers its letters afresh, having lost its records.
        entry.epoch = letters.epoch;
        entry.taken = 0;
    }
    let taken_by_member = match letters.taken {
        Some((epoch, taken)) if epoch == entry.out_epoch => taken,
        _ => 0,
    };
    entry.connected_at = Some(Timestamp::now());
    let welcome = Down::Welcome {
        world,
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
    let replaced = hub.sessions().insert(name.clone(), Session { generation, out });
    drop(store);
    if let Some(old) = replaced {
        let _ = old.out.send(Out::End(Refusal::Replaced));
        fail_calls_of(hub, name, old.generation);
    }
    Ok(())
}

fn close(hub: &Hub, name: &MachineName, generation: u64) {
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
    fail_calls_of(hub, name, generation);
    crate::data::member_left(hub, name);
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
    for (_, pending) in ended {
        if pending.target == *name {
            hub.tell(&pending.caller, Down::Status { id: pending.id, status: CallStatus::Lost });
        }
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
        Up::Send { seq, message } => letter(hub, name, seq, &message, &reply)?,
        Up::Ack { seq } => {
            let store = hub.store();
            store.letters_taken(name, seq).map_err(|error| storage_failure(hub, "dropping taken letters", &error))?;
        }
        Up::Call { id, message } => reply(Down::Status { id, status: call(hub, name, generation, id, &message) }),
        Up::Answer { call, message } => answer(hub, name, generation, call, message),
        Up::Cancel { id } => {
            hub.calls
                .lock()
                .expect("never poisoned")
                .retain(|_, pending| !(pending.caller == *name && pending.id == id));
        }
        Up::Ask { id, question } => {
            let answer = ask(hub, name, question);
            reply(Down::Told { id, answer });
        }
        Up::Hello { .. } => {
            return Err(Refusal::Broken { reason: "a second hello on one connection".to_string() });
        }
    }
    Ok(())
}

/// Opens a member's signed message, checking it came from the member on
/// this connection.
fn open_message(hub: &Hub, name: &MachineName, message: &Signed) -> Result<Message, String> {
    let claimed: Message = message.claim().map_err(|error| error.to_string())?;
    if claimed.from != *name {
        return Err(format!("it says it is from {}, on the connection of {name}", claimed.from));
    }
    let store = hub.store();
    if claimed.world != store.hub.world {
        return Err(format!("it is for the World {}", claimed.world));
    }
    let key = store.table().get(name).map(|member| member.key).ok_or_else(|| format!("{name} is not a member"))?;
    message.open(&key, Domain::Message).map_err(|error| error.to_string())
}

fn letter(hub: &Hub, name: &MachineName, seq: u64, message: &Signed, reply: &impl Fn(Down)) -> Result<(), Refusal> {
    let store = hub.store();
    let taken = store.letters.get(name).map(|entry| entry.taken).unwrap_or(0);
    if seq <= taken {
        // Sent again after a reconnect; the hub holds it already.
        reply(Down::Ack { seq, refused: None });
        return Ok(());
    }
    drop(store);
    let refused = match open_message(hub, name, message) {
        Ok(Message { to, body: Body::Letter { .. }, .. }) => {
            let mut store = hub.store();
            if to == *name {
                Some("a machine's letters to itself do not go through the hub".to_string())
            } else if store.table().get(&to).is_none() {
                Some(match store.table().revoked(&to) {
                    Some(revocation) => {
                        format!("{to} was revoked by {} at {}", revocation.by, revocation.at)
                    }
                    None => format!("{to} is not a member"),
                })
            } else {
                let kept = store
                    .keep_letter(&to, message)
                    .map_err(|error| storage_failure(hub, "keeping a letter", &error))?;
                hub.tell(&to, Down::Deliver { seq: kept, message: message.clone() });
                None
            }
        }
        Ok(_) => Some("only letters are kept for later".to_string()),
        Err(reason) => Some(reason),
    };
    let mut store = hub.store();
    if let Some(entry) = store.letters.get_mut(name) {
        entry.taken = seq;
    }
    store.save_letters().map_err(|error| storage_failure(hub, "counting a letter", &error))?;
    if let Some(reason) = &refused {
        hub.log.line(format_args!("refused letter {seq} from {name}: {reason}"));
    }
    reply(Down::Ack { seq, refused });
    Ok(())
}

fn call(hub: &Hub, name: &MachineName, generation: u64, id: u64, message: &Signed) -> CallStatus {
    let to = match open_message(hub, name, message) {
        Ok(Message { to, body: Body::Call { .. } | Body::LargeCall { .. } | Body::StreamCall { .. }, .. }) => to,
        // A caller that sends something unreadable gets the plainest
        // answer: nobody by that description is there.
        Ok(_) | Err(_) => return CallStatus::NotMember,
    };
    let store = hub.store();
    if store.table().get(&to).is_none() {
        return CallStatus::NotMember;
    }
    let last_seen = store.letters.get(&to).and_then(|entry| entry.last_seen);
    let sessions = hub.sessions();
    let Some(target) = sessions.get(&to) else {
        return CallStatus::Offline { last_seen };
    };
    let key = hub.next();
    let pending = Pending {
        caller: name.clone(),
        caller_generation: generation,
        id,
        target: to.clone(),
        target_generation: target.generation,
    };
    hub.calls.lock().expect("never poisoned").insert(key, pending);
    if target.out.send(Out::Frame(Down::Incoming { call: key, message: message.clone() })).is_err() {
        hub.calls.lock().expect("never poisoned").remove(&key);
        return CallStatus::Offline { last_seen: Some(Timestamp::now()) };
    }
    CallStatus::Delivered
}

fn answer(hub: &Hub, name: &MachineName, generation: u64, call: u64, message: Signed) {
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
    match open_message(hub, name, &message) {
        Ok(Message { to, body: Body::Answer { .. } | Body::LargeAnswer { .. }, .. }) if to == pending.caller => {
            let sessions = hub.sessions();
            if let Some(caller) =
                sessions.get(&pending.caller).filter(|session| session.generation == pending.caller_generation)
            {
                let _ = caller.out.send(Out::Frame(Down::Answered { id: pending.id, message }));
            }
        }
        Ok(_) | Err(_) => {
            hub.log.line(format_args!("{name} answered call {call} with something that is not its answer"));
            hub.tell(&pending.caller, Down::Status { id: pending.id, status: CallStatus::Lost });
        }
    }
}

fn ask(hub: &Hub, name: &MachineName, question: Question) -> Result<Told, String> {
    match question {
        Question::Members => {
            let store = hub.store();
            let sessions = hub.sessions();
            let presence = store
                .members
                .records
                .iter()
                .map(|admitted| {
                    let entry = store.letters.get(&admitted.name);
                    let online = sessions.contains_key(&admitted.name);
                    let since = entry.and_then(|entry| if online { entry.connected_at } else { entry.last_seen });
                    Presence { name: admitted.name.clone(), online, since }
                })
                .collect();
            Ok(Told::Members {
                records: store.members.records.iter().map(|admitted| admitted.record.clone()).collect(),
                revocations: store.members.revocations.clone(),
                presence,
            })
        }
        Question::Invite { endorsement } => invite(hub, name, endorsement).map(|()| Told::Done),
        Question::Revoke { revocation } => revoke(hub, name, revocation).map(|()| Told::Done),
    }
}

fn invite(hub: &Hub, name: &MachineName, signed: Signed) -> Result<(), String> {
    let mut store = hub.store();
    let key = store.table().get(name).map(|member| member.key).ok_or_else(|| format!("{name} is not a member"))?;
    let endorsement: Endorsement =
        signed.open(&key, Domain::Endorsement).map_err(|error| format!("the endorsement: {error}"))?;
    if endorsement.inviter != *name {
        return Err(format!("the endorsement is by {}, sent by {name}", endorsement.inviter));
    }
    if endorsement.world != store.hub.world {
        return Err(format!("the endorsement is for the World {}", endorsement.world));
    }
    if endorsement.expires <= Timestamp::now() {
        return Err("the invitation has expired already".to_string());
    }
    if store.invitation(&endorsement.invite).is_some() {
        return Err("that invitation key is registered already".to_string());
    }
    store.members.invitations.push(Invitation {
        endorsement: signed,
        invite: endorsement.invite,
        inviter: name.clone(),
        expires: endorsement.expires,
        used_by: None,
    });
    store.save_members().map_err(|error| format!("the hub could not store the invitation: {error}"))?;
    hub.log.line(format_args!("{name} made an invitation, valid until {}", endorsement.expires));
    Ok(())
}

fn revoke(hub: &Hub, name: &MachineName, signed: Signed) -> Result<(), String> {
    let revoked = {
        let mut store = hub.store();
        let key = store.table().get(name).map(|member| member.key).ok_or_else(|| format!("{name} is not a member"))?;
        let revocation: Revocation =
            signed.open(&key, Domain::Revocation).map_err(|error| format!("the revocation: {error}"))?;
        if revocation.by != *name {
            return Err(format!("the revocation is by {}, sent by {name}", revocation.by));
        }
        if revocation.world != store.hub.world {
            return Err(format!("the revocation is for the World {}", revocation.world));
        }
        let member = store.table().get(&revocation.name).cloned();
        match member {
            Some(member) if member.key == revocation.key => {}
            Some(_) => return Err(format!("{} has another key", revocation.name)),
            None => return Err(format!("{} is not a member", revocation.name)),
        }
        store.members.revocations.push(signed);
        store.save_members().map_err(|error| format!("the hub could not store the revocation: {error}"))?;
        if let Err(error) = store.drop_letters(&revocation.name) {
            hub.log.line(format_args!("dropping the letters for {}: {error}", revocation.name));
        }
        revocation
    };
    hub.log.line(format_args!("{} revoked {}: {}", revoked.by, revoked.name, revoked.reason));
    let sessions = hub.sessions();
    if let Some(session) = sessions.get(&revoked.name) {
        let _ = session.out.send(Out::End(Refusal::Revoked {
            by: revoked.by.clone(),
            at: revoked.at,
            reason: revoked.reason.clone(),
        }));
    }
    for session in sessions.values() {
        let _ = session.out.send(Out::Frame(Down::MembersChanged));
    }
    Ok(())
}
