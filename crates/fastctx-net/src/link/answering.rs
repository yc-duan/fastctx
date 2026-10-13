//! Answering other members' calls, whether the hub passed them on or they
//! came over a direct link.
//!
//! Every call is checked before anything else: that it is for this machine,
//! and that the view lets its sender ask it, for the account it names
//! (`Inner::admit`); over a direct link, also that it says it is from the
//! machine at the other end. A call that does not check out is refused in a
//! short word that names the reason; one that does goes to the node, which
//! checks again what the call asks of this machine. A call over a direct
//! link is registered while it is answered, so that taking back the right
//! it was made under cuts it (`Inner::cut_lapsed`).

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteResult};

use super::direct::DataWay;
use super::{Inner, Replier};
use crate::data::{Endpoint, Stream};
use crate::envelope::{Envelope, Kind};
use crate::message::Body;
use crate::sign::sha256;
use crate::wire::{self, Carried, End, Ticket, Up};

/// Checks a call from another machine, and does what it asks.
pub(super) async fn answer(inner: Arc<Inner>, replier: Replier, message: Envelope) {
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
    let caller = match inner.admit(&message).await {
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
    let mut _answering = None;
    let result: RemoteResult = match open_call(&inner, &message, &endpoint).await {
        Ok((remote, stream)) => match inner.node.answering(&remote) {
            Ok(guard) => {
                _answering = guard;
                inner.node.call(caller.clone(), remote, stream).await
            }
            Err(failure) => Err(failure),
        },
        Err(reason) => {
            inner.node.log(format!("refused a call from {}: {reason}", caller.machine));
            Err(Failure::new(format!("{} refused the call: {reason}", inner.me())))
        }
    };
    if registered.as_ref().is_some_and(|registered| !registered.wanted()) {
        // Cut while it ran: the caller was told, and the answer is dropped.
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
    if let Err(error) = crate::data::source(&endpoint, &inner.key, stream, &bytes).await {
        inner.node.log(format!("sending a large answer to {}: {error}", caller.machine));
    }
}

/// Sends `message`, which answers a call, back the way the call came.
async fn send(replier: &Replier, message: Envelope) -> Result<(), String> {
    match replier {
        Replier::Hub(up, call) => up.write(Up::Answer { call: *call, message }).await,
        Replier::Peer(conn, id) => conn
            .send(crate::peer::Frame::Answer { id: *id, message })
            .map_err(|()| "the direct link ended before the answer was queued".to_string()),
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

/// The call an envelope holds, fetched or with its data connection opened
/// when it comes with one.
async fn open_call(
    inner: &Inner,
    call: &Envelope,
    endpoint: &Endpoint,
) -> Result<(RemoteCall, Option<Stream>), String> {
    let body: Body = call.body()?;
    let from_the_caller = || -> Result<u64, String> {
        let ticket = call.stream.as_ref().ok_or("it names no data connection")?;
        if ticket.source != call.from || !inner.view().is_me(&ticket.sink) {
            return Err(format!("its data connection is for {} to {}", ticket.source, ticket.sink));
        }
        Ok(ticket.stream)
    };
    match body {
        Body::Call { call } => Ok((call, None)),
        Body::StreamCall { call: remote } => {
            let stream = Stream::open(endpoint, &inner.key, from_the_caller()?, End::Sink)
                .await
                .map_err(|error| format!("opening its data connection: {error}"))?;
            Ok((remote, Some(stream)))
        }
        Body::LargeCall { carried } => {
            if from_the_caller()? != carried.stream {
                return Err("it names one data connection and carries another".to_string());
            }
            let bytes = crate::data::sink(endpoint, &inner.key, &carried)
                .await
                .map_err(|error| format!("fetching it, too large for a message: {error}"))?;
            Ok((serde_json::from_slice(&bytes).map_err(|error| format!("it cannot be read: {error}"))?, None))
        }
        Body::Answer { .. } | Body::LargeAnswer { .. } => Err("it is not a call".to_string()),
    }
}
