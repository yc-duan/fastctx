//! Data connections: one per transfer, between a source and a sink, paired
//! by the hub. Bytes travel as binary messages; the two ends may also say
//! short things to each other, as JSON in text messages, such as which of
//! the offered files the sink wants. Signed control messages or signed
//! stream pieces describe the bytes, so the hub in between can neither
//! change them unnoticed nor make them up.

use futures_util::{SinkExt as _, StreamExt as _};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::dial::{self, HubSocket};
use crate::link::Membership;
use crate::sign::{Digest, Domain, SecretKey, Signed, sha256};
use crate::wire::{self, Carried, Down, End, Intent, PROTOCOL, Up};

/// What the sink says when it has every byte and they check out. The
/// source waits for it before closing, since a close passes the hub as a
/// close of both directions.
const DONE: &str = "\"done\"";

/// One end of a paired data connection.
pub struct Stream {
    socket: HubSocket,
    /// Bytes received and not yet taken.
    pending: Vec<u8>,
}

impl Stream {
    /// Opens this machine's end of the transfer `ticket` names and waits
    /// for the other end. The hub ends the wait if the other end's machine
    /// leaves.
    pub async fn open(membership: &Membership, key: &SecretKey, ticket: Signed, end: End) -> Result<Self, String> {
        let limit = wire::MAX_DATA_MESSAGE + wire::MAX_CONTROL_FRAME;
        let dialed = dial::dial(&membership.hub, membership.cert, wire::DATA_PATH, limit).await?;
        let mut socket = dialed.socket;
        let proof = key.sign(Domain::Session, &dialed.binding);
        let hello = Up::Hello {
            protocol: PROTOCOL,
            version: env!("CARGO_PKG_VERSION").to_string(),
            intent: Intent::Data { key: key.public(), proof, ticket, end },
        };
        dial::send(&mut socket, &hello).await?;
        match dial::recv(&mut socket).await? {
            Some(Down::Paired) => Ok(Self { socket, pending: Vec::new() }),
            Some(Down::Refused { reason }) => Err(reason.to_string()),
            Some(_) => Err("the hub answered something other than a pairing".to_string()),
            None => Err("the hub closed the connection".to_string()),
        }
    }

    /// Sends bytes.
    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        for chunk in bytes.chunks(wire::MAX_DATA_MESSAGE) {
            self.socket
                .send(WsMessage::Binary(chunk.to_vec().into()))
                .await
                .map_err(|error| format!("sending: {}", dial::describe(&error)))?;
        }
        Ok(())
    }

    /// Takes exactly `count` bytes, handing each piece to `take` as it comes.
    pub async fn take(
        &mut self,
        mut count: u64,
        mut take: impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        while count > 0 {
            if self.pending.is_empty() {
                match self.next().await? {
                    Some(Message::Bytes(bytes)) => self.pending = bytes,
                    Some(Message::Word(_)) => {
                        return Err("the other end said something where bytes were due".to_string());
                    }
                    None => return Err(format!("the other end stopped with {count} bytes still due")),
                }
            }
            let here = (self.pending.len() as u64).min(count) as usize;
            take(&self.pending[..here])?;
            self.pending.drain(..here);
            count -= here as u64;
        }
        Ok(())
    }

    /// Says something to the other end.
    pub async fn say(&mut self, word: &impl Serialize) -> Result<(), String> {
        let text = serde_json::to_string(word).expect("words serialize");
        self.socket
            .send(WsMessage::Text(text.into()))
            .await
            .map_err(|error| format!("sending: {}", dial::describe(&error)))
    }

    /// What the other end says next.
    pub async fn hear<T: DeserializeOwned>(&mut self) -> Result<T, String> {
        if !self.pending.is_empty() {
            return Err("bytes came where the other end was to say something".to_string());
        }
        match self.next().await? {
            Some(Message::Word(text)) => {
                serde_json::from_str(&text).map_err(|error| format!("what the other end said: {error}"))
            }
            Some(Message::Bytes(_)) => Err("bytes came where the other end was to say something".to_string()),
            None => Err("the other end left".to_string()),
        }
    }

    /// Waits until the other end closes or breaks the connection. For an
    /// end that only sends, so it notices the other end has gone while it
    /// has nothing to send; anything the other end says meanwhile is
    /// dropped. Dropping the wait halfway loses nothing else.
    pub async fn closed(&mut self) {
        while let Ok(Some(_)) = self.next().await {}
    }

    /// As the source: waits for the sink's word that it has everything, and
    /// closes.
    pub async fn finish(mut self) -> Result<(), String> {
        match self.next().await? {
            Some(Message::Word(text)) if text == DONE => {
                let _ = self.socket.close(None).await;
                Ok(())
            }
            Some(Message::Word(text)) => Err(format!("the other end did not take it: {text}")),
            Some(Message::Bytes(_)) => Err("the other end sent bytes instead of its receipt".to_string()),
            None => Err("the other end left before saying it had everything".to_string()),
        }
    }

    /// As the sink: says everything arrived, and closes.
    pub async fn done(mut self) -> Result<(), String> {
        self.socket
            .send(WsMessage::Text(DONE.into()))
            .await
            .map_err(|error| format!("sending the receipt: {}", dial::describe(&error)))?;
        let _ = self.socket.close(None).await;
        Ok(())
    }

    async fn next(&mut self) -> Result<Option<Message>, String> {
        loop {
            match self.socket.next().await {
                None | Some(Ok(WsMessage::Close(_))) => return Ok(None),
                Some(Ok(WsMessage::Binary(bytes))) => return Ok(Some(Message::Bytes(bytes.to_vec()))),
                Some(Ok(WsMessage::Text(text))) => return Ok(Some(Message::Word(text.to_string()))),
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
                Some(Err(error)) => return Err(format!("receiving: {}", dial::describe(&error))),
            }
        }
    }
}

enum Message {
    Bytes(Vec<u8>),
    Word(String),
}

/// Sends `bytes` as the source of the transfer `ticket` names.
pub async fn source(membership: &Membership, key: &SecretKey, ticket: Signed, bytes: &[u8]) -> Result<(), String> {
    let mut stream = Stream::open(membership, key, ticket, End::Source).await?;
    stream.send(bytes).await?;
    stream.finish().await
}

/// Takes the bytes `carried` promised, as the sink of its transfer, and
/// checks them.
pub async fn sink(membership: &Membership, key: &SecretKey, carried: &Carried) -> Result<Vec<u8>, String> {
    let mut stream = Stream::open(membership, key, carried.ticket.clone(), End::Sink).await?;
    let mut bytes = Vec::new();
    stream
        .take(carried.bytes, |piece| {
            bytes.extend_from_slice(piece);
            Ok(())
        })
        .await?;
    if Digest(sha256(&bytes)) != carried.sha256 {
        return Err("the bytes are not the ones the source signed for".to_string());
    }
    stream.done().await?;
    Ok(bytes)
}
