//! Original file bytes for interfaces, over a local IPC connection or a
//! device's authenticated data stream. Dropping a read cancels it; bounded
//! queues propagate backpressure to the source.
//!
//! A read is not over because its way broke (`world-network.md`, 断开不是
//! 结束). It is opened again at the pace of everything that tries again
//! (`crate::again`), from the byte where it broke, as long as the machine
//! names the same version of the file (`FileReadInfo::version`); a file that
//! changed between two tries is read again from the start of what was
//! asked, which `Read::Opened` tells the reader. What the machine says of
//! the file itself ends it: it is not there, cannot be read, or changed
//! while a try read it. Before the file first opens, only a way that broke
//! is tried again; once it opened, so is a machine that went away or refused
//! for now, for it may come back.

use fastctx_types::access::Written;
use std::io;

use fastctx_net::file_read::Part;
use fastctx_types::attachments::AttachmentSource;
use fastctx_types::file_read::{FileRange, FileReadComplete, FileReadInfo, FileReadPart, ReadFileRequest};
use fastctx_types::ipc::{ClientFrame, NodeFrame};
use fastctx_types::{Call, Reply};
use tokio::sync::watch;

use crate::again::{Again, Interruption};
use crate::{CallError, Client, ConnectError, Error, World};

impl World {
    /// The verified original of a file the user sent, or the picture its
    /// host kept, read as `read_file` reads.
    pub fn read_attachment(&self, attachment: impl Into<AttachmentSource>) -> Reading {
        Reading::new(self.clone(), Asked::Attachment(attachment.into()))
    }

    /// A file on `machine` (this node when absent), read as the user. A
    /// device has no local node and must name the machine holding the file.
    /// Nothing is asked until `Reading::next`; consume it through its
    /// terminal result, for bytes handed out before that are provisional if
    /// the read fails.
    pub fn read_file(&self, machine: Option<Written>, request: ReadFileRequest) -> Reading {
        Reading::new(self.clone(), Asked::File { machine, request })
    }
}

/// What a read hands out next.
#[derive(Debug)]
pub enum Read {
    /// The file as the read opened it, before any of its bytes. It comes
    /// again when the file changed while the read was broken off: the bytes
    /// handed out before it are void, and those after it are what was asked
    /// of the file as it is now, from the start.
    Opened(FileReadInfo),
    /// The next bytes, which begin at `info().offset + received()` as it was
    /// before they came.
    Bytes(Vec<u8>),
}

/// How a read is going, for an interface that shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// The bytes handed out of what was asked.
    pub received: u64,
    /// How many bytes were asked of the file, once it opened.
    pub length: Option<u64>,
    /// Why the read is broken off now, and since when.
    pub interrupted: Option<Interruption>,
}

/// A read's opening metadata and its original byte chunks. No whole-file
/// buffer is retained. `next` is not cancellation-safe: cancel by dropping
/// the entire `Reading`, rather than abandoning a partially received frame.
pub struct Reading {
    world: World,
    asked: Asked,
    /// The try under way, once asked.
    source: Option<Source>,
    /// The version being read, as the try that began it opened it.
    opened: Option<FileReadInfo>,
    /// What the try under way opened.
    this_try: Option<FileReadInfo>,
    /// The bytes of `opened` handed out, over all its tries.
    received: u64,
    /// `received` when the try under way was asked.
    at_try: u64,
    /// Whether the file opened once: the machine was reached and the read
    /// is the user's to have.
    began: bool,
    complete: Option<FileReadComplete>,
    ended: bool,
    again: Again,
    progress: watch::Sender<Progress>,
}

#[derive(Clone)]
enum Asked {
    File { machine: Option<Written>, request: ReadFileRequest },
    Attachment(AttachmentSource),
}

/// What a step of a read came to, short of its end.
enum Step {
    Handed(Read),
    /// Nothing to hand out yet: a try went on from where the last broke,
    /// or the read starts over.
    Again,
}

impl Reading {
    fn new(world: World, asked: Asked) -> Self {
        Self {
            world,
            asked,
            source: None,
            opened: None,
            this_try: None,
            received: 0,
            at_try: 0,
            began: false,
            complete: None,
            ended: false,
            again: Again::default(),
            progress: watch::channel(Progress::default()).0,
        }
    }

    /// The file as the read opened it, once it did.
    pub fn info(&self) -> Option<&FileReadInfo> {
        self.opened.as_ref()
    }

    /// How many bytes of the version being read have been handed out,
    /// including after a failed read. Their offsets start at `info().offset`.
    pub fn received(&self) -> u64 {
        self.received
    }

    /// Present only after `next` returned `Ok(None)` with verified success.
    pub fn complete(&self) -> Option<&FileReadComplete> {
        self.complete.as_ref()
    }

    /// How the read goes from now on, each change as it happens.
    pub fn progress(&self) -> watch::Receiver<Progress> {
        self.progress.subscribe()
    }

    /// The next part of the file. `Ok(None)` means a verified end, never
    /// merely EOF. A failure ends the read; calling again after one is an
    /// error, so failure cannot look like success.
    pub async fn next(&mut self) -> Result<Option<Read>, Error> {
        if self.ended {
            return if self.complete.is_some() {
                Ok(None)
            } else {
                Err(Error::Other("this file read already failed".into()))
            };
        }
        loop {
            let error = match self.step().await {
                Ok(Some(Step::Handed(read))) => return Ok(Some(read)),
                Ok(Some(Step::Again)) => continue,
                Ok(None) => {
                    self.ended = true;
                    return Ok(None);
                }
                Err(error) => error,
            };
            self.source = None;
            self.this_try = None;
            if !self.goes_on(&error) {
                self.ended = true;
                return Err(error);
            }
            let interruption = self.again.broke(error.to_string());
            self.progress.send_modify(|progress| progress.interrupted = Some(interruption));
            // Asked for in its own statement: the wait owns all it needs,
            // and the read is held by nothing across it.
            let pause = self.again.pause(&self.world, self.machine().as_ref());
            pause.await;
        }
    }

    async fn step(&mut self) -> Result<Option<Step>, Error> {
        if self.source.is_none() {
            let range = self.range();
            let source = ask(&self.world, &self.asked, range).await?;
            self.source = Some(source);
            self.at_try = self.received;
        }
        let source = self.source.as_mut().expect("asked above");
        match source.next().await? {
            Part::Opened(info) => self.opened(info).map(Some),
            Part::Chunk { offset, bytes } => self.chunk(offset, bytes).map(|read| Some(Step::Handed(read))),
            Part::Finished(Ok(complete)) => self.finished(complete).map(|()| None),
            Part::Finished(Err(error)) => {
                if error.bytes != self.received - self.at_try {
                    return Err(unexpected("the file read's failure disagrees with the received byte count"));
                }
                Err(error.into())
            }
        }
    }

    /// What the next try asks for: the rest of the version being read, or
    /// what was asked, when nothing of a version is had.
    fn range(&self) -> FileRange {
        match (&self.opened, &self.asked) {
            (Some(opened), _) => {
                FileRange::Range { offset: opened.offset + self.received, bytes: opened.length - self.received }
            }
            (None, Asked::File { request, .. }) => request.range.clone(),
            (None, Asked::Attachment(_)) => FileRange::All,
        }
    }

    fn opened(&mut self, info: FileReadInfo) -> Result<Step, Error> {
        if self.this_try.is_some() {
            return Err(unexpected("the file read opened twice"));
        }
        if info.offset > info.file.size || info.length > info.file.size - info.offset {
            return Err(unexpected("the node selected a range outside the observed file"));
        }
        if self.again.came() {
            self.progress.send_modify(|progress| progress.interrupted = None);
        }
        if let Some(opened) = &self.opened {
            let same = opened.version.is_some()
                && info.version == opened.version
                && info.offset == opened.offset + self.received
                && info.length == opened.length - self.received;
            if same {
                self.this_try = Some(info);
                return Ok(Step::Again);
            }
            // Changed since the try that broke, or its machine cannot say
            // which version it opened: what was asked, from the start.
            self.opened = None;
            self.received = 0;
            self.source = None;
            return Ok(Step::Again);
        }
        self.began = true;
        self.opened = Some(info.clone());
        self.this_try = Some(info.clone());
        let length = info.length;
        self.progress.send_modify(|progress| {
            progress.received = 0;
            progress.length = Some(length);
        });
        Ok(Step::Handed(Read::Opened(info)))
    }

    fn chunk(&mut self, offset: u64, bytes: Vec<u8>) -> Result<Read, Error> {
        let Some(opened) = self.opened.as_ref().filter(|_| self.this_try.is_some()) else {
            return Err(unexpected("the file read sent bytes before it opened"));
        };
        if offset != opened.offset + self.received
            || bytes.is_empty()
            || bytes.len() as u64 > opened.length - self.received
        {
            return Err(unexpected("the file read sent bytes outside the selected range or out of order"));
        }
        self.received += bytes.len() as u64;
        let received = self.received;
        self.progress.send_modify(|progress| progress.received = received);
        Ok(Read::Bytes(bytes))
    }

    fn finished(&mut self, complete: FileReadComplete) -> Result<(), Error> {
        let (Some(opened), Some(this_try)) = (&self.opened, &self.this_try) else {
            return Err(unexpected("the file read finished before it opened"));
        };
        if complete.bytes != self.received - self.at_try
            || self.received != opened.length
            || complete.file != this_try.file
        {
            return Err(unexpected("the file read's completion disagrees with the received bytes or metadata"));
        }
        self.complete = Some(complete);
        Ok(())
    }

    /// Whether the read goes on after `error`. It does when its way broke;
    /// once the file opened, also when the machine went away or refuses for
    /// now, as a stream that broke does. It never does for what the machine
    /// said of the file, for what came wrong, or for a node of another
    /// version.
    fn goes_on(&self, error: &Error) -> bool {
        match error {
            Error::Connect(ConnectError::Refused { .. }) => false,
            Error::Connect(_) | Error::Call(CallError::Broken(_)) => true,
            Error::Call(CallError::Failed(failure)) => failure.is_unanswered() || self.began,
            _ => false,
        }
    }

    /// The machine the file is on, whose return ends a wait early; none
    /// for this computer's node.
    fn machine(&self) -> Option<Written> {
        match &self.asked {
            Asked::File { machine, .. } => machine.clone(),
            Asked::Attachment(attachment) => Some(attachment.owner().machine.clone().into()),
        }
    }
}

/// Asks `world` for `range` of what `asked` names: a try of the read. It
/// borrows no more than it reads, so a read is held across none of it.
async fn ask(world: &World, asked: &Asked, range: FileRange) -> Result<Source, Error> {
    if let Some(device) = world.device() {
        let reading = match asked {
            Asked::File { machine, request } => {
                device.read_file(machine.clone(), ReadFileRequest { path: request.path.clone(), range }).await?
            }
            Asked::Attachment(attachment) => device.read_attachment(attachment.clone(), range).await?,
        };
        return Ok(Source::Device(reading));
    }
    let mut client = world.connect().await?;
    client.next += 1;
    let id = client.next;
    let call = match asked {
        Asked::File { machine, request } => {
            Call::ReadFile { machine: machine.clone(), request: ReadFileRequest { path: request.path.clone(), range } }
        }
        Asked::Attachment(attachment) => Call::ReadAttachment { attachment: attachment.clone(), range },
    };
    client.connection.send(&crate::to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
    Ok(Source::Local { client, id })
}

/// One try of a read: an exclusive connection to this computer's node, or
/// the device's own read of the machine.
enum Source {
    Local { client: Client, id: u64 },
    Device(crate::device::file_read::Reading),
}

impl Source {
    async fn next(&mut self) -> Result<Part, Error> {
        let (client, id) = match self {
            Self::Device(reading) => return reading.next().await,
            Self::Local { client, id } => (client, *id),
        };
        match serde_json::from_slice(&frame(client).await?)
            .map_err(|e| unexpected(&format!("invalid file-read frame: {e}")))?
        {
            NodeFrame::FileRead { id: answered, part } if answered == id => match part {
                FileReadPart::Opened { info } => Ok(Part::Opened(info)),
                FileReadPart::Chunk { offset, bytes } => {
                    if bytes == 0 || bytes > fastctx_net::wire::MAX_DATA_MESSAGE as u64 {
                        return Err(unexpected("the node declared an invalid file-read block size"));
                    }
                    let data = frame(client).await?;
                    if data.len() as u64 != bytes {
                        return Err(unexpected("the binary block's length differs from its header"));
                    }
                    Ok(Part::Chunk { offset, bytes: data })
                }
            },
            NodeFrame::Reply { id: answered, reply } if answered == id => match reply.map_err(CallError::Failed)? {
                Reply::FileRead { result } => Ok(Part::Finished(result)),
                _ => Err(unexpected("the node answered something other than a file read")),
            },
            _ => Err(unexpected("the node sent an unexpected frame in a file read")),
        }
    }
}

/// The next frame from the node; the connection ending before the read's
/// answer is a read that broke off.
async fn frame(client: &mut Client) -> Result<Vec<u8>, Error> {
    Ok(client.connection.recv().await.map_err(CallError::Broken)?.ok_or_else(|| {
        CallError::Broken(io::Error::new(io::ErrorKind::UnexpectedEof, "the node's connection ended in a file read"))
    })?)
}

/// Something the node sent that no read sends: a fault, never a break.
fn unexpected(message: &str) -> Error {
    Error::Unexpected(message.to_string())
}
