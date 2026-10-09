//! Answering other members' calls.
//!
//! Every call is checked before anything else: that it is for this machine,
//! and that the view lets its sender ask it, for the account it names
//! (`Inner::admit`). A call that does not check out is refused in a short
//! word that names the reason; one that does goes to the node, which checks
//! again what the call asks of this machine.

use std::sync::Arc;

use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteResult};

use super::outgoing::Outgoing;
use super::Inner;
use crate::data::Stream;
use crate::envelope::{Envelope, Kind};
use crate::message::Body;
use crate::sign::sha256;
use crate::wire::{self, Carried, End, Ticket, Up};

/// Checks a call from another machine, and does what it asks.
pub(super) async fn answer(inner: Arc<Inner>, up: Outgoing, call: u64, message: Envelope) {
    if message.kind != Kind::Call {
        inner.node.log(format!("dropped what came from {} as a call: it is a {:?}", message.from, message.kind));
        return;
    }
    let caller = match inner.admit(&message).await {
        Ok(caller) => caller,
        Err(reason) => {
            inner.node.log(format!("refused a call from {}: {reason}", message.from));
            reply(&inner, &up, call, &message, Kind::Refused, None, serde_json::Value::String(reason)).await;
            return;
        }
    };
    let mut _answering = None;
    let result: RemoteResult = match open_call(&inner, &message).await {
        Ok((remote, stream)) => match inner.node.answering(&remote) {
            Ok(guard) => {
                _answering = guard;
                inner.node.call(caller.clone(), remote, stream).await
            }
            Err(failure) => Err(failure),
        },
        Err(reason) => {
            inner.node.log(format!("refused a call from {}: {reason}", caller.machine));
            Err(Failure::new(format!("{} refused the call: {reason}", inner.membership.name)))
        }
    };
    let small = Body::Answer { result: result.clone() };
    let frame = Up::Answer { call, message: answer_envelope(&inner, &message, Kind::Answer, None, &small) };
    if wire::encode(&frame).len() <= wire::MAX_CONTROL_FRAME {
        if let Err(error) = up.write(frame).await {
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
    let ticket = Ticket { stream, source: inner.membership.name.clone(), sink: caller.machine.clone() };
    let carried = Carried { stream, bytes: bytes.len() as u64, sha256: crate::sign::Digest(sha256(&bytes)) };
    let large = answer_envelope(&inner, &message, Kind::Answer, Some(ticket), &Body::LargeAnswer { carried });
    if let Err(error) = up.write(Up::Answer { call, message: large }).await {
        inner.node.log(format!("announcing a large answer to {}: {error}", caller.machine));
        return;
    }
    if let Err(error) = crate::data::source(&inner.data_endpoint(), &inner.key, stream, &bytes).await {
        inner.node.log(format!("sending a large answer to {}: {error}", caller.machine));
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
        hub: inner.membership.world.clone(),
        from: inner.membership.name.clone(),
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
    up: &Outgoing,
    id: u64,
    call: &Envelope,
    kind: Kind,
    stream: Option<Ticket>,
    body: serde_json::Value,
) {
    if let Err(error) = up.write(Up::Answer { call: id, message: answer_envelope(inner, call, kind, stream, &body) }).await
    {
        inner.node.log(format!("answering {}: {error}", call.from));
    }
}

/// The call an envelope holds, fetched or with its data connection opened
/// when it comes with one.
async fn open_call(inner: &Inner, call: &Envelope) -> Result<(RemoteCall, Option<Stream>), String> {
    let body: Body = call.body()?;
    let from_the_caller = || -> Result<u64, String> {
        let ticket = call.stream.as_ref().ok_or("it names no data connection")?;
        if ticket.source != call.from || ticket.sink != inner.membership.name {
            return Err(format!("its data connection is for {} to {}", ticket.source, ticket.sink));
        }
        Ok(ticket.stream)
    };
    match body {
        Body::Call { call } => Ok((call, None)),
        Body::StreamCall { call: remote } => {
            let stream = Stream::open(&inner.data_endpoint(), &inner.key, from_the_caller()?, End::Sink)
                .await
                .map_err(|error| format!("opening its data connection: {error}"))?;
            Ok((remote, Some(stream)))
        }
        Body::LargeCall { carried } => {
            if from_the_caller()? != carried.stream {
                return Err("it names one data connection and carries another".to_string());
            }
            let bytes = crate::data::sink(&inner.data_endpoint(), &inner.key, &carried)
                .await
                .map_err(|error| format!("fetching it, too large for a message: {error}"))?;
            Ok((serde_json::from_slice(&bytes).map_err(|error| format!("it cannot be read: {error}"))?, None))
        }
        Body::Answer { .. } | Body::LargeAnswer { .. } => Err("it is not a call".to_string()),
    }
}
