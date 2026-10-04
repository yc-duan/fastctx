//! Talking to a resident and watching it (`world-colleagues.md`, 用户在任何
//! 地方和居民说话), as any interface does it: what came before in its
//! session, then each event as it happens; words said to it, and the turn
//! they went into followed until it ends.

use std::collections::{HashSet, VecDeque};

use fastctx_types::residents::Landed;
use fastctx_types::session::{Happened, SessionEvent, Watched};
use fastctx_types::{MachineName, ResidentId};
use tokio::sync::mpsc;

use crate::CallError;
use crate::world::{Error, Said, World};

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
            None => Vec::new(),
        };
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
        {
            let seen = history.iter().filter_map(identity).collect();
            Ok((Self { pieces, following: None, queued: VecDeque::new(), seen }, history))
        }
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
            if let Some(event) = self.queued.pop_front() {
                if let Some(key) = identity(&event)
                    && !self.seen.insert(key)
                {
                    continue;
                }
                let answered = self.take(&event);
                return Ok(Some(Moment { event, answered }));
            }
            match self.pieces.recv().await {
                Some(Ok(Some(Watched::Live { event }))) => self.queued.push_back(event),
                Some(Ok(Some(Watched::History { events }))) => self.queued.extend(events),
                Some(Ok(None)) | None => return Ok(None),
                Some(Err(error)) => return Err(error.into()),
            }
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
        Happened::Writing { .. } => None,
        Happened::DeliveryUncertain { delivery, .. } => Some(format!("uncertain:{delivery}")),
        _ => serde_json::to_string(event).ok(),
    }
}
