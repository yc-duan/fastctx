//! The two ends of a data connection: the source sends the bytes a signed
//! message promised, the sink takes exactly those bytes and checks them.

use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::dial::{self, HubSocket};
use crate::link::Membership;
use crate::sign::{Digest, Domain, SecretKey, Signed, sha256};
use crate::wire::{self, Carried, Down, End, Intent, PROTOCOL, SILENCE, Up};

/// What the sink sends when it has every byte and they check out. The
/// source waits for it before closing, since a close passes the hub as a
/// close of both directions.
const DONE: &[u8] = b"done";

/// Sends `bytes` as the source of the transfer `ticket` names.
pub async fn source(membership: &Membership, key: &SecretKey, ticket: Signed, bytes: &[u8]) -> Result<(), String> {
    let mut socket = open(membership, key, ticket, End::Source).await?;
    for chunk in bytes.chunks(wire::MAX_DATA_MESSAGE) {
        socket.send(WsMessage::Binary(chunk.to_vec().into())).await.map_err(|error| format!("sending: {error}"))?;
    }
    loop {
        match socket.next().await {
            Some(Ok(WsMessage::Binary(word))) if word.as_ref() == DONE => break,
            Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
            Some(Ok(_)) => {
                return Err("the other end sent something other than its receipt".to_string());
            }
            Some(Err(error)) => return Err(format!("waiting for the receipt: {error}")),
            None => return Err("the other end left before saying it had everything".to_string()),
        }
    }
    let _ = socket.close(None).await;
    Ok(())
}

/// Takes the bytes `carried` promised, as the sink of its transfer.
pub async fn sink(membership: &Membership, key: &SecretKey, carried: &Carried) -> Result<Vec<u8>, String> {
    let mut socket = open(membership, key, carried.ticket.clone(), End::Sink).await?;
    let mut bytes = Vec::new();
    while (bytes.len() as u64) < carried.bytes {
        match socket.next().await {
            Some(Ok(WsMessage::Binary(chunk))) => bytes.extend_from_slice(&chunk),
            Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
            Some(Ok(_)) => return Err("the source sent something other than bytes".to_string()),
            Some(Err(error)) => return Err(format!("receiving: {error}")),
            None => {
                return Err(format!("the source stopped after {} of {} bytes", bytes.len(), carried.bytes));
            }
        }
        if bytes.len() as u64 > carried.bytes {
            return Err(format!("the source sent more than the {} bytes it promised", carried.bytes));
        }
    }
    if Digest(sha256(&bytes)) != carried.sha256 {
        return Err("the bytes are not the ones the source signed for".to_string());
    }
    socket
        .send(WsMessage::Binary(DONE.to_vec().into()))
        .await
        .map_err(|error| format!("sending the receipt: {error}"))?;
    let _ = socket.close(None).await;
    Ok(bytes)
}

async fn open(membership: &Membership, key: &SecretKey, ticket: Signed, end: End) -> Result<HubSocket, String> {
    let limit = wire::MAX_DATA_MESSAGE + wire::MAX_CONTROL_FRAME;
    let dialed = tokio::time::timeout(SILENCE, dial::dial(&membership.hub, membership.cert, wire::DATA_PATH, limit))
        .await
        .map_err(|_| format!("the hub did not answer within {} s", SILENCE.as_secs()))??;
    let mut socket = dialed.socket;
    let proof = key.sign(Domain::Session, &dialed.binding);
    let hello = Up::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: Intent::Data { key: key.public(), proof, ticket, end },
    };
    dial::send(&mut socket, &hello).await?;
    // The other end may take a while to come; the hub ends the wait if it
    // leaves.
    match dial::recv(&mut socket).await? {
        Some(Down::Paired) => Ok(socket),
        Some(Down::Refused { reason }) => Err(reason.to_string()),
        Some(_) => Err("the hub answered something other than a pairing".to_string()),
        None => Err("the hub closed the connection".to_string()),
    }
}
