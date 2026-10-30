//! Talking to a resident and watching it (`world-colleagues.md`, 用户在任何
//! 地方和居民说话), as any interface does it: what came before in its
//! session, then each event as it happens; words said to it, and the turn
//! they went into followed until it ends. An interface that keeps what it
//! views (`crate::cache`) opens a conversation from what it keeps at once
//! and is given the history again whenever it changed (`Conversation::open`).
//!
//! A watch whose connection broke is not over (`world-network.md`, 断开不是
//! 结束): it says it is interrupted and why, opens again as `crate::again`
//! paces it with what it already holds, and goes on from what it missed.
//! Whether the conversation itself is gone is for the World's listing to
//! say, not for a broken line.

mod kept;

use fastctx_types::access::Written;
use std::collections::{HashSet, VecDeque};

use fastctx_net::watch_queue;
use fastctx_types::residents::Landed;
use fastctx_types::session::{Happened, SessionChange, SessionEvent, Watched};
use fastctx_types::{Address, ResidentId};

use crate::Watching;
use crate::again::{Again, Interruption};
use crate::world::{Error, Said, World};

/// A watched session, and the turn the latest words went into.
///
/// The watch is read in a task of its own, so [`Conversation::next`] can be
/// raced against typing or a timer: a frame half read from the node is never
/// dropped by a `select!`. Dropping the conversation ends the watch; nothing
/// else does.
pub struct Conversation {
    pieces: watch_queue::Receiver<Result<Option<Watched>, Error>>,
    following: Option<Following>,
    queued: VecDeque<SessionEvent>,
    seen: HashSet<String>,
    finished_voice: HashSet<String>,
    finished: HashSet<(String, String)>,
    ended: HashSet<String>,
    /// What this device keeps of the history, when it keeps it; away while
    /// `working` has it.
    keeping: Option<kept::Keeping>,
    /// What this device keeps, at work in a task of its own on what came:
    /// awaited again after an update that was given up, so nothing is lost.
    working: Option<Working>,
    /// What is watched, to watch it again.
    source: Source,
    /// How it is opened again after a break.
    again: Again,
    recording: Option<Recording>,
    refresh_due: bool,
    current_due: bool,
    /// Whether the resident works now, as last told and not yet handed over.
    now_due: Option<Now>,
}

type Working = tokio::task::JoinHandle<(kept::Keeping, Result<Option<Vec<SessionEvent>>, Error>)>;
type Recording = tokio::task::JoinHandle<(kept::Keeping, Moment, Result<(), Error>)>;

/// The resident watched, and where.
#[derive(Clone)]
struct Source {
    world: World,
    resident: ResidentId,
    machine: Option<Written>,
    /// Opened for an interface, which is told whether the resident works.
    shown: bool,
}

/// What [`Conversation::update`] hands over.
#[derive(Clone, Debug)]
pub enum Update {
    /// The host's initial history is reconciled, even when nothing changed;
    /// after an interruption, the watch is back.
    Current,
    /// The watch broke, or opening it again failed; it is being opened
    /// again.
    Interrupted(Interruption),
    /// Something happened.
    Moment(Moment),
    /// The history is now this, in place of what was handed over before.
    History(Vec<SessionEvent>),
    /// New history after what was handed over; the preceding history agrees.
    Extended(Vec<SessionEvent>),
    /// Whether the resident works now, on which turn, and which words its
    /// machine keeps on their way in: after every event handed over before
    /// it. Only a machine that tells this sends it.
    Now { busy: bool, turn: Option<String>, kept: Vec<fastctx_types::session::KeptWords> },
}

/// What [`Conversation::next`] hands over.
#[derive(Clone, Debug)]
pub enum Next {
    Moment(Moment),
    /// The watch broke, or opening it again failed; it is being opened
    /// again.
    Interrupted(Interruption),
    /// The watch is back after an interruption; what happened meanwhile
    /// follows.
    Resumed,
}

/// Whether the resident works now, as a machine told it.
#[derive(Clone, Debug)]
struct Now {
    busy: bool,
    turn: Option<String>,
    kept: Vec<fastctx_types::session::KeptWords>,
}

/// What the latest words are waiting on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Following {
    /// The turn they went into.
    Turn(String),
    /// The session's plugin has them, and says which turn takes them; until
    /// then its delivery identity is what is looked for.
    Delivery(fastctx_types::NoticeId),
}

/// One thing that happened in the session.
#[derive(Clone, Debug)]
pub struct Moment {
    pub event: SessionEvent,
    /// It ended the turn the latest words went into.
    pub answered: bool,
}

impl Conversation {
    /// Starts watching a resident; returns what came before in its session.
    pub async fn watch(
        world: &World,
        resident: ResidentId,
        machine: Option<Written>,
    ) -> Result<(Self, Vec<SessionEvent>), Error> {
        Self::watching(Source { world: world.clone(), resident, machine, shown: false }).await
    }

    async fn watching(source: Source) -> Result<(Self, Vec<SessionEvent>), Error> {
        let mut watching = source.whole().await?;
        let mut now = None;
        let history = loop {
            break match watching.next().await? {
                Some(Watched::Documents { .. }) => {
                    return Err(Error::Unexpected("document packets are not conversation history".into()));
                }
                Some(Watched::History { events }) => events,
                Some(Watched::Live { event }) => vec![event],
                Some(Watched::Now { busy, turn, kept }) => {
                    now = Some(Now { busy, turn, kept });
                    continue;
                }
                Some(Watched::Unchanged { .. } | Watched::Segments { .. }) => {
                    return Err(Error::Unexpected("segments of a history no one asked for".into()));
                }
                None => Vec::new(),
            };
        };
        let mut conversation = Self::reading(watching, source);
        conversation.now_due = now;
        conversation.reset(&history);
        conversation.current_due = true;
        conversation.again.came();
        Ok((conversation, history))
    }

    /// Opens a conversation for an interface: from what this device keeps
    /// of it at once, its latest part first, when the World keeps what it
    /// views; the rest, and the history as its machine has it now, follow
    /// as [`Update::History`]. A machine that cannot be reached is said by
    /// the first update, once what is kept was shown.
    pub async fn open(
        world: &World,
        resident: ResidentId,
        machine: Written,
    ) -> Result<(Self, Vec<SessionEvent>), Error> {
        Self::opening(world, resident, machine, false).await
    }

    /// Opens again a conversation an interface already shows, as when it
    /// comes back to it: what is kept is given whole at once, so what is
    /// shown never shrinks to its latest part on the way.
    pub async fn reopen(
        world: &World,
        resident: ResidentId,
        machine: Written,
    ) -> Result<(Self, Vec<SessionEvent>), Error> {
        Self::opening(world, resident, machine, true).await
    }

    /// Local history is still due even while a device has no connection.
    pub fn has_cached_rest(&self) -> bool {
        self.keeping.as_ref().is_some_and(|keeping| keeping.rest_due())
    }

    async fn opening(
        world: &World,
        resident: ResidentId,
        machine: Written,
        whole: bool,
    ) -> Result<(Self, Vec<SessionEvent>), Error> {
        let source =
            Source { world: world.clone(), resident: resident.clone(), machine: Some(machine.clone()), shown: true };
        let Some(cache) = world.cache().cloned() else {
            return Self::watching(source).await;
        };
        let address = Address { machine: machine.name, resident };
        let (mut keeping, latest) = match kept::Keeping::open(cache, address.clone(), whole).await {
            Ok(opened) => opened,
            // This device's copy cannot be opened for a passing reason: the
            // conversation is read whole from its machine instead.
            Err(Error::Kept(_)) => return Self::watching(source).await,
            Err(error) => return Err(error),
        };
        let watching = source.held(keeping.held());
        if let Some(latest) = latest {
            let mut conversation = Self::reading_after(watching, source);
            conversation.reset(&latest);
            conversation.keeping = Some(keeping);
            return Ok((conversation, latest));
        }
        let mut conversation = Self::reading_after(watching, source);
        let history = loop {
            match conversation.piece().await {
                Some(Ok(Some(Watched::Live { event }))) => conversation.queued.push_back(event),
                Some(Ok(Some(Watched::Now { busy, turn, kept }))) => {
                    conversation.now_due = Some(Now { busy, turn, kept })
                }
                Some(Ok(Some(watched))) => {
                    conversation.again.came();
                    let watched = conversation.pending_from(watched);
                    match keeping.take(watched).await {
                        Ok(Some(history)) => break history,
                        Ok(None) => {}
                        Err(Error::Kept(_)) => return Self::watching(conversation.source.clone()).await,
                        Err(error) => return Err(error),
                    }
                }
                Some(Ok(None)) | None => break Vec::new(),
                Some(Err(error)) => return Err(error),
            }
        };
        conversation.reset(&history);
        conversation.keeping = Some(keeping);
        conversation.current_due = conversation.keeping.as_mut().expect("kept history").current();
        Ok((conversation, history))
    }

    /// The next thing to hand over of a conversation [`Conversation::open`]
    /// opened. Safe to cancel, as `next` is.
    pub async fn update(&mut self) -> Result<Update, Error> {
        if std::mem::take(&mut self.current_due) {
            return Ok(Update::Current);
        }
        if std::mem::take(&mut self.refresh_due)
            && let Some(keeping) = self.keeping.as_ref()
        {
            self.pieces = Self::read(self.source.held(keeping.held()));
        }
        if self.working.is_none()
            && self.keeping.as_ref().is_some_and(|keeping| keeping.rest_due())
            && let Some(mut keeping) = self.keeping.take()
        {
            self.working = Some(tokio::spawn(async move {
                let rest = keeping.rest().await;
                (keeping, rest)
            }));
        }
        loop {
            if let Some(recording) = &mut self.recording {
                let (keeping, moment, saved) =
                    recording.await.map_err(|error| Error::Kept(std::io::Error::other(error)))?;
                self.recording = None;
                self.keeping = Some(keeping);
                // What happened is shown whether or not this device could
                // keep it; a copy that lacks it is read whole next time.
                if saved.is_err()
                    && let Some(keeping) = self.keeping.as_mut()
                {
                    keeping.missed();
                }
                self.refresh_due = matches!(
                    moment.event.happened,
                    Happened::Changed { change: SessionChange::HistoryChanged | SessionChange::HistoryCut { .. }, .. }
                );
                return Ok(Update::Moment(moment));
            }
            if let Some(working) = &mut self.working {
                let done = working.await;
                self.working = None;
                let (mut keeping, done) =
                    done.map_err(|error| Error::Kept(std::io::Error::other(format!("keeping the history: {error}"))))?;
                match done {
                    Ok(Some(history)) => {
                        self.current_due = keeping.current();
                        let extended = keeping.extended();
                        self.keeping = Some(keeping);
                        if extended {
                            self.remember(&history);
                            return Ok(Update::Extended(history));
                        }
                        self.reset(&history);
                        return Ok(Update::History(history));
                    }
                    Ok(None) => {
                        let current = keeping.current();
                        self.keeping = Some(keeping);
                        if current {
                            return Ok(Update::Current);
                        }
                    }
                    Err(error) if keeping.lost(&error) => {
                        keeping.discard_damage(&error)?;
                        keeping.renew_generation()?;
                        let held = keeping.held();
                        self.keeping = Some(keeping);
                        self.pieces = Self::read(self.source.held(held));
                    }
                    // This device's copy could not be read or written for a
                    // passing reason, as when no descriptor or no disk was
                    // left: the conversation is read whole from its machine,
                    // and kept no more while this reading lasts, rather than
                    // its watch ending with the page left as it stood.
                    Err(Error::Kept(_)) => {
                        self.pieces = Self::read(self.source.whole());
                    }
                    Err(error) => {
                        self.keeping = Some(keeping);
                        return Err(error);
                    }
                }
            }
            if let Some(moment) = self.queued_moment() {
                if let Some(mut keeping) = self.keeping.take() {
                    self.recording = Some(tokio::spawn(async move {
                        let saved = async {
                            if matches!(
                                moment.event.happened,
                                Happened::Changed {
                                    change: SessionChange::HistoryChanged | SessionChange::HistoryCut { .. },
                                    ..
                                }
                            ) {
                                keeping.rewritten(&moment.event)?;
                            }
                            keeping.record(moment.event.clone()).await
                        }
                        .await;
                        (keeping, moment, saved)
                    }));
                    continue;
                }
                return Ok(Update::Moment(moment));
            }
            if self.queued.is_empty()
                && let Some(Now { busy, turn, kept }) = self.now_due.take()
            {
                return Ok(Update::Now { busy, turn, kept });
            }
            let watched = match self.piece().await {
                Some(Ok(Some(Watched::Live { event }))) => {
                    self.current_due |= self.again.came();
                    self.queued.push_back(event);
                    continue;
                }
                // Told after what came before it: handed over once that is.
                Some(Ok(Some(Watched::Now { busy, turn, kept }))) => {
                    self.now_due = Some(Now { busy, turn, kept });
                    continue;
                }
                Some(Ok(Some(watched))) => {
                    self.current_due |= self.again.came();
                    watched
                }
                Some(Ok(None)) | None => return Ok(Update::Interrupted(self.broke("the watch ended".into()))),
                // Deleted, or this account refused: watching again changes nothing.
                Some(Err(error)) if error.is_deleted() || error.is_refused() => return Err(error),
                Some(Err(error)) => return Ok(Update::Interrupted(self.broke(error.to_string()))),
            };
            let watched = self.pending_from(watched);
            let Some(mut keeping) = self.keeping.take() else {
                let Watched::History { events } = watched else {
                    return Err(Error::Unexpected("segments of a history no one asked for".into()));
                };
                self.reset(&events);
                return Ok(Update::History(events));
            };
            self.working = Some(tokio::spawn(async move {
                let taken = keeping.take(watched).await;
                (keeping, taken)
            }));
        }
    }

    /// A conversation read from `watching`, in a task of its own.
    fn reading(watching: Watching, source: Source) -> Self {
        Self::reading_after(async { Ok(watching) }, source)
    }

    /// The watch broke for `why`: it is opened again once `again` says,
    /// with what is held of the history when this device keeps it.
    fn broke(&mut self, why: String) -> Interruption {
        let interruption = self.again.broke(why);
        let pause = self.again.pause(&self.source.world, self.source.machine.as_ref());
        let opening = match &self.keeping {
            Some(keeping) => self.source.held(keeping.held()),
            None => self.source.whole(),
        };
        self.pieces = Self::read(async move {
            pause.await;
            opening.await
        });
        interruption
    }

    fn pending_from(&mut self, mut watched: Watched) -> Watched {
        if let Watched::History { events } = &mut watched {
            events.retain(|event| {
                if fastctx_net::history::transient(event) {
                    self.queued.push_back(event.clone());
                    false
                } else {
                    true
                }
            });
        }
        watched
    }

    fn reading_after(
        open: impl std::future::Future<Output = Result<Watching, Error>> + Send + 'static,
        source: Source,
    ) -> Self {
        Self::from(Self::read(open), source)
    }

    /// Opens a watch with `open` and reads it in a task of its own, until
    /// it ends or nobody reads on.
    fn read(
        open: impl std::future::Future<Output = Result<Watching, Error>> + Send + 'static,
    ) -> watch_queue::Receiver<Result<Option<Watched>, Error>> {
        let (sender, pieces) = watch_queue::channel(|piece: &Result<Option<Watched>, Error>| match piece {
            Ok(Some(piece)) => watch_queue::watched_size(piece),
            Ok(None) => (0, None),
            Err(error) => (error.to_string().len(), None),
        });
        tokio::spawn(async move {
            let mut watching = tokio::select! {
                () = sender.closed() => return,
                watching = open => match watching {
                    Ok(watching) => watching,
                    Err(error) => { let _ = sender.send(Err(error)); return; }
                }
            };
            loop {
                tokio::select! {
                    // Nobody reads on: the watch ends with this task, and a
                    // frame half read goes with it.
                    () = sender.closed() => return,
                    piece = watching.next() => {
                        let ended = !matches!(piece, Ok(Some(_)));
                        let history = matches!(piece, Ok(Some(Watched::History { .. } | Watched::Segments { .. } | Watched::Unchanged { .. })));
                        let piece = piece.map_err(Error::from);
                        let sent = if history {
                            sender.send_wait(piece, std::time::Duration::from_secs(30)).await
                        } else { sender.send(piece) };
                        if sent.is_err() || ended { return; }
                    }
                }
            }
        });
        pieces
    }

    fn from(pieces: watch_queue::Receiver<Result<Option<Watched>, Error>>, source: Source) -> Self {
        Self {
            pieces,
            following: None,
            queued: VecDeque::new(),
            seen: HashSet::new(),
            finished: HashSet::new(),
            ended: HashSet::new(),
            finished_voice: HashSet::new(),
            keeping: None,
            working: None,
            source,
            again: Again::default(),
            recording: None,
            refresh_due: false,
            current_due: false,
            now_due: None,
        }
    }

    async fn piece(&mut self) -> Option<Result<Option<Watched>, Error>> {
        self.pieces.recv().await.unwrap_or_else(|error| Some(Err(Error::Unexpected(error.to_string()))))
    }

    /// What came before is now `history`: what is live is told apart from it.
    fn reset(&mut self, history: &[SessionEvent]) {
        self.seen.clear();
        self.finished_voice.clear();
        self.finished.clear();
        self.ended.clear();
        self.remember(history);
    }

    fn remember(&mut self, history: &[SessionEvent]) {
        self.seen.extend(history.iter().filter_map(identity));
        self.finished_voice.extend(history.iter().filter_map(|event| match &event.happened {
            Happened::Realtime { item, complete: true } => item["id"].as_str().map(str::to_string),
            _ => None,
        }));
        self.finished.extend(history.iter().filter_map(|event| match &event.happened {
            Happened::Finished { turn, item } => Some((turn.clone(), item.id.clone())),
            _ => None,
        }));
        self.ended.extend(history.iter().filter_map(|event| match &event.happened {
            Happened::TurnEnded { turn, .. } => Some(turn.clone()),
            _ => None,
        }));
    }

    /// Words were said with `text` and landed as `said` says: what they go
    /// into is followed from now on. Returns whether there is anything to
    /// follow; words waiting for a way in, or gone to another machine as a
    /// letter, are in no turn yet.
    pub fn said(&mut self, said: &Said, _text: &str) -> bool {
        self.following = match &said.landed {
            Landed::NewTurn { turn } | Landed::JoinedTurn { turn } => Some(Following::Turn(turn.clone())),
            Landed::Handed { id } => Some(Following::Delivery(id.clone())),
            Landed::Waiting { .. } | Landed::Sent | Landed::Uncertain { .. } => None,
        };
        self.following.is_some()
    }

    /// What the latest words wait on, while they do.
    pub fn following(&self) -> Option<&Following> {
        self.following.as_ref()
    }

    /// Whether the followed turn already ended among `events`, as among the
    /// history the watch began with.
    pub fn ended_among(&mut self, events: &[SessionEvent]) -> bool {
        events.iter().any(|event| self.take(event))
    }

    /// The next thing that happens, or that the watch broke or came back;
    /// what is seen again after it came back is not handed over twice.
    /// Safe to cancel: nothing is lost when another branch of a `select!`
    /// wins.
    pub async fn next(&mut self) -> Result<Next, Error> {
        loop {
            if std::mem::take(&mut self.current_due) && self.keeping.is_none() {
                return Ok(Next::Resumed);
            }
            if let Some(moment) = self.queued_moment() {
                return Ok(Next::Moment(moment));
            }
            match self.piece().await {
                Some(Ok(Some(Watched::Live { event }))) => {
                    self.current_due |= self.again.came();
                    self.queued.push_back(event);
                }
                Some(Ok(Some(Watched::History { events }))) => {
                    self.current_due |= self.again.came();
                    self.queued.extend(events);
                }
                Some(Ok(Some(Watched::Unchanged { .. } | Watched::Segments { .. }))) => {
                    return Err(Error::Unexpected("segments of a history no one asked for".into()));
                }
                Some(Ok(Some(Watched::Now { .. }))) => {}
                Some(Ok(Some(Watched::Documents { .. }))) => {
                    return Err(Error::Unexpected("document packets are not conversation history".into()));
                }
                Some(Ok(None)) | None => return Ok(Next::Interrupted(self.broke("the watch ended".into()))),
                Some(Err(error)) if error.is_deleted() || error.is_refused() => return Err(error),
                Some(Err(error)) => return Ok(Next::Interrupted(self.broke(error.to_string()))),
            }
        }
    }

    /// The next queued event that is news, as a moment.
    fn queued_moment(&mut self) -> Option<Moment> {
        loop {
            if let Some(event) = self.queued.pop_front() {
                // Registration precedes the history read. That read can
                // include a completed item whose older deltas are queued.
                let stale = match &event.happened {
                    Happened::Began { turn, item } | Happened::Updated { turn, item } => {
                        self.ended.contains(turn) || self.finished.contains(&(turn.clone(), item.id.clone()))
                    }
                    Happened::Grew { turn, item, .. } => {
                        self.ended.contains(turn) || self.finished.contains(&(turn.clone(), item.clone()))
                    }
                    Happened::Finished { turn, item } => {
                        self.finished.insert((turn.clone(), item.id.clone()));
                        false
                    }
                    Happened::TurnEnded { turn, .. } => {
                        self.ended.insert(turn.clone());
                        false
                    }
                    Happened::Realtime { item, complete: false } => {
                        item["id"].as_str().is_some_and(|id| self.finished_voice.contains(id))
                    }
                    Happened::RealtimeGrew { item, .. } => self.finished_voice.contains(item),
                    Happened::Realtime { item, complete: true } => {
                        if let Some(id) = item["id"].as_str() {
                            self.finished_voice.insert(id.into());
                        }
                        false
                    }
                    _ => false,
                };
                if stale {
                    continue;
                }
                if let Some(key) = identity(&event)
                    && !self.seen.insert(key)
                {
                    continue;
                }
                let answered = self.take(&event);
                return Some(Moment { event, answered });
            }
            return None;
        }
    }

    /// Follows the latest words through `event`; returns whether it ended
    /// their turn.
    fn take(&mut self, event: &SessionEvent) -> bool {
        match (&self.following, &event.happened) {
            (Some(Following::Delivery(id)), Happened::DeliveryUncertain { delivery, .. }) if id == delivery => {
                self.following = None;
                true
            }
            // The session says which turn took them.
            (Some(Following::Delivery(id)), Happened::Heard { turn: Some(turn), delivery: Some(delivery), .. })
                if id == delivery =>
            {
                self.following = Some(Following::Turn(turn.clone()));
                false
            }
            (Some(Following::Turn(turn)), Happened::TurnEnded { turn: ended, .. }) if ended == turn => {
                self.following = None;
                true
            }
            _ => false,
        }
    }
}

impl Source {
    /// Watches it again, telling its machine what this device holds.
    fn held(&self, held: fastctx_types::session::held::Held) -> Opening {
        self.opening(Some(held))
    }

    /// Watches it again from its whole history.
    fn whole(&self) -> Opening {
        self.opening(None)
    }

    fn opening(&self, held: Option<fastctx_types::session::held::Held>) -> Opening {
        let (world, resident, machine, shown) =
            (self.world.clone(), self.resident.clone(), self.machine.clone(), self.shown);
        Box::pin(async move {
            match (shown, held) {
                (true, held) => world.watch_shown(resident, machine, held).await,
                (false, Some(held)) => world.watch_held(resident, machine, held).await,
                (false, None) => world.watch(resident, machine).await,
            }
        })
    }
}

type Opening = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Watching, Error>> + Send>>;

fn identity(event: &SessionEvent) -> Option<String> {
    match &event.happened {
        Happened::TurnEnded { turn, .. } => Some(format!("end:{turn}")),
        Happened::TurnStarted { turn } => Some(format!("start:{turn}")),
        Happened::RealtimeGrew { .. } => None,
        Happened::Began { turn, item } => Some(format!("began:{turn}:{}", item.id)),
        Happened::Finished { turn, item } => Some(format!("finished:{turn}:{}", item.id)),
        Happened::DeliveryUncertain { delivery, .. } => Some(format!("uncertain:{delivery}")),
        Happened::Heard { turn, message: Some(message), .. } => Some(format!("heard:{turn:?}:{}", message.id)),
        Happened::Realtime { item, complete } => {
            Some(format!("realtime:{complete}:{}:{}", item["realtimeSessionId"], item["id"]))
        }
        _ => serde_json::to_string(event).ok(),
    }
}
