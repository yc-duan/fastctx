//! Answering other members: their handshakes and their calls.
//!
//! Every envelope is checked before anything else: that it is for this
//! machine, that a member this machine trusts signed it, and that its
//! number was not taken before. A handshake starts a session the caller
//! holds with this machine; a call comes on such a session, and one that
//! comes on a session this machine does not have, as after a restart, is
//! answered with just that, unopened, so the caller starts a new session
//! and calls again. A call that does not check out is refused in a short
//! signed word that names the reason and nothing of the call.

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteResult};

use super::Inner;
use super::outgoing::Outgoing;
use crate::data::Stream;
use crate::envelope::{self, Envelope, Kind};
use crate::message::Body;
use crate::noise::{self, Keys};
use crate::sign::{Bytes, Domain, Signed, sha256};
use crate::stream_crypto::StreamKey;
use crate::wire::{self, Carried, End, Ticket, Up};

/// Checks an envelope from another machine, and does what it asks.
pub(super) async fn answer(inner: Arc<Inner>, up: Outgoing, call: u64, message: Signed) {
    let opened = match check(&inner, &message).await {
        Ok(opened) => opened,
        Err(Refusal::Silent(reason)) => {
            inner.node.log(format!("dropped what came through the hub as a call: {reason}"));
            return;
        }
        Err(Refusal::Said { caller, n, reason }) => {
            inner.node.log(format!("refused a call from {caller}: {reason}"));
            reply(&inner, &up, call, &caller, n, Kind::Refused, None, reason.into_bytes()).await;
            return;
        }
    };
    match opened.kind {
        Kind::Handshake => handshake(&inner, &up, call, &opened).await,
        Kind::Call => match opened.session.and_then(|session| inner.sessions.answering(&opened.from, session)) {
            Some(keys) => answer_call(&inner, &up, call, &opened, &keys).await,
            None => {
                reply(&inner, &up, call, &opened.from, opened.n, Kind::UnknownSession, opened.session, Vec::new()).await
            }
        },
        _ => unreachable!("only calls and handshakes pass the check"),
    }
}

enum Refusal {
    /// Nobody to answer: what came is not for this machine, or not from a
    /// member it can name.
    Silent(String),
    /// A word back to the member that sent it.
    Said { caller: fastctx_types::MachineName, n: u64, reason: String },
}

async fn check(inner: &Inner, message: &Signed) -> Result<Envelope, Refusal> {
    let claimed = envelope::claim(message).map_err(Refusal::Silent)?;
    if claimed.to != inner.membership.name || claimed.world != inner.membership.world {
        return Err(Refusal::Silent(format!("it is for {} in {}", claimed.to, claimed.world)));
    }
    let Some(key) = inner.key_of(&claimed.from).await else {
        return Err(Refusal::Silent(format!("{} is not a member this machine trusts", claimed.from)));
    };
    let opened = envelope::open(message, &key).map_err(Refusal::Silent)?;
    let said = |reason: String| Refusal::Said { caller: opened.from.clone(), n: opened.n, reason };
    if !opened.kind.is_call() {
        return Err(said(format!("a {:?} is not a call", opened.kind)));
    }
    match inner.taken.take(&opened.from, opened.n) {
        Ok(Ok(())) => Ok(opened),
        Ok(Err(reason)) => Err(said(reason)),
        Err(error) => Err(said(format!("recording the call: {error}"))),
    }
}

async fn handshake(inner: &Inner, up: &Outgoing, call: u64, opened: &Envelope) {
    let caller = &opened.from;
    let refuse = |reason: String| async move {
        inner.node.log(format!("refused a session with {caller}: {reason}"));
        reply(inner, up, call, caller, opened.n, Kind::Refused, None, reason.into_bytes()).await;
    };
    let Some(session) = opened.session else {
        return refuse("the handshake names no session".to_string()).await;
    };
    let Some(seal) = inner.members().table.get(caller).and_then(|member| member.seal) else {
        return refuse(format!("{caller} has no sealing key this machine knows")).await;
    };
    let responded = noise::respond(
        &inner.membership.world,
        caller,
        &seal,
        &inner.membership.name,
        &inner.seal,
        session,
        &opened.sealed.0,
    );
    match responded {
        Ok((keys, finish)) => {
            inner.sessions.answer(caller, keys);
            reply(inner, up, call, caller, opened.n, Kind::HandshakeReply, Some(session), finish).await;
        }
        Err(error) => refuse(error.to_string()).await,
    }
}

async fn answer_call(inner: &Arc<Inner>, up: &Outgoing, call: u64, opened: &Envelope, keys: &Arc<Keys>) {
    let caller = opened.from.clone();
    let mut _answering = None;
    let result: RemoteResult = match open_call(inner, opened, keys).await {
        Ok((remote, stream)) => match inner.node.answering(&remote) {
            Ok(guard) => {
                _answering = guard;
                inner.node.call(caller.clone(), remote, stream).await
            }
            Err(failure) => Err(failure),
        },
        Err(reason) => {
            inner.node.log(format!("refused a call from {caller}: {reason}"));
            Err(Failure::new(format!("{} refused the call: {reason}", inner.membership.name)))
        }
    };
    let small = Body::Answer { result: result.clone() };
    let Some(sealed) = sealed_answer(inner, &caller, opened, keys, &small) else { return };
    let frame = Up::Answer { call, message: sealed.clone() };
    if wire::encode(&frame).len() <= wire::MAX_CONTROL_FRAME {
        if let Err(error) = up.write(frame).await {
            inner.node.log(format!("sending an answer to {caller}: {error}"));
        }
        return;
    }
    // Too large for the control connection: the answer says what follows
    // over a data connection.
    let bytes = serde_json::to_vec(&result).expect("answers serialize");
    let (stream, key) = match (crate::sign::random::<8>(), StreamKey::generate()) {
        (Ok(random), Ok(key)) => (u64::from_le_bytes(random), key),
        (Err(error), _) | (_, Err(error)) => {
            inner.node.log(format!("choosing a stream for a large answer: {error}"));
            return;
        }
    };
    let ticket = Ticket {
        world: inner.membership.world.clone(),
        stream,
        source: inner.membership.name.clone(),
        sink: caller.clone(),
    };
    let ticket = Signed::new(&inner.key, Domain::Ticket, &ticket);
    let carried = Carried {
        ticket: ticket.clone(),
        key: key.clone(),
        bytes: bytes.len() as u64,
        sha256: crate::sign::Digest(sha256(&bytes)),
    };
    let Some(sealed) = sealed_answer(inner, &caller, opened, keys, &Body::LargeAnswer { carried }) else { return };
    if let Err(error) = up.write(Up::Answer { call, message: sealed }).await {
        inner.node.log(format!("announcing a large answer to {caller}: {error}"));
        return;
    }
    if let Err(error) = crate::data::source(&inner.membership, &inner.key, ticket, &key, &bytes).await {
        inner.node.log(format!("sending a large answer to {caller}: {error}"));
    }
}

/// The answer to `opened`, sealed with its session.
fn sealed_answer(
    inner: &Inner,
    caller: &fastctx_types::MachineName,
    opened: &Envelope,
    keys: &Keys,
    body: &Body,
) -> Option<Signed> {
    let n = match inner.counter.next() {
        Ok(n) => n,
        Err(error) => {
            inner.node.log(format!("numbering an answer: {error}"));
            return None;
        }
    };
    let (world, me) = (&inner.membership.world, &inner.membership.name);
    let (session, re) = (Some(keys.session), Some(opened.n));
    let header = envelope::header(world, me, caller, n, Kind::Answer, &session, &re);
    let plaintext = serde_json::to_vec(body).expect("bodies serialize");
    let envelope = Envelope {
        world: world.clone(),
        from: me.clone(),
        to: caller.clone(),
        n,
        kind: Kind::Answer,
        session,
        re,
        sealed: Bytes(keys.seal(n, &header, &plaintext)),
    };
    Some(envelope.sign(&inner.key))
}

/// Sends an unsealed answer of `kind` to the call `n` of `caller`.
#[allow(clippy::too_many_arguments)]
async fn reply(
    inner: &Inner,
    up: &Outgoing,
    call: u64,
    caller: &fastctx_types::MachineName,
    n: u64,
    kind: Kind,
    session: Option<crate::noise::SessionId>,
    body: Vec<u8>,
) {
    let mine = match inner.counter.next() {
        Ok(mine) => mine,
        Err(error) => {
            inner.node.log(format!("numbering an answer: {error}"));
            return;
        }
    };
    let envelope = Envelope {
        world: inner.membership.world.clone(),
        from: inner.membership.name.clone(),
        to: caller.clone(),
        n: mine,
        kind,
        session,
        re: Some(n),
        sealed: Bytes(body),
    };
    if let Err(error) = up.write(Up::Answer { call, message: envelope.sign(&inner.key) }).await {
        inner.node.log(format!("answering {caller}: {error}"));
    }
}

/// Opens a call, and fetches it or opens its data connection when it comes
/// with one.
async fn open_call(inner: &Inner, opened: &Envelope, keys: &Keys) -> Result<(RemoteCall, Option<Stream>), String> {
    let plaintext = keys.open(opened.n, &opened.header(), &opened.sealed.0).map_err(|error| error.to_string())?;
    let body: Body = serde_json::from_slice(&plaintext).map_err(|error| format!("it cannot be read: {error}"))?;
    let from_the_caller = |ticket: &Signed| -> Result<(), String> {
        let ticket: Ticket = ticket.claim().map_err(|error| format!("the ticket: {error}"))?;
        if ticket.source != opened.from || ticket.sink != inner.membership.name {
            return Err(format!("its ticket is for {} to {}", ticket.source, ticket.sink));
        }
        Ok(())
    };
    match body {
        Body::Call { call } => Ok((call, None)),
        Body::StreamCall { call, ticket, key } => {
            from_the_caller(&ticket)?;
            let stream = Stream::open(&inner.membership, &inner.key, ticket, &key, End::Sink)
                .await
                .map_err(|error| format!("opening its data connection: {error}"))?;
            Ok((call, Some(stream)))
        }
        Body::LargeCall { carried } => {
            from_the_caller(&carried.ticket)?;
            let bytes = crate::data::sink(&inner.membership, &inner.key, &carried)
                .await
                .map_err(|error| format!("fetching it, too large for a message: {error}"))?;
            Ok((serde_json::from_slice(&bytes).map_err(|error| format!("it cannot be read: {error}"))?, None))
        }
        Body::Answer { .. } | Body::LargeAnswer { .. } => Err("it is not a call".to_string()),
    }
}
