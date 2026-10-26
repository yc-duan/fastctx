//! Answering other members' calls, whether the hub passed them on or they
//! came over a direct link.
//!
//! Every call is checked before anything else: that it is for this machine,
//! and that the view lets its sender ask it, for the account it names
//! (`Inner::admit`); over a direct link, also that it says it is from the
//! machine at the other end. A call that does not check out is refused in a
//! short word that names the reason; one that does takes its place here
//! (`Node::place`) before anything waits on it, and goes to the node, which
//! checks again what the call asks. A call over a direct link is registered
//! while it is answered, so that taking back the right it was made under
//! cuts it (`Inner::cut_lapsed`).
//!
//! What each call is (`fastctx_types::contract`) says what stops it. Until
//! the node takes it on, anything does: its caller giving it up, or the
//! connection it came on ending. Once taken on, a call that only reads stops
//! too, and also on its own a little past the time its caller waits; a
//! change is done whole, whoever is left to hear of it.
//!
//! A call holds its place here, and what keeps its node from handing over
//! (`Node::answering`), until its answer is written, a large answer's bytes
//! all sent, or the answer given up: a node handing over waits for answers
//! to go, not only for the work behind them.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::contract::Pool;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteResult};

use super::answers::Answer;
use super::direct::DataWay;
use super::{Caller, Inner, Replier};
use crate::data::{Endpoint, Stream};
use crate::envelope::{Envelope, Kind};
use crate::message::Body;
use crate::sign::sha256;
use crate::wire::{self, Carried, End, Ticket, Up};

/// How long past the time its caller waits a call that only reads is
/// answered, for a caller whose giving up never came: past that it is
/// given up here too.
const PAST_THE_CALLER: Duration = Duration::from_secs(5);

/// Checks a call from another machine, and does what it asks.
pub(super) async fn answer(inner: Arc<Inner>, replier: Replier, message: Envelope, mut stop: Answer) {
    if message.kind != Kind::Call {
        inner.node.log(format!("dropped what came from {} as a call: it is a {:?}", message.from, message.kind));
        return;
    }
    if let Replier::Peer(conn, _) = &replier
        && message.from != conn.name
    {
        let reason = format!("it says it is from {}, on the direct link of {}", message.from, conn.name);
        inner.node.log(format!("refused a direct call: {reason}"));
        reply(&inner, &replier, &message, Kind::Refused, None, serde_json::Value::String(reason)).await;
        return;
    }
    if matches!(replier, Replier::Peer(..)) && crate::message::plan_request(&message).is_some() {
        reply(
            &inner,
            &replier,
            &message,
            Kind::Refused,
            None,
            serde_json::Value::String(fastctx_types::plan::words::CONTROL.into()),
        )
        .await;
        return;
    }
    if matches!(replier, Replier::Peer(..)) && crate::message::project_entry(&message).is_some() {
        let reason = "a project's entry from another machine must be authorized by its hub";
        reply(&inner, &replier, &message, Kind::Refused, None, serde_json::Value::String(reason.into())).await;
        return;
    }
    // Its time here runs from now, not from when its work begins: what it
    // takes to admit it and bring its bytes in is waited for in it.
    let came = tokio::time::Instant::now();
    let admitted = match inner.admits_now(&message) {
        Ok(caller) => Ok(caller),
        Err(_) => {
            // The view may not hold a grant made just now. Asking the hub
            // waits in one of the few places kept for what comes in before
            // it is known, and stops with the call.
            let account = message.standing().map(|(account, _)| account.clone());
            let waiting = account.and_then(|account| {
                inner.node.place(&message.from, &account, Pool::Receiving).map_err(|failure| failure.message)
            });
            match waiting {
                Ok(_place) => tokio::select! {
                    admitted = inner.admit(&message) => admitted,
                    _ = stop.stopping() => return,
                    () = tokio::time::sleep_until(came + fastctx_types::contract::OPENED_WITHIN) => {
                        Err("admitting the call did not finish within 60 seconds".to_string())
                    },
                },
                Err(reason) => Err(reason),
            }
        }
    };
    let caller = match admitted {
        Ok(caller) => caller,
        Err(reason) => {
            inner.node.log(format!("refused a call from {}: {reason}", message.from));
            reply(&inner, &replier, &message, Kind::Refused, None, serde_json::Value::String(reason)).await;
            return;
        }
    };
    // Over a direct link, this machine is the one that cuts the call when
    // its right is taken back, as the hub cuts what it passes on.
    let registered = match &replier {
        Replier::Hub(..) => None,
        Replier::Peer(conn, id) => {
            let purpose = message.purpose.expect("an admitted call names its purpose");
            Some(inner.answering(conn, *id, caller.account.clone(), purpose))
        }
    };
    let endpoint = match &replier {
        Replier::Hub(..) => inner.data_endpoint(),
        Replier::Peer(conn, _) => Endpoint::Peer(DataWay::new(
            inner.clone(),
            conn.clone(),
            registered.as_ref().map(|registered| registered.cut.clone()),
        )),
    };
    // Kept until the answer has gone, or is given up.
    let mut held: Vec<Box<dyn Send>> = Vec::new();
    let Some(result) = work(&inner, &caller, &message, &endpoint, &mut stop, &mut held, came).await else {
        // Nobody is left to hear the answer.
        return;
    };
    if stop.stopped().is_some() || registered.as_ref().is_some_and(|registered| !registered.wanted()) {
        // Given up or cut while it ran: the caller was told, and the answer
        // is dropped.
        return;
    }
    let small = Body::Answer { result: result.clone() };
    let fits = match &replier {
        Replier::Hub(_, call) => {
            let frame =
                Up::Answer { call: *call, message: answer_envelope(&inner, &message, Kind::Answer, None, &small) };
            wire::encode(&frame).len() <= wire::MAX_CONTROL_FRAME
        }
        Replier::Peer(_, id) => {
            let frame = crate::peer::Frame::Answer {
                id: *id,
                message: answer_envelope(&inner, &message, Kind::Answer, None, &small),
            };
            crate::peer::encode(&frame).len() <= wire::MAX_CONTROL_FRAME
        }
    };
    if fits {
        if let Err(error) = send(&replier, answer_envelope(&inner, &message, Kind::Answer, None, &small)).await {
            inner.node.log(format!("sending an answer to {}: {error}", caller.machine));
        }
        return;
    }
    // Too large for the control connection: the answer says what follows
    // over a data connection.
    let bytes = serde_json::to_vec(&result).expect("answers serialize");
    let stream = match crate::sign::random::<8>() {
        Ok(random) => u64::from_le_bytes(random),
        Err(error) => {
            inner.node.log(format!("choosing a data connection for a large answer: {error}"));
            return;
        }
    };
    let ticket = Ticket { stream, source: inner.me(), sink: caller.machine.clone() };
    let carried = Carried { stream, bytes: bytes.len() as u64, sha256: crate::sign::Digest(sha256(&bytes)) };
    let large = answer_envelope(&inner, &message, Kind::Answer, Some(ticket), &Body::LargeAnswer { carried });
    if let Err(error) = send(&replier, large).await {
        inner.node.log(format!("announcing a large answer to {}: {error}", caller.machine));
        return;
    }
    let sent = tokio::select! {
        sent = crate::data::source(&endpoint, &inner.key, stream, &bytes) => sent,
        _ = stop.stopping() => Err("its caller gave it up".to_string()),
    };
    if let Err(error) = sent {
        inner.node.log(format!("sending a large answer to {}: {error}", caller.machine));
    }
    drop(held);
}

/// Takes the call `message` holds on and does it; none when it was stopped
/// first. Its place here and what keeps its node from handing over go into
/// `held`, for its caller to keep until the answer has gone.
async fn work(
    inner: &Arc<Inner>,
    caller: &Caller,
    message: &Envelope,
    endpoint: &Endpoint,
    stop: &mut Answer,
    held: &mut Vec<Box<dyn Send>>,
    came: tokio::time::Instant,
) -> Option<RemoteResult> {
    let refused = |reason: String| {
        inner.node.log(format!("refused a call from {}: {reason}", caller.machine));
        Err(Failure::not_run(format!("{} refused the call: {reason}", inner.me())))
    };
    // A call this release does not know, as a newer one may make, is
    // refused saying so.
    let body: Body = match message.body() {
        Ok(body) => body,
        Err(error) => return Some(refused(format!("{error}; a newer Fastctx may make calls this one does not know"))),
    };
    // A large call's own kind is known only once it is in: it comes in in
    // a place for calls coming in, and then takes its own.
    let pool = match &body {
        Body::Call { call } | Body::StreamCall { call } => call.contract().pool,
        Body::LargeCall { .. } | Body::Answer { .. } | Body::LargeAnswer { .. } => Pool::Receiving,
    };
    let mut place = match inner.node.place(&caller.machine, &caller.account, pool) {
        Ok(place) => place,
        Err(failure) => return Some(Err(failure)),
    };
    let accepting = match &body {
        Body::Call { call } | Body::StreamCall { call } => {
            Some(call.contract().within.unwrap_or(fastctx_types::contract::OPENED_WITHIN))
        }
        _ => None,
    };
    let opening_deadline = came + fastctx_types::contract::OPENED_WITHIN + PAST_THE_CALLER;
    let accepting_deadline = async {
        match accepting {
            Some(within) => tokio::time::sleep_until(came + within + PAST_THE_CALLER).await,
            None => std::future::pending().await,
        }
    };
    let opened = tokio::select! {
        opened = open_call(inner, message, body, endpoint, opening_deadline) => opened,
        _ = stop.stopping() => return None,
        () = accepting_deadline => {
            Err(format!("opening the call did not finish within {} seconds", (accepting.expect("finite opening") + PAST_THE_CALLER).as_secs()))
        },
    };
    let (remote, stream, received) = match opened {
        Ok(opened) => opened,
        Err(reason) => return Some(refused(reason)),
    };
    if matches!(remote, RemoteCall::EnterProject { .. }) && crate::message::project_entry(message).is_none() {
        return Some(refused("a project entry must be a complete, unstreamed control request".into()));
    }
    if matches!(remote, RemoteCall::Plan { .. }) && crate::message::plan_request(message).is_none() {
        return Some(refused(fastctx_types::plan::words::CONTROL.into()));
    }
    let contract = remote.contract();
    if contract.pool != pool {
        place = match inner.node.place(&caller.machine, &caller.account, contract.pool) {
            Ok(place) => place,
            Err(failure) => return Some(Err(failure)),
        };
    }
    held.push(place);
    // A large call's bytes stay counted until its answer is written, past
    // the receiving place it came in through.
    if let Some(received) = received {
        held.push(Box::new(received));
    }
    match inner.node.answering(&remote) {
        Ok(Some(guard)) => held.push(guard),
        Ok(None) => {}
        Err(failure) => {
            // Its data connection is closed, not dropped: the other end
            // then waits for this answer, which says why.
            if let Some(stream) = stream {
                stream.close().await;
            }
            return Some(Err(failure));
        }
    }
    let kind = remote.kind();
    let doing = inner.node.call(caller.clone(), remote, stream);
    if !contract.cancellable {
        return Some(doing.await);
    }
    let past = async {
        match contract.within {
            Some(within) => tokio::time::sleep_until(came + within + PAST_THE_CALLER).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        result = doing => Some(result),
        _ = stop.stopping() => None,
        () = past => {
            let waited = contract.within.unwrap_or_default() + PAST_THE_CALLER;
            let reason = format!("gave up answering its {kind}: no answer within {} s", waited.as_secs());
            inner.node.log(format!("{reason}, for {}", caller.machine));
            Some(Err(Failure::new(format!("{} {reason}", inner.me()))))
        }
    }
}

/// Sends `message`, which answers a call, back the way the call came.
async fn send(replier: &Replier, message: Envelope) -> Result<(), String> {
    match replier {
        Replier::Hub(up, call) => up.write(Up::Answer { call: *call, message }).await,
        Replier::Peer(conn, id) => conn.write(crate::peer::Frame::Answer { id: *id, message }).await,
    }
}

/// An answer of `kind` to `call`.
fn answer_envelope(
    inner: &Inner,
    call: &Envelope,
    kind: Kind,
    stream: Option<Ticket>,
    body: &impl serde::Serialize,
) -> Envelope {
    Envelope {
        hub: inner.hub.clone(),
        from: inner.me(),
        to: call.from.clone(),
        n: 0,
        kind,
        account: None,
        purpose: None,
        re: Some(call.n),
        stream,
        body: serde_json::to_value(body).expect("bodies serialize"),
    }
}

/// Sends a short answer of `kind` to `call`.
async fn reply(
    inner: &Inner,
    replier: &Replier,
    call: &Envelope,
    kind: Kind,
    stream: Option<Ticket>,
    body: serde_json::Value,
) {
    if let Err(error) = send(replier, answer_envelope(inner, call, kind, stream, &body)).await {
        inner.node.log(format!("answering {}: {error}", call.from));
    }
}

/// The call `body`, which came in `call`, fetched or with its data
/// connection opened when it comes with one.
async fn open_call(
    inner: &Inner,
    call: &Envelope,
    body: Body,
    endpoint: &Endpoint,
    opening_deadline: tokio::time::Instant,
) -> Result<(RemoteCall, Option<Stream>, Option<crate::reassembly::Reservation>), String> {
    let from_the_caller = || -> Result<u64, String> {
        let ticket = call.stream.as_ref().ok_or("it names no data connection")?;
        if ticket.source != call.from || !inner.view().is_me(&ticket.sink) {
            return Err(format!("its data connection is for {} to {}", ticket.source, ticket.sink));
        }
        Ok(ticket.stream)
    };
    match body {
        Body::Call { call } => Ok((call, None, None)),
        Body::StreamCall { call: remote } => {
            let stream = Stream::open(endpoint, &inner.key, from_the_caller()?, End::Sink)
                .await
                .map_err(|error| format!("opening its data connection: {error}"))?;
            Ok((remote, Some(stream), None))
        }
        Body::LargeCall { carried, details } => {
            let details =
                details.ok_or("upgrade the sender: its large call carries no verifiable authorization facts")?;
            if from_the_caller()? != carried.stream {
                return Err("it names one data connection and carries another".to_string());
            }
            let bytes = crate::data::sink_until(endpoint, &inner.key, &carried, opening_deadline)
                .await
                .map_err(|error| format!("fetching it, too large for a message: {error}"))?;
            let remote: RemoteCall = serde_json::from_slice(&bytes).map_err(|error| {
                format!("it cannot be read: {error}; a newer Fastctx may make calls this one does not know")
            })?;
            if details != crate::call_details::CallDetails::of(&call.from, &remote)
                || call.purpose != Some(details.purpose)
            {
                return Err("the large call differs from its authorized control-frame facts".into());
            }
            Ok((remote, None, Some(bytes.into_reservation())))
        }
        Body::Answer { .. } | Body::LargeAnswer { .. } => Err("it is not a call".to_string()),
    }
}
