//! Talking to a resident and watching it (`world-colleagues.md`, 用户在任何
//! 地方和居民说话), as any interface does it: what came before in its
//! session, then each event as it happens; words said to it, and the turn
//! they went into followed until it ends.

use std::collections::VecDeque;

use fastctx_types::residents::Landed;
use fastctx_types::session::{Happened, SessionEvent, Speaker, Watched};
use fastctx_types::{MachineName, ResidentId};

use crate::Watching;
use crate::world::{Error, Said, World};

/// A watched session, and the turn the latest words went into.
pub struct Conversation {
    watching: Watching,
    following: Option<Following>,
    queued: VecDeque<SessionEvent>,
}

/// What the latest words are waiting on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Following {
    /// The turn they went into.
    Turn(String),
    /// The session's plugin has them, and says which turn takes them; until
    /// then the words themselves are what is looked for.
    Words(String),
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
        Ok((Self { watching, following: None, queued: VecDeque::new() }, history))
    }

    /// Words were said with `text` and landed as `said` says: what they go
    /// into is followed from now on. Returns whether there is anything to
    /// follow; words waiting for a way in, or gone to another machine as a
    /// letter, are in no turn yet.
    pub fn said(&mut self, said: &Said, text: &str) -> bool {
        self.following = match &said.landed {
            Landed::NewTurn { turn } | Landed::JoinedTurn { turn } => Some(Following::Turn(turn.clone())),
            Landed::Handed => Some(Following::Words(text.to_string())),
            Landed::Waiting { .. } | Landed::Sent => None,
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

    /// The next thing that happens; `None` once the watch ended.
    pub async fn next(&mut self) -> Result<Option<Moment>, Error> {
        loop {
            if let Some(event) = self.queued.pop_front() {
                let answered = self.take(&event);
                return Ok(Some(Moment { event, answered }));
            }
            match self.watching.next().await? {
                Some(Watched::Live { event }) => self.queued.push_back(event),
                Some(Watched::History { events }) => self.queued.extend(events),
                None => return Ok(None),
            }
        }
    }

    /// Follows the latest words through `event`; returns whether it ended
    /// their turn.
    fn take(&mut self, event: &SessionEvent) -> bool {
        match (&self.following, &event.happened) {
            // The session says which turn took them.
            (
                Some(Following::Words(words)),
                Happened::Heard { turn: Some(turn), from: Some(Speaker::User { .. }), text },
            ) if words == text => {
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
