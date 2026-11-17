//! Asset lines to the hub (`fastctx_types::asset_lines`): data connections
//! this machine keeps open for the bulk of its asset sync. One is taken for
//! an exchange or a run of them and kept for the next, so what goes on it
//! costs no connection of its own; several are open at once, and what goes
//! on one holds up nothing on another.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::asset_lines::{Ask, FRAME_BYTES, RUN_BYTES, Told};
use fastctx_types::assets::{Frame, LINES, ObjectRef, Reply, Request};

use super::Link;
use crate::data::Stream;

/// Lines one link has open at most: as many exchanges go on at once.
pub const LINES_AT_ONCE: usize = 6;

/// How long a line nothing uses is kept open. The hub lets an unused line
/// go later than this, so one kept here is still open there.
const KEPT_FOR: Duration = Duration::from_secs(45);

/// The lines of one link: those open and unused, and the room for more.
pub(crate) struct Lines {
    unused: Mutex<Vec<(Stream, Instant)>>,
    room: Arc<tokio::sync::Semaphore>,
}

impl Default for Lines {
    fn default() -> Self {
        Self { unused: Mutex::new(Vec::new()), room: Arc::new(tokio::sync::Semaphore::new(LINES_AT_ONCE)) }
    }
}

/// An asset line taken for use: kept open for the next use when it is let
/// go between two exchanges, closed when it is let go within one.
pub struct AssetLine {
    stream: Option<Stream>,
    lines: Arc<Lines>,
    _room: tokio::sync::OwnedSemaphorePermit,
    /// Whether nothing of an exchange is under way on it.
    between: bool,
}

/// What came of one of the objects asked for on a line.
pub enum Given {
    /// Its bytes, every one.
    Bytes(Vec<u8>),
    /// The hub did not give it, and why.
    Withheld(String),
    /// The hub gives none of them: what it answered of the scope itself,
    /// revoked or sealed, or why it refused the ask.
    Refused(Result<Reply, String>),
}

/// What a put asked on a line is answered.
pub enum PutAsked {
    /// The hub holds the object: nothing is sent.
    Held,
    /// The hub wants its bytes (`AssetLine::put_run`, `put_done`).
    Send,
    /// The hub answered as it would any request: a scope revoked, the assets
    /// sealed.
    Reply(Reply),
}

impl Link {
    /// Whether the hub keeps asset lines.
    pub fn keeps_asset_lines(&self) -> bool {
        self.asset_version() >= LINES
    }

    /// Takes an asset line: one open and unused, or a new one; waits while
    /// as many as this link may have are in use.
    pub async fn asset_line(&self) -> Result<AssetLine, String> {
        if !self.keeps_asset_lines() {
            return Err("the hub keeps no asset lines".into());
        }
        let lines = self.inner.lines.clone();
        let room = lines.room.clone().acquire_owned().await.map_err(|_| "the link's asset lines ended".to_string())?;
        let kept = {
            let mut unused = lines.unused.lock().expect("never poisoned");
            unused.retain(|(stream, since)| since.elapsed() < KEPT_FOR && !stream.ended_already());
            unused.pop()
        };
        let stream = match kept {
            Some((stream, _)) => stream,
            None => {
                let crate::data::Endpoint::Hub { upstreams, hub, cert } = self.inner.data_endpoint() else {
                    return Err("asset lines go to the hub itself".into());
                };
                Stream::to_hub(&upstreams, &hub, cert, &self.inner.key, |key, proof| crate::wire::Intent::AssetLine {
                    key,
                    proof,
                })
                .await?
            }
        };
        Ok(AssetLine { stream: Some(stream), lines, _room: room, between: true })
    }
}

impl Drop for AssetLine {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take()
            && self.between
            && !stream.ended_already()
        {
            self.lines.unused.lock().expect("never poisoned").push((stream, Instant::now()));
        }
    }
}

impl AssetLine {
    /// The line, an exchange beginning on it.
    fn begin(&mut self) -> &mut Stream {
        self.between = false;
        self.stream.as_mut().expect("held until dropped")
    }

    /// Asks the hub `request`, answered as on the control connection.
    pub async fn ask(&mut self, request: Request) -> Result<Reply, String> {
        let stream = self.begin();
        stream.tell(&Ask::Request { request }).await?;
        let told: Told = stream.told("the hub's answer").await?;
        match told {
            Told::Reply { reply } => {
                self.between = true;
                Ok(reply)
            }
            Told::Failed { why } => {
                self.between = true;
                Err(why)
            }
            _ => Err("the hub answered a request with something else".into()),
        }
    }

    /// Up to `frames` frames of `stream` of `scope` from the one holding
    /// `from`, each whole: `Reply::Frames`, or what the hub answers of a
    /// scope revoked or assets sealed.
    pub async fn frames(&mut self, scope: &str, stream: &str, from: u64, frames: u32) -> Result<Reply, String> {
        let ask = Ask::Frames { scope: scope.to_string(), stream: stream.to_string(), from, frames };
        let line = self.begin();
        line.tell(&ask).await?;
        let told: Told = line.told("the hub's frames").await?;
        match told {
            Told::Frames { mut frames } => {
                let bodies = line
                    .given(FRAME_BYTES as u64 + fastctx_types::assets::APART_FRAME_BYTES, "the frames' bodies")
                    .await?;
                let mut rest = bodies.as_slice();
                for frame in &mut frames {
                    let length = frame.apart as usize;
                    if !frame.body.0.is_empty() || length > rest.len() {
                        return Err("the frames' bodies are not as long as the frames said".into());
                    }
                    let (body, after) = rest.split_at(length);
                    frame.body = body.to_vec().into();
                    frame.apart = 0;
                    rest = after;
                }
                if !rest.is_empty() {
                    return Err("more bytes of bodies came than the frames said".into());
                }
                self.between = true;
                Ok(Reply::Frames { frames, ticket: 0 })
            }
            Told::Reply { reply } => {
                self.between = true;
                Ok(reply)
            }
            Told::Failed { why } => {
                self.between = true;
                Err(why)
            }
            _ => Err("the hub answered an ask for frames with something else".into()),
        }
    }

    /// Asks for `objects` of `scope`: each is then taken with `object`, in
    /// the order they are named here.
    pub async fn ask_objects(&mut self, scope: &str, objects: &[ObjectRef]) -> Result<(), String> {
        let ask = Ask::Objects { scope: scope.to_string(), objects: objects.to_vec() };
        self.begin().tell(&ask).await
    }

    /// The object at `index` of those asked for, which is `object`; `last`
    /// when no other follows it. After an answer of the scope itself, or a
    /// refusal of the whole ask, none follows.
    pub async fn object(&mut self, object: &ObjectRef, index: usize, last: bool) -> Result<Given, String> {
        let line = self.stream.as_mut().expect("held until dropped");
        let told: Told = line.told("the hub's word on an object").await?;
        match told {
            Told::Object { index: said, given, why } if said as usize == index => {
                if !given {
                    self.between = last;
                    return Ok(Given::Withheld(why));
                }
                let mut bytes = Vec::with_capacity(object.size as usize);
                while (bytes.len() as u64) < object.size {
                    let run = line.given(RUN_BYTES.min(object.size - bytes.len() as u64), "an object's bytes").await?;
                    if run.is_empty() {
                        return Err("an object's bytes ended before its length".into());
                    }
                    bytes.extend_from_slice(&run);
                }
                self.between = last;
                Ok(Given::Bytes(bytes))
            }
            Told::Reply { reply } if index == 0 => {
                self.between = true;
                Ok(Given::Refused(Ok(reply)))
            }
            Told::Failed { why } if index == 0 => {
                self.between = true;
                Ok(Given::Refused(Err(why)))
            }
            _ => Err("the hub's words on the objects do not come one for each, in order".into()),
        }
    }

    /// Asks to put `object` of `scope`, or to prove holding it by its bytes.
    pub async fn put(&mut self, scope: &str, object: &ObjectRef, prove: bool) -> Result<PutAsked, String> {
        let ask = Ask::Put { scope: scope.to_string(), object: object.clone(), prove };
        let line = self.begin();
        line.tell(&ask).await?;
        let told: Told = line.told("the hub's word on a put").await?;
        match told {
            Told::Held => {
                self.between = true;
                Ok(PutAsked::Held)
            }
            // The exchange goes on with the object's bytes.
            Told::Send => Ok(PutAsked::Send),
            Told::Reply { reply } => {
                self.between = true;
                Ok(PutAsked::Reply(reply))
            }
            Told::Failed { why } => {
                self.between = true;
                Err(why)
            }
            _ => Err("the hub answered a put with something else".into()),
        }
    }

    /// Sends a run of the bytes of the object a put was answered `Send`
    /// for: at most `RUN_BYTES` of them, in order.
    pub async fn put_run(&mut self, run: &[u8]) -> Result<(), String> {
        self.stream.as_mut().expect("held until dropped").give(run, false).await
    }

    /// Hears the hub took the object every byte of which went.
    pub async fn put_done(&mut self) -> Result<(), String> {
        let line = self.stream.as_mut().expect("held until dropped");
        let told: Told = line.told("the hub's word that it has the object").await?;
        match told {
            Told::Held => {
                self.between = true;
                Ok(())
            }
            Told::Failed { why } => {
                self.between = true;
                Err(why)
            }
            _ => Err("the hub answered an object's bytes with something else".into()),
        }
    }

    /// Gives the hub `frames` of one stream of `scope`, in order, each
    /// whole: answered as `Request::TakeMany` is.
    pub async fn take(&mut self, scope: &str, frames: &[Frame]) -> Result<Reply, String> {
        let mut bodies = Vec::with_capacity(frames.iter().map(|frame| frame.body.0.len()).sum());
        let mut heads = Vec::with_capacity(frames.len());
        for frame in frames {
            bodies.extend_from_slice(&frame.body.0);
            heads.push(Frame {
                header: frame.header.clone(),
                signature: frame.signature.clone(),
                body: Default::default(),
                apart: frame.body.0.len() as u64,
            });
        }
        let line = self.begin();
        line.tell(&Ask::Take { scope: scope.to_string(), frames: heads }).await?;
        // A frame's records are compressed as they are kept.
        line.give(&bodies, true).await?;
        let told: Told = line.told("the hub's word on the frames").await?;
        match told {
            Told::Reply { reply } => {
                self.between = true;
                Ok(reply)
            }
            Told::Failed { why } => {
                self.between = true;
                Err(why)
            }
            _ => Err("the hub answered frames given with something else".into()),
        }
    }
}
