//! Talking to a resident and watching it (`world-colleagues.md`, 用户在任何
//! 地方和居民说话), as any interface does it: what came before in its
//! session, then each event as it happens; words said to it, and the turn
//! they went into followed until it ends. An interface that keeps what it
//! views (`crate::cache`) opens a conversation from what it keeps at once
//! and is given the history again whenever it changed (`Conversation::open`).

mod kept;

use std::collections::{HashSet, VecDeque};

use fastctx_types::residents::Landed;
use fastctx_types::session::{Happened, SessionChange, SessionEvent, Watched};
use fastctx_types::{Address, MachineName, ResidentId};
use tokio::sync::mpsc;

use crate::world::{Error, Said, World};
use crate::{CallError, Watching};

/// A watched session, and the turn the latest words went into.
///
/// The watch is read in a task of its own, so [`Conversation::next`] can be
/// raced against typing or a timer: a frame half read from the node is never
/// dropped by a `select!`. Dropping the conversation ends the watch.
pub struct Conversation {
    pieces: mpsc::UnboundedReceiver<Result<Option<Watched>, CallError>>,
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
    /// How to watch again, when what was kept went away while it was read.
    again: Option<(World, Address)>,
    /// Why the watch could not start, said once what was kept was shown.
    failed: Option<Error>,
}

type Working = tokio::task::JoinHandle<(kept::Keeping, Result<Option<Vec<SessionEvent>>, Error>)>;

/// What [`Conversation::update`] hands over.
#[derive(Clone, Debug)]
pub enum Update {
    /// Something happened.
    Moment(Moment),
    /// The history is now this, in place of what was handed over before.
    History(Vec<SessionEvent>),
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
        machine: Option<MachineName>,
    ) -> Result<(Self, Vec<SessionEvent>), Error> {
        let mut watching = world.watch(resident, machine).await?;
        let history = match watching.next().await? {
            Some(Watched::History { events }) => events,
            Some(Watched::Live { event }) => vec![event],
            Some(Watched::Unchanged { .. } | Watched::Segments { .. }) => {
                return Err(Error::Unexpected("segments of a history no one asked for".into()));
            }
            None => Vec::new(),
        };
        let mut conversation = Self::reading(watching);
        conversation.reset(&history);
        Ok((conversation, history))
    }

    /// Opens a conversation for an interface: from what this device keeps
    /// of it at once, its latest part first, when the World keeps what it
    /// views; the rest, and the history as its machine has it now, follow
    /// as [`Update::History`]. A machine that cannot be reached is said by
    /// the first update, once what is kept was shown.
    pub async fn open(world: &World, address: Address) -> Result<(Self, Vec<SessionEvent>), Error> {
        let Some(cache) = world.cache().cloned() else {
            return Self::watch(world, address.resident, Some(address.machine)).await;
        };
        let (mut keeping, latest) = kept::Keeping::open(cache, address.clone()).await?;
        let watching = world.watch_held(address.resident.clone(), Some(address.machine.clone()), keeping.held()).await;
        if let Some(latest) = latest {
            let mut conversation = match watching {
                Ok(watching) => Self::reading(watching),
                Err(error) => {
                    let mut conversation = Self::reading_nothing();
                    conversation.failed = Some(error);
                    conversation
                }
            };
            conversation.reset(&latest);
            conversation.keeping = Some(keeping);
            conversation.again = Some((world.clone(), address));
            return Ok((conversation, latest));
        }
        let mut conversation = Self::reading(watching?);
        let history = loop {
            match conversation.pieces.recv().await {
                Some(Ok(Some(Watched::Live { event }))) => conversation.queued.push_back(event),
                Some(Ok(Some(watched))) => {
                    if let Some(history) = keeping.take(watched).await? {
                        break history;
                    }
                }
                Some(Ok(None)) | None => break Vec::new(),
                Some(Err(error)) => return Err(error.into()),
            }
        };
        conversation.reset(&history);
        conversation.keeping = Some(keeping);
        conversation.again = Some((world.clone(), address));
        Ok((conversation, history))
    }

    /// The next thing to hand over of a conversation [`Conversation::open`]
    /// opened; `None` once the watch ended. Safe to cancel, as `next` is.
    pub async fn update(&mut self) -> Result<Option<Update>, Error> {
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
            if let Some(working) = &mut self.working {
                let done = working.await;
                self.working = None;
                let (mut keeping, done) =
                    done.map_err(|error| Error::Kept(std::io::Error::other(format!("keeping the history: {error}"))))?;
                match done {
                    Ok(Some(history)) => {
                        self.keeping = Some(keeping);
                        self.reset(&history);
                        return Ok(Some(Update::History(history)));
                    }
                    Ok(None) => self.keeping = Some(keeping),
                    Err(error) if keeping.lost(&error) => {
                        let (world, address) = self.again.clone().expect("a kept conversation can be watched again");
                        let held = keeping.held();
                        self.keeping = Some(keeping);
                        let watching = world.watch_held(address.resident, Some(address.machine), held).await?;
                        self.pieces = Self::reading(watching).pieces;
                    }
                    Err(error) => {
                        self.keeping = Some(keeping);
                        return Err(error);
                    }
                }
            }
            // What is kept is all given before a watch that could not start
            // is said to have failed.
            if let Some(error) = self.failed.take() {
                return Err(error);
            }
            if let Some(moment) = self.queued_moment() {
                if let Happened::Changed {
                    change: SessionChange::HistoryChanged | SessionChange::HistoryCut { .. },
                    ..
                } = &moment.event.happened
                    && let Some(keeping) = &self.keeping
                {
                    keeping.rewritten()?;
                }
                return Ok(Some(Update::Moment(moment)));
            }
            let watched = match self.pieces.recv().await {
                Some(Ok(Some(Watched::Live { event }))) => {
                    self.queued.push_back(event);
                    continue;
                }
                Some(Ok(Some(watched))) => watched,
                Some(Ok(None)) | None => return Ok(None),
                Some(Err(error)) => return Err(error.into()),
            };
            let Some(mut keeping) = self.keeping.take() else {
                let Watched::History { events } = watched else {
                    return Err(Error::Unexpected("segments of a history no one asked for".into()));
                };
                self.reset(&events);
                return Ok(Some(Update::History(events)));
            };
            self.working = Some(tokio::spawn(async move {
                let taken = keeping.take(watched).await;
                (keeping, taken)
            }));
        }
    }

    /// A conversation read from `watching`, in a task of its own.
    fn reading(mut watching: Watching) -> Self {
        let (sender, pieces) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Nobody reads on: the watch ends with this task, and a
                    // frame half read goes with it.
                    () = sender.closed() => return,
                    piece = watching.next() => {
                        let ended = !matches!(piece, Ok(Some(_)));
                        if sender.send(piece).is_err() || ended {
                            return;
                        }
                    }
                }
            }
        });
        Self::from(pieces)
    }

    /// A conversation whose watch did not start.
    fn reading_nothing() -> Self {
        Self::from(mpsc::unbounded_channel().1)
    }

    fn from(pieces: mpsc::UnboundedReceiver<Result<Option<Watched>, CallError>>) -> Self {
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
            again: None,
            failed: None,
        }
    }

    /// What came before is now `history`: what is live is told apart from it.
    fn reset(&mut self, history: &[SessionEvent]) {
        self.seen = history.iter().filter_map(identity).collect();
        self.finished_voice = history
            .iter()
            .filter_map(|event| match &event.happened {
                Happened::Realtime { item, complete: true } => item["id"].as_str().map(str::to_string),
                _ => None,
            })
            .collect();
        self.finished = history
            .iter()
            .filter_map(|event| match &event.happened {
                Happened::Finished { turn, item } => Some((turn.clone(), item.id.clone())),
                _ => None,
            })
            .collect();
        self.ended = history
            .iter()
            .filter_map(|event| match &event.happened {
                Happened::TurnEnded { turn, .. } => Some(turn.clone()),
                _ => None,
            })
            .collect();
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

    /// The next thing that happens; `None` once the watch ended. Safe to
    /// cancel: nothing is lost when another branch of a `select!` wins.
    pub async fn next(&mut self) -> Result<Option<Moment>, Error> {
        loop {
            if let Some(moment) = self.queued_moment() {
                return Ok(Some(moment));
            }
            match self.pieces.recv().await {
                Some(Ok(Some(Watched::Live { event }))) => self.queued.push_back(event),
                Some(Ok(Some(Watched::History { events }))) => self.queued.extend(events),
                Some(Ok(Some(Watched::Unchanged { .. } | Watched::Segments { .. }))) => {
                    return Err(Error::Unexpected("segments of a history no one asked for".into()));
                }
                Some(Ok(None)) | None => return Ok(None),
                Some(Err(error)) => return Err(error.into()),
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
