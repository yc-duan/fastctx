//! Asset lines, the hub's end (`fastctx_types::asset_lines`): a data
//! connection a member keeps open for the bulk of its asset sync. One
//! exchange goes on a line at a time, in the order the member asks: a
//! request answered as the control connection answers it, frames given or
//! taken with their bodies as bytes, objects given or put whole. Nothing on
//! a line waits for a ticket or for a connection of its own.
//!
//! Each ask is judged as it comes, by the directory as it stands then; an
//! object given is judged again by itself. The hub says something at least
//! every heartbeat while it waits or works, as on every data connection,
//! and takes the member for gone once nothing of its own came for a
//! silence. A line no ask comes on for a while is closed; how many a member
//! has open, and all members together, is bounded.

use super::answering::answer;
use super::objects;
use super::serve::Room;
use crate::origin::Origin;
use crate::updates::transfer::put as write;
use crate::{Hub, Socket, WsMessage, refuse, send};
use fastctx_content::v2::LEASE;
use fastctx_net::data::{Pace, runs};
use fastctx_net::pulse::Pulse;
use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{Down, HEARTBEAT, Refusal, SILENCE};
use fastctx_types::MachineName;
use fastctx_types::asset_lines::{Ask, BytesHead, FRAME_BYTES, FRAMES, OBJECTS, RUN_BYTES, Told};
use fastctx_types::assets::{APART_FRAME_BYTES, ObjectRef as WireObject, Reply, Request};
use futures_util::StreamExt as _;
use futures_util::stream::{SplitSink, SplitStream};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// Lines one member has open at most: a few more than a member opens.
const LINES_OF_ONE: usize = 8;
/// Lines open at most, every member's together.
const LINES: usize = 4096;
/// How long a line no ask comes on stays open: longer than a member keeps
/// one it does not use.
const UNUSED_FOR: Duration = Duration::from_secs(120);
/// The longest ask a line takes.
const ASK_BYTES: u64 = 4 << 20;
/// What members' bytes not kept yet are counted in, and how many of them
/// the hub holds at once, all lines together: frames' bodies and objects on
/// their way in wait here for room, so however many lines send at once the
/// hub holds this much of them and no more.
const TAKEN_UNIT: u64 = 1 << 20;
const TAKEN_UNITS: usize = 256;

/// The asset lines open, by member.
pub(crate) struct Lines {
    open: Mutex<HashMap<MachineName, usize>>,
    /// The room for what members send before the hub has kept it, in
    /// `TAKEN_UNIT`s, all lines together.
    taking: Arc<tokio::sync::Semaphore>,
}

impl Default for Lines {
    fn default() -> Self {
        Self { open: Mutex::new(HashMap::new()), taking: Arc::new(tokio::sync::Semaphore::new(TAKEN_UNITS)) }
    }
}

/// What the hub is told while it is being moved and takes nothing.
const MOVING: &str = "the hub takes nothing now: it is being moved";

/// One line counted among those open, until it is dropped.
struct Open(Arc<Hub>, MachineName);

impl Lines {
    /// Counts one more line of `member`; none when it has as many as it
    /// may, or the hub has.
    /// Room for `bytes` a member is about to send, held until it is dropped.
    async fn room(&self, bytes: u64) -> tokio::sync::OwnedSemaphorePermit {
        let units = bytes.div_ceil(TAKEN_UNIT).clamp(1, TAKEN_UNITS as u64) as u32;
        self.taking.clone().acquire_many_owned(units).await.expect("never closed")
    }

    fn enter(hub: &Arc<Hub>, member: &MachineName) -> Option<Open> {
        let mut open = hub.assets.lines.open.lock().expect("lines");
        if open.values().sum::<usize>() >= LINES || open.get(member).is_some_and(|mine| *mine >= LINES_OF_ONE) {
            return None;
        }
        *open.entry(member.clone()).or_default() += 1;
        Some(Open(hub.clone(), member.clone()))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let mut open = self.0.assets.lines.open.lock().expect("lines");
        if let Some(mine) = open.get_mut(&self.1) {
            *mine -= 1;
            if *mine == 0 {
                open.remove(&self.1);
            }
        }
    }
}

/// The hub's end of the asset line a member opened.
pub(crate) async fn serve(hub: Arc<Hub>, mut socket: Socket, binding: [u8; 32], key: PublicKey, proof: Signature) {
    let me = match hub.member(&key, &proof, &binding) {
        Ok(me) => me,
        Err(reason) => {
            refuse(&mut socket, reason).await;
            return;
        }
    };
    if hub.moving.frozen() {
        refuse(&mut socket, Refusal::ShuttingDown).await;
        return;
    }
    let Some(_open) = Lines::enter(&hub, &me) else {
        refuse(
            &mut socket,
            Refusal::Broken { reason: format!("{me} has as many asset lines open as it may; try again") },
        )
        .await;
        return;
    };
    if tokio::time::timeout(SILENCE, send(&mut socket, &Down::Paired)).await.map_or(true, |sent| sent.is_err()) {
        return;
    }
    let pulse = crate::pulse_of(&socket);
    let (sink, stream) = socket.split();
    let mut beat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut io = Io { sink, stream, pulse, pace: Pace::default(), beat, heard: Instant::now() };
    let origin = Origin { name: me.clone(), key };
    if let Err(why) = run(&hub, &mut io, &origin).await {
        hub.log.line(format_args!("an asset line with {me}: {why}"));
    }
    let _ = tokio::time::timeout(SILENCE, futures_util::SinkExt::close(&mut io.sink)).await;
}

/// The line itself: what is said and the bytes that follow, each way.
struct Io {
    sink: SplitSink<Socket, WsMessage>,
    stream: SplitStream<Socket>,
    pulse: Arc<Pulse>,
    pace: Pace,
    beat: tokio::time::Interval,
    /// When something of the member's own last came.
    heard: Instant,
}

impl Io {
    async fn put(&mut self, message: WsMessage) -> Result<(), String> {
        write(&mut self.sink, &self.pulse, message).await
    }

    /// The next thing the member says or sends, the line's upkeep seen to
    /// meanwhile; none when it closed the line, or `until` came first.
    async fn next(&mut self, until: Option<Instant>) -> Result<Option<WsMessage>, String> {
        loop {
            let ended = async {
                match until {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                next = self.stream.next() => {
                    self.heard = Instant::now();
                    match next {
                        None | Some(Ok(WsMessage::Close(_))) => return Ok(None),
                        Some(Err(error)) => return Err(error.to_string()),
                        Some(Ok(WsMessage::Ping(said))) => {
                            if let Some(took) = fastctx_net::pulse::said(&said) {
                                self.pulse.told(took);
                            }
                        }
                        Some(Ok(WsMessage::Pong(_) | WsMessage::Frame(_))) => {}
                        Some(Ok(message)) => return Ok(Some(message)),
                    }
                }
                _ = self.beat.tick() => self.put(fastctx_net::pulse::ping(&self.pulse)).await?,
                () = tokio::time::sleep_until(self.heard + SILENCE) => match self.pulse.since(self.heard) {
                    Some(at) => self.heard = at,
                    None => return Err(format!("nothing came from the member for {} s", SILENCE.as_secs())),
                },
                () = ended => return Ok(None),
            }
        }
    }

    /// Drives `work`, the hub saying it is there meanwhile.
    async fn working<T>(&mut self, work: impl Future<Output = T>) -> Result<T, String> {
        tokio::pin!(work);
        loop {
            tokio::select! {
                done = &mut work => {
                    // Nothing was listened for meanwhile: that time is not
                    // the member's silence.
                    self.heard = Instant::now();
                    return Ok(done);
                }
                _ = self.beat.tick() => self.put(fastctx_net::pulse::ping(&self.pulse)).await?,
            }
        }
    }

    /// Exactly `length` bytes the member sends, in as many messages as it
    /// likes.
    async fn raw(&mut self, length: u64) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::with_capacity(length as usize);
        while (bytes.len() as u64) < length {
            // The bytes themselves are to keep coming: a member that only
            // says it is there holds what it sent so far no longer.
            match self.next(Some(Instant::now() + SILENCE)).await? {
                Some(WsMessage::Binary(piece)) if bytes.len() as u64 + piece.len() as u64 > length => {
                    return Err("more bytes came than were said".into());
                }
                Some(WsMessage::Binary(piece)) => bytes.extend_from_slice(&piece),
                Some(_) => return Err("words came where bytes were due".into()),
                None => return Err(format!("the member left, or sent none of the bytes due for {} s", SILENCE.as_secs())),
            }
        }
        Ok(bytes)
    }

    /// Sends `bytes`, as many to a message as the line takes.
    async fn pieces(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let (piece, after) = rest.split_at(rest.len().min(self.pace.chunk()));
            let began = std::time::Instant::now();
            self.put(WsMessage::Binary(piece.to_vec().into())).await?;
            self.pace.wrote(piece.len(), began.elapsed());
            rest = after;
        }
        self.heard = Instant::now();
        Ok(())
    }

    /// Says `value`, however long its JSON.
    async fn tell(&mut self, value: &impl Serialize) -> Result<(), String> {
        let json = serde_json::to_vec(value).expect("what is said on a line serializes");
        if json.len() <= runs::SHORT {
            let text = String::from_utf8(json).expect("JSON is UTF-8");
            return self.put(WsMessage::Text(text.into())).await;
        }
        let (head, packed) = runs::long(&json);
        self.put(WsMessage::Text(serde_json::to_string(&head).expect("a head serializes").into())).await?;
        self.pieces(packed.as_deref().unwrap_or(&json)).await
    }

    /// The value `text` is, or is the head of.
    async fn value<T: DeserializeOwned>(&mut self, text: &str, most: u64) -> Result<T, String> {
        let Some(head) = runs::long_head(text) else {
            return serde_json::from_str(text).map_err(|error| format!("what the member said: {error}"));
        };
        if head.long > most || head.packed > most {
            return Err(format!("the member said {} bytes at once; at most {most} are taken", head.long));
        }
        let travelling = if head.packed > 0 { head.packed } else { head.long };
        let bytes = self.raw(travelling).await?;
        let json = if head.packed > 0 { runs::unpack(&bytes, head.long)? } else { bytes };
        serde_json::from_slice(&json).map_err(|error| format!("what the member said: {error}"))
    }

    /// The next ask; none when the member closed the line, the line ended,
    /// or no ask came for as long as an unused line is kept.
    async fn ask(&mut self) -> Result<Option<Ask>, String> {
        let text = match self.next(Some(Instant::now() + UNUSED_FOR)).await {
            Ok(Some(WsMessage::Text(text))) => text,
            Ok(Some(_)) => return Err("bytes came where an ask was due".into()),
            // Between two exchanges a line that ends, however it ends, owes
            // nothing.
            Ok(None) | Err(_) => return Ok(None),
        };
        self.value(text.as_str(), ASK_BYTES).await.map(Some)
    }

    /// Sends a run of bytes under its head; `plain` those known not to
    /// compress.
    async fn give(&mut self, bytes: &[u8], plain: bool) -> Result<(), String> {
        let (head, packed) = runs::run(bytes, plain);
        self.put(WsMessage::Text(serde_json::to_string(&head).expect("a head serializes").into())).await?;
        self.pieces(packed.as_deref().unwrap_or(bytes)).await
    }

    /// Takes a run of at most `most` bytes.
    async fn given(&mut self, most: u64) -> Result<Vec<u8>, String> {
        let head: BytesHead = match self.next(None).await? {
            Some(WsMessage::Text(text)) => {
                serde_json::from_str(text.as_str()).map_err(|error| format!("a run's head: {error}"))?
            }
            Some(_) => return Err("bytes came where a run's head was due".into()),
            None => return Err("the member left before it sent it all".into()),
        };
        if head.bytes > most || head.packed > most {
            return Err(format!("a run of {} bytes came; at most {most} were due", head.bytes));
        }
        let travelling = if head.packed > 0 { head.packed } else { head.bytes };
        let bytes = self.raw(travelling).await?;
        if head.packed > 0 { runs::unpack(&bytes, head.bytes) } else { Ok(bytes) }
    }
}

/// Runs `work` on a thread that may block once this member's turn among the
/// hub's asset work comes (`work`), the hub saying it is there meanwhile.
/// The turn, the room `room` holds and the hub's count of what may change
/// its records go with the work itself: they are let go when it ends, on a
/// line that broke meanwhile too, so what a member left behind is counted
/// against what it asks next. Nothing is run while the hub is being moved.
async fn blocked<T: Send + 'static>(
    hub: &Arc<Hub>,
    io: &mut Io,
    origin: &Origin,
    room: Option<tokio::sync::OwnedSemaphorePermit>,
    work: impl FnOnce(Arc<Hub>) -> T + Send + 'static,
) -> Result<T, String> {
    let Some(held) = crate::moving::Moving::enter(hub) else { return Err(MOVING.into()) };
    let turn = io.working(hub.assets.work.wait(&origin.name)).await?;
    io.working(hub.blocking(move |hub| {
        let _kept = (turn, held, room);
        work(hub)
    }))
    .await
}

/// Answers `request` as the control connection would, with `room`, and
/// tells whom the answer gives something to be told.
async fn answered(
    hub: &Arc<Hub>,
    io: &mut Io,
    origin: &Origin,
    request: Request,
    room: Room,
    taken: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Result<Result<Reply, String>, String> {
    let asker = origin.clone();
    let (reply, pushes) = blocked(hub, io, origin, taken, move |hub| answer(&hub, &asker, request, room)).await?;
    for (machine, push) in pushes {
        hub.tell(&machine, Down::Assets { push });
    }
    Ok(reply)
}

/// Says what a request was answered.
async fn told(io: &mut Io, reply: Result<Reply, String>) -> Result<(), String> {
    match reply {
        Ok(reply) => io.tell(&Told::Reply { reply }).await,
        Err(why) => io.tell(&Told::Failed { why }).await,
    }
}

/// Takes `frames` of `scope` a few at a time, as many as the control
/// connection takes at once, so no more of them than that are opened into
/// their records together: the hub's word on each in order, up to the first
/// it did not take.
async fn take(
    hub: &Arc<Hub>,
    io: &mut Io,
    origin: &Origin,
    scope: String,
    frames: Vec<fastctx_types::assets::Frame>,
    taken: tokio::sync::OwnedSemaphorePermit,
) -> Result<Result<Reply, String>, String> {
    let taken = Arc::new(taken);
    let mut words = Vec::with_capacity(frames.len());
    let mut frames = frames.into_iter().peekable();
    while frames.peek().is_some() {
        let some: Vec<_> = frames.by_ref().take(fastctx_types::assets::FRAMES_A_TAKE).collect();
        let sent = some.len();
        let asker = origin.clone();
        let (within, kept) = (scope.clone(), taken.clone());
        let (reply, pushes) = blocked(hub, io, origin, None, move |hub| {
            let _room = kept;
            answer(&hub, &asker, Request::TakeMany { scope: within, frames: some }, Room::Line)
        })
        .await?;
        for (machine, push) in pushes {
            hub.tell(&machine, Down::Assets { push });
        }
        match reply {
            Ok(Reply::ReceivedMany { received }) => {
                let all = received.len() == sent;
                words.extend(received);
                if !all {
                    break;
                }
            }
            // What stopped it is told where nothing before it was taken;
            // else what was taken is, and the member hears of the rest by
            // sending it again.
            other if words.is_empty() => return Ok(other),
            _ => break,
        }
    }
    Ok(Ok(Reply::ReceivedMany { received: words }))
}

/// One exchange after another, until the member closes the line or it is
/// left unused.
async fn run(hub: &Arc<Hub>, io: &mut Io, origin: &Origin) -> Result<(), String> {
    loop {
        let Some(ask) = io.ask().await? else { return Ok(()) };
        match ask {
            Ask::Request { request } => {
                let reply = answered(hub, io, origin, request, Room::Control, None).await?;
                told(io, reply).await?;
            }
            Ask::Frames { scope, stream, from, frames } => {
                let request = Request::Frames { scope: scope.clone(), stream, from, limit: frames };
                match answered(hub, io, origin, request, Room::Line, None).await? {
                    Ok(Reply::Frames { mut frames, .. }) => {
                        let mut bodies = Vec::with_capacity(frames.iter().map(|frame| frame.body.0.len()).sum());
                        for frame in &mut frames {
                            frame.apart = frame.body.0.len() as u64;
                            bodies.append(&mut frame.body.0);
                        }
                        io.tell(&Told::Frames { frames }).await?;
                        // Bodies that take a while to give are given only to
                        // one that may still read the scope, judged again as
                        // each body sent apart is.
                        if bodies.len() as u64 > RUN_BYTES {
                            let (asker, key) = (origin.clone(), scope);
                            let still = blocked(hub, io, origin, None, move |hub| objects::line_scope(&hub, &asker, &key)).await?;
                            if !matches!(still, Ok(Ok(_))) {
                                return Err("the scope was withdrawn as its frames were given".into());
                            }
                        }
                        // A frame's records are compressed as they are kept.
                        io.give(&bodies, true).await?;
                    }
                    reply => told(io, reply).await?,
                }
            }
            Ask::Take { scope, mut frames } => {
                // The bodies are on their way already: what cannot be taken
                // ends the line, as nothing after it could be told apart.
                let due: u64 = frames.iter().map(|frame| frame.apart).sum();
                if frames.len() > FRAMES
                    || due > FRAME_BYTES as u64
                    || frames.iter().any(|frame| !frame.body.0.is_empty() || frame.apart > APART_FRAME_BYTES)
                {
                    return Err("a take carried more than a line's exchange does".into());
                }
                if hub.moving.frozen() {
                    return Err(MOVING.into());
                }
                // They wait for room among what the hub holds unkept.
                let taken = io.working(hub.assets.lines.room(due)).await?;
                let bodies = io.given(FRAME_BYTES as u64).await?;
                if bodies.len() as u64 != due {
                    return Err("the bodies of a take are not as long as its frames said".into());
                }
                let mut rest = bodies.as_slice();
                for frame in &mut frames {
                    let (body, after) = rest.split_at(frame.apart as usize);
                    frame.body = body.to_vec().into();
                    frame.apart = 0;
                    rest = after;
                }
                drop(bodies);
                let reply = take(hub, io, origin, scope, frames, taken).await?;
                told(io, reply).await?;
            }
            Ask::Objects { scope, objects } => give_objects(hub, io, origin, scope, objects).await?,
            Ask::Put { scope, object, prove } => take_object(hub, io, origin, scope, object, prove).await?,
        }
    }
}

/// Gives each of `wanted` of `scope` it may, in order: its word, then its
/// bytes a run at a time.
async fn give_objects(
    hub: &Arc<Hub>,
    io: &mut Io,
    origin: &Origin,
    scope: String,
    wanted: Vec<WireObject>,
) -> Result<(), String> {
    if wanted.is_empty() || wanted.len() > OBJECTS {
        return io.tell(&Told::Failed { why: format!("objects are asked for 1 to {OBJECTS} at a time") }).await;
    }
    let (asker, key) = (origin.clone(), scope.clone());
    let kept = match blocked(hub, io, origin, None, move |hub| objects::line_scope(&hub, &asker, &key)).await? {
        Ok(Ok(kept)) => kept,
        Ok(Err(reply)) => return io.tell(&Told::Reply { reply }).await,
        Err(why) => return io.tell(&Told::Failed { why }).await,
    };
    for (index, object) in wanted.into_iter().enumerate() {
        let index = index as u32;
        let (asker, of) = (origin.clone(), kept.clone());
        let found = blocked(hub, io, origin, None, move |hub| objects::line_object(&hub, &asker, &of, &object)).await?;
        let (library, object) = match found {
            Ok(found) => found,
            Err(why) => {
                io.tell(&Told::Object { index, given: false, why }).await?;
                continue;
            }
        };
        io.tell(&Told::Object { index, given: true, why: String::new() }).await?;
        let mut offset = 0;
        while offset < object.size {
            let length = RUN_BYTES.min(object.size - offset) as u32;
            let reading = library.clone();
            let bytes = blocked(hub, io, origin, None, move |_| reading.read(&object.hash, offset, length))
                .await?
                .map_err(|error| error.to_string())?;
            if bytes.is_empty() {
                return Err("an object the hub holds ended before its length".into());
            }
            io.give(&bytes, false).await?;
            offset += bytes.len() as u64;
        }
    }
    Ok(())
}

/// Takes an object put, or proved by its bytes: says whether its bytes are
/// wanted, takes them, and says whether the hub holds it.
async fn take_object(
    hub: &Arc<Hub>,
    io: &mut Io,
    origin: &Origin,
    scope: String,
    object: WireObject,
    prove: bool,
) -> Result<(), String> {
    let asker = origin.clone();
    let asked = blocked(hub, io, origin, None, move |hub| objects::line_put(&hub, &asker, &scope, &object, prove)).await?;
    let (library, object) = match asked {
        Ok(Ok(Some(wanted))) => wanted,
        Ok(Ok(None)) => return io.tell(&Told::Held).await,
        Ok(Err(reply)) => return io.tell(&Told::Reply { reply }).await,
        Err(why) => return io.tell(&Told::Failed { why }).await,
    };
    // Its bytes wait for room among what the hub holds unkept.
    let taken = io.working(hub.assets.lines.room(object.size)).await?;
    io.tell(&Told::Send).await?;
    // What the device proved and staged stays while this one comes (AA05).
    hub.assets.objects.keep_alive(origin.key.as_bytes());
    let mut bytes = Vec::with_capacity(object.size as usize);
    while (bytes.len() as u64) < object.size {
        let run = io.given(RUN_BYTES.min(object.size - bytes.len() as u64)).await?;
        if run.is_empty() {
            return Err("an object's bytes ended before its length".into());
        }
        bytes.extend_from_slice(&run);
    }
    let holder = format!("upload:{}", origin.name);
    let kept = blocked(hub, io, origin, Some(taken), move |_| -> Result<(), String> {
            if prove {
                // Every byte, hashed: nothing is kept again.
                if <[u8; 32]>::from(Sha256::digest(&bytes)) != object.hash {
                    return Err("the bytes sent do not hash to the object".into());
                }
                return Ok(());
            }
            let lease = library.put(&mut bytes.as_slice(), &holder, LEASE).map_err(|error| error.to_string())?;
            if lease.object != object {
                return Err("the bytes sent do not hash to the object".into());
            }
            Ok(())
        })
        .await?;
    match kept {
        Ok(()) => {
            // Every byte of it sent proves holding as well as a proof does.
            hub.assets.objects.proved(origin.key.as_bytes(), &object);
            io.tell(&Told::Held).await
        }
        Err(why) => io.tell(&Told::Failed { why }).await,
    }
}
