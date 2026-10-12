//! Data connections: one per transfer, between a source and a sink. When
//! the call they go with went through the hub, each end connects to the
//! hub, which pairs them; when it went over a direct link, the two machines
//! connect to each other (`crate::link`). Bytes travel as binary messages;
//! the two ends may also say short things to each other, as JSON text
//! messages, such as which of the offered files the sink wants. A transfer
//! ends when the right its call was made under is taken back: the hub cuts
//! the pair it relays, and over a direct link the machine answering the
//! call cuts its own end.

use futures_util::{SinkExt as _, StreamExt as _};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::dial::{self, HubSocket, Upstream};
use crate::ids::{CertPin, HubId};
use crate::sign::{Digest, Domain, SecretKey, sha256};
use crate::wire::{self, Carried, Down, End, Intent, Up};

/// What the sink says when it has every byte and they check out. The
/// source waits for it before closing, since a close passes the hub as a
/// close of both directions.
const DONE: &str = "\"done\"";

/// Where a data connection goes.
pub enum Endpoint {
    /// To the hub, which pairs it with the other end: by the ways this
    /// machine reaches the hub, the one its control connection is on first.
    Hub { upstreams: Vec<Upstream>, hub: HubId, cert: CertPin },
    /// Straight to the machine at the other end of the direct link the call
    /// went over.
    Peer(crate::link::DataWay),
}

/// One end of a paired data connection.
pub struct Stream {
    socket: HubSocket,
    /// Bytes received and not yet taken.
    pending: Vec<u8>,
    /// Says why, when this machine cut the call the transfer goes with.
    cut: Option<watch::Receiver<Option<String>>>,
}

impl Stream {
    /// Opens this machine's end of the transfer `stream` and waits for the
    /// other end. The hub ends the wait if the other end's machine leaves;
    /// over a direct link, the link's end does.
    pub async fn open(endpoint: &Endpoint, key: &SecretKey, stream: u64, end: End) -> Result<Self, String> {
        let (upstreams, hub, cert) = match endpoint {
            Endpoint::Hub { upstreams, hub, cert } => (upstreams, hub, *cert),
            Endpoint::Peer(way) => {
                let socket = way.open(stream, end).await?;
                return Ok(Self { socket, pending: Vec::new(), cut: way.cut() });
            }
        };
        let limit = wire::MAX_DATA_MESSAGE + wire::MAX_CONTROL_FRAME;
        let mut failures = Vec::new();
        let mut dialed = None;
        for upstream in upstreams {
            match dial::dial_upstream(upstream, hub, cert, wire::DATA_PATH, limit).await {
                Ok(reached) => {
                    dialed = Some(reached);
                    break;
                }
                Err(error) => failures.push(format!("{upstream}: {error}")),
            }
        }
        let dialed = dialed.ok_or_else(|| failures.join("; "))?;
        let mut socket = dialed.socket;
        let proof = key.sign(Domain::Session, &dialed.binding);
        let hello = Up::Hello {
            protocol: wire::GREETING_PROTOCOL,
            version: env!("CARGO_PKG_VERSION").to_string(),
            intent: Intent::Data { key: key.public(), proof, stream, end },
        };
        dial::send(&mut socket, &hello).await?;
        match dial::recv(&mut socket).await? {
            Some(Down::Paired) => Ok(Self { socket, pending: Vec::new(), cut: None }),
            Some(Down::Refused { reason }) => Err(reason.to_string()),
            Some(_) => Err("the hub answered something other than a pairing".to_string()),
            None => Err("the hub closed the connection".to_string()),
        }
    }

    /// Why the transfer was cut, if it was.
    fn was_cut(&self) -> Option<String> {
        self.cut.as_ref().and_then(|cut| cut.borrow().clone())
    }

    /// Sends bytes.
    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        for chunk in bytes.chunks(wire::MAX_DATA_MESSAGE) {
            self.put(WsMessage::Binary(chunk.to_vec().into())).await?;
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

    /// Passes exactly `count` bytes on to `to` as they come, holding no more
    /// than one message of them at a time.
    pub async fn forward(&mut self, to: &mut Stream, mut count: u64) -> Result<(), String> {
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
            to.send(&self.pending[..here]).await?;
            self.pending.drain(..here);
            count -= here as u64;
        }
        Ok(())
    }

    /// Says something to the other end.
    pub async fn say(&mut self, word: &impl Serialize) -> Result<(), String> {
        self.put(WsMessage::Text(serde_json::to_string(word).expect("words serialize").into())).await
    }

    async fn put(&mut self, message: WsMessage) -> Result<(), String> {
        if let Some(why) = self.was_cut() {
            return Err(why);
        }
        self.socket.send(message).await.map_err(|error| format!("sending: {}", dial::describe(&error)))
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
        self.put(WsMessage::Text(DONE.into())).await.map_err(|error| format!("sending the receipt: {error}"))?;
        let _ = self.socket.close(None).await;
        Ok(())
    }

    async fn next(&mut self) -> Result<Option<Message>, String> {
        loop {
            let next = match &mut self.cut {
                Some(cut) => tokio::select! {
                    next = self.socket.next() => next,
                    cut = cut.wait_for(Option::is_some) => {
                        return Err(cut.map(|why| why.clone().unwrap_or_default()).unwrap_or_else(|_| "the call was cut".to_string()));
                    }
                },
                None => self.socket.next().await,
            };
            match next {
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

/// Sends `bytes` as the source of the transfer `stream`.
pub async fn source(endpoint: &Endpoint, key: &SecretKey, stream: u64, bytes: &[u8]) -> Result<(), String> {
    let mut stream = Stream::open(endpoint, key, stream, End::Source).await?;
    stream.send(bytes).await?;
    stream.finish().await
}

/// Takes the bytes `carried` promised, as the sink of its transfer, and
/// checks them.
pub async fn sink(endpoint: &Endpoint, key: &SecretKey, carried: &Carried) -> Result<Vec<u8>, String> {
    let mut stream = Stream::open(endpoint, key, carried.stream, End::Sink).await?;
    let mut bytes = Vec::new();
    stream
        .take(carried.bytes, |piece| {
            bytes.extend_from_slice(piece);
            Ok(())
        })
        .await?;
    if Digest(sha256(&bytes)) != carried.sha256 {
        return Err("the bytes are not the ones the source said it sent".to_string());
    }
    stream.done().await?;
    Ok(bytes)
}
