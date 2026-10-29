//! Data connections: one per transfer, between a source and a sink. When
//! the call they go with went through the hub, each end connects to the
//! hub, which pairs them; when it went over a direct link, the two machines
//! connect to each other (`crate::link`). Bytes travel as binary messages;
//! the two ends may also say short things to each other, as JSON text
//! messages, such as which of the offered files the sink wants. A transfer
//! ends when the right its call was made under is taken back: the hub cuts
//! the pair it relays, and over a direct link the machine answering the
//! call cuts its own end. Both ends keep the connection alive and take it
//! for gone when the other falls silent (`line`).

mod duplex;
mod line;
pub use duplex::{ReadHalf, WriteHalf};
mod receipt;
mod writing;
pub use receipt::{ReceiptWork, Settlement, settling_ping};

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::dial::{self, HubSocket, Upstream};
use crate::ids::{CertPin, HubId};
use crate::sign::{Digest, Domain, SecretKey, sha256};
use crate::wire::{self, Carried, Down, End, Intent, Up};

use line::{Line, Received as Message};

/// What the sink says when it has every byte and they check out. The
/// source waits for it before closing, since a close passes the hub as a
/// close of both directions.
const DONE: &str = "\"done\"";

/// How long an end waits for the hub to pair it: the hub waits a minute
/// for the other end, and says so within a silence.
const PAIRED_WITHIN: std::time::Duration = std::time::Duration::from_secs(90);

/// How long bytes the other end said it sends may stop coming before it is
/// given up: a quiet stream may wait for ever, bytes due may not.
pub const DUE_WITHIN: std::time::Duration = std::time::Duration::from_secs(120);

/// The most bytes one message carries. The other end counts only whole
/// messages as hearing from this one, so each must arrive well within
/// `SILENCE`: a transfer's messages grow to this only on a line that takes
/// them at once (`Pace`).
pub const CHUNK: usize = 64 * 1024;

/// The fewest bytes a message carries, however slow the line: 4 KiB within
/// the silence is about 140 bytes a second.
pub const SMALLEST_CHUNK: usize = 4 * 1024;

/// How large each message of a transfer is: as large as the line takes
/// well within the silence, which the other end counts only whole messages
/// against. It starts at the smallest; one that took long to write halves
/// it, and it doubles, up to `CHUNK`, once more than any send buffer holds
/// went at once, as a buffer taking the first messages at once says
/// nothing of the line behind it.
pub struct Pace {
    chunk: usize,
    /// The bytes written at once since the size last changed.
    quick: usize,
}

/// A message that took this long to write halves the next.
const SLOW_MESSAGE: std::time::Duration = std::time::Duration::from_secs(4);
/// One written within this went at once.
const QUICK_MESSAGE: std::time::Duration = std::time::Duration::from_millis(500);
/// How much has to go at once before the messages grow.
const BUFFERED: usize = 512 * 1024;

impl Default for Pace {
    fn default() -> Self {
        Self { chunk: SMALLEST_CHUNK, quick: 0 }
    }
}

impl Pace {
    /// How many bytes the next message carries.
    pub fn chunk(&self) -> usize {
        self.chunk
    }

    /// A message of `bytes` took `took` to write.
    pub fn wrote(&mut self, bytes: usize, took: std::time::Duration) {
        if took >= SLOW_MESSAGE {
            self.chunk = (self.chunk / 2).max(SMALLEST_CHUNK);
            self.quick = 0;
        } else if took <= QUICK_MESSAGE {
            self.quick += bytes;
            if self.quick >= BUFFERED {
                self.chunk = (self.chunk * 2).min(CHUNK);
                self.quick = 0;
            }
        }
    }
}

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
    line: Line,
    /// Bytes received and not yet taken.
    pending: Vec<u8>,
    /// How large the messages it sends are now.
    pace: Pace,
    /// Says why, when this machine cut the call the transfer goes with.
    cut: Option<watch::Receiver<Option<String>>>,
}

impl Stream {
    fn over(socket: HubSocket, cut: Option<watch::Receiver<Option<String>>>) -> Self {
        Self { line: Line::new(socket, cut.clone()), pending: Vec::new(), pace: Pace::default(), cut }
    }

    /// Opens this machine's end of the transfer `stream` and waits for the
    /// other end. The hub ends the wait if the other end's machine leaves;
    /// over a direct link, the link's end does.
    pub async fn open(endpoint: &Endpoint, key: &SecretKey, stream: u64, end: End) -> Result<Self, String> {
        let (upstreams, hub, cert) = match endpoint {
            Endpoint::Hub { upstreams, hub, cert } => (upstreams, hub, *cert),
            Endpoint::Peer(way) => {
                let socket = way.open(stream, end).await?;
                return Ok(Self::over(socket, way.cut()));
            }
        };
        Self::to_hub(upstreams, hub, cert, key, |key, proof| Intent::Data { key, proof, stream, end }).await
    }

    /// Opens a data connection the hub itself is the other end of, as
    /// `intent` says: by the first of `upstreams` that reaches it.
    pub(crate) async fn to_hub(
        upstreams: &[Upstream],
        hub: &HubId,
        cert: CertPin,
        key: &SecretKey,
        intent: impl FnOnce(crate::sign::PublicKey, crate::sign::Signature) -> Intent,
    ) -> Result<Self, String> {
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
            intent: intent(key.public(), proof),
        };
        let paired = async {
            dial::send(&mut socket, &hello).await?;
            dial::recv(&mut socket).await
        };
        let paired = tokio::time::timeout(PAIRED_WITHIN, paired)
            .await
            .map_err(|_| format!("the hub paired nothing within {} s", PAIRED_WITHIN.as_secs()))??;
        match paired {
            Some(Down::Paired) => Ok(Self::over(socket, None)),
            Some(Down::Refused { reason }) => Err(reason.to_string()),
            Some(_) => Err("the hub answered something other than a pairing".to_string()),
            None => Err("the hub closed the connection".to_string()),
        }
    }

    /// Why the transfer was cut, if it was.
    fn was_cut(&self) -> Option<String> {
        self.cut.as_ref().and_then(|cut| cut.borrow().clone())
    }

    /// Sends bytes, as many to a message as the line takes (`Pace`).
    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let (piece, after) = rest.split_at(rest.len().min(self.pace.chunk()));
            let began = std::time::Instant::now();
            self.put(WsMessage::Binary(piece.to_vec().into())).await?;
            fastctx_platform::doing::progress();
            self.pace.wrote(piece.len(), began.elapsed());
            rest = after;
        }
        Ok(())
    }

    /// The most a message it sends carries now: what is said in one word
    /// is said whole only when it fits (`crate::watch_stream`).
    pub fn message_size(&self) -> usize {
        self.pace.chunk()
    }

    /// Takes exactly `count` bytes, handing each piece to `take` as it comes.
    pub async fn take(
        &mut self,
        mut count: u64,
        mut take: impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        while count > 0 {
            if self.pending.is_empty() {
                self.pending = self.due(count).await?;
            }
            let here = (self.pending.len() as u64).min(count) as usize;
            take(&self.pending[..here])?;
            fastctx_platform::doing::progress();
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
                self.pending = self.due(count).await?;
            }
            let here = (self.pending.len() as u64).min(count) as usize;
            to.send(&self.pending[..here]).await?;
            self.pending.drain(..here);
            count -= here as u64;
        }
        Ok(())
    }

    /// The next of `count` bytes still due, within `DUE_WITHIN`.
    async fn due(&mut self, count: u64) -> Result<Vec<u8>, String> {
        let deadline = tokio::time::Instant::now() + DUE_WITHIN;
        loop {
            let next = tokio::time::timeout_at(deadline, self.next())
                .await
                .map_err(|_| format!("{count} bytes were still due, and none came for {} s", DUE_WITHIN.as_secs()))?;
            match next? {
                Some(Message::Bytes(bytes)) if bytes.is_empty() => continue,
                Some(Message::Bytes(bytes)) => return Ok(bytes),
                Some(Message::Word(_)) => return Err("the other end said something where bytes were due".to_string()),
                None => return Err(format!("the other end stopped with {count} bytes still due")),
            }
        }
    }

    /// Says something to the other end.
    pub async fn say(&mut self, word: &impl Serialize) -> Result<(), String> {
        self.put(WsMessage::Text(serde_json::to_string(word).expect("words serialize").into())).await
    }

    async fn put(&mut self, message: WsMessage) -> Result<(), String> {
        if let Some(why) = self.was_cut() {
            return Err(why);
        }
        self.line.send(message).await
    }

    /// Waits for the next file protocol step. Transport heartbeats are not
    /// progress; the other end's report of local work toward the step is, as
    /// reading what it already holds can take longer than `DUE_WITHIN`.
    pub async fn hear_step<T: DeserializeOwned>(&mut self, step: &str) -> Result<T, String> {
        let mut progress = self.line.receipt.subscribe();
        let began = tokio::time::Instant::now();
        loop {
            let worked = (*progress.borrow_and_update()).unwrap_or(began).max(began);
            tokio::select! {
                heard = self.hear() => return heard,
                changed = progress.changed() => {
                    changed.map_err(|_| "the other end's work notifications ended".to_string())?;
                },
                () = tokio::time::sleep_until(worked + DUE_WITHIN) => {
                    return Err(format!(
                        "the file transfer's {step} did not come, and the other end reported no work toward it, for {} s",
                        DUE_WITHIN.as_secs()
                    ));
                },
            }
        }
    }

    /// What the other end says next.
    pub async fn hear<T: DeserializeOwned>(&mut self) -> Result<T, String> {
        self.hear_or_end().await?.ok_or_else(|| "the other end left".to_string())
    }

    /// What the other end says next; none once it has closed the connection.
    pub async fn hear_or_end<T: DeserializeOwned>(&mut self) -> Result<Option<T>, String> {
        if !self.pending.is_empty() {
            return Err("bytes came where the other end was to say something".to_string());
        }
        match self.next().await? {
            Some(Message::Word(text)) => {
                serde_json::from_str(&text).map(Some).map_err(|error| format!("what the other end said: {error}"))
            }
            Some(Message::Bytes(_)) => Err("bytes came where the other end was to say something".to_string()),
            None => Ok(None),
        }
    }

    /// Waits until the other end closes or breaks the connection. For an
    /// end that only sends, so it notices the other end has gone while it
    /// has nothing to send; anything the other end says meanwhile is
    /// dropped. Dropping the wait halfway loses nothing else.
    pub async fn closed(&mut self) {
        self.ended().await;
    }

    /// As `closed`, saying why it broke; none when the other end closed it.
    pub async fn ended(&mut self) -> Option<String> {
        loop {
            match self.next().await {
                Ok(Some(_)) => {}
                Ok(None) => return None,
                Err(why) => return Some(why),
            }
        }
    }

    /// As the source: waits for the sink's word that it has everything, and
    /// closes.
    pub async fn finish(mut self) -> Result<(), String> {
        let mut progress = self.line.receipt.subscribe();
        let began = tokio::time::Instant::now();
        let receipt = loop {
            let worked = (*progress.borrow_and_update()).unwrap_or(began).max(began);
            tokio::select! {
                next = self.next() => break next?,
                changed = progress.changed() => {
                    changed.map_err(|_| "the receiver's work notifications ended".to_string())?;
                },
                () = tokio::time::sleep_until(worked + DUE_WITHIN) => {
                    return Err(format!("the final receipt did not arrive and no receiver work was reported for {} s", DUE_WITHIN.as_secs()));
                },
            }
        };
        match receipt {
            Some(Message::Word(text)) if text == DONE => {
                self.line.close().await;
                Ok(())
            }
            Some(Message::Word(text)) => Err(format!("the other end did not take it: {text}")),
            Some(Message::Bytes(_)) => Err("the other end sent bytes instead of its receipt".to_string()),
            None => Err("the other end left before saying it had everything".to_string()),
        }
    }

    /// Closes it the way a WebSocket closes, with nothing more to send: the
    /// other end reads its end, not a cut.
    pub async fn close(self) {
        self.line.close().await;
    }

    /// Keeps the receipt wait alive only while this receiver owns actual local
    /// writes or finalization. Transport heartbeats alone do not renew it.
    pub fn settling(&self) -> Settlement {
        self.line.receipt.hold()
    }

    /// A work handle that can enter a blocking writer without borrowing the stream.
    pub fn receipt_work(&self) -> ReceiptWork {
        ReceiptWork(self.line.receipt.clone())
    }

    /// As the sink: says everything arrived, and closes.
    pub async fn done(mut self) -> Result<(), String> {
        self.put(WsMessage::Text(DONE.into())).await.map_err(|error| format!("sending the receipt: {error}"))?;
        self.line.close().await;
        Ok(())
    }

    async fn next(&mut self) -> Result<Option<Message>, String> {
        self.line.next().await
    }
}

/// Sends `bytes` as the source of the transfer `stream`.
pub async fn source(endpoint: &Endpoint, key: &SecretKey, stream: u64, bytes: &[u8]) -> Result<(), String> {
    let mut stream = Stream::open(endpoint, key, stream, End::Source).await?;
    stream.send(bytes).await?;
    stream.finish().await
}

/// The largest call or answer assembled from a data connection. File transfers
/// use `Stream::take` or `Stream::forward` and are not subject to this limit.
pub const MAX_BODY: usize = 64 * 1024 * 1024;

/// Takes and checks `carried`, reserving its bytes before opening a connection.
/// The returned buffer keeps the process reservation until decoding finishes.
pub async fn sink(
    endpoint: &Endpoint,
    key: &SecretKey,
    carried: &Carried,
) -> Result<crate::reassembly::Buffer, String> {
    sink_until(endpoint, key, carried, tokio::time::Instant::now() + PAIRED_WITHIN).await
}

/// Bounds opening separately; the body is governed by actual byte progress.
pub(crate) async fn sink_until(
    endpoint: &Endpoint,
    key: &SecretKey,
    carried: &Carried,
    opening_deadline: tokio::time::Instant,
) -> Result<crate::reassembly::Buffer, String> {
    if carried.bytes > MAX_BODY as u64 {
        return Err(format!("the remote body is {} bytes; at most {MAX_BODY} can be assembled", carried.bytes));
    }
    // Cover the message waiting on the line and the one being read as well as
    // the body; even an empty advertised body takes space before its stream opens.
    let overhead = 2 * wire::MAX_DATA_MESSAGE;
    let mut bytes = crate::reassembly::Buffer::new(carried.bytes as usize, overhead)?;
    let mut stream = tokio::time::timeout_at(opening_deadline, Stream::open(endpoint, key, carried.stream, End::Sink))
        .await
        .map_err(|_| "opening the body transfer exceeded its deadline".to_string())??;
    stream.take(carried.bytes, |piece| bytes.extend(piece)).await?;
    if Digest(sha256(&bytes)) != carried.sha256 {
        return Err("the bytes are not the ones the source said it sent".to_string());
    }
    // The bytes are whole and the ones the source said it sent: a receipt
    // that cannot be said once the connection is already over leaves only
    // the source unsure, which it says itself, and loses nothing here.
    let _ = stream.done().await;
    Ok(bytes)
}
