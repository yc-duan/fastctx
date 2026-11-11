//! Backpressure belongs to this subscriber's forwarding task, never to the
//! conversation publisher. What the subscriber's queue has no room for waits
//! here, and a watcher that falls behind is sent less rather than cut: more
//! of an item that already waits joins it, a newer complete state replaces
//! the older one. Only when even that grows past a bound does the watch lag,
//! and its watcher resumes it.

use std::collections::{BTreeMap, HashMap};

use fastctx_net::watch_queue::{self, Lagged, Sender, TrySendError};
use fastctx_types::{
    ipc::Failure,
    session::{Happened, Watched},
};

type Pieces = Sender<Result<Watched, Failure>>;

/// What may wait for a slow watcher, merged as far as it goes. One piece
/// alone may be larger (a whole history waits behind nothing).
const WAITING_BYTES: usize = 16 * 1024 * 1024;
const WAITING_PIECES: usize = 16 * 1024;

struct Waiting {
    piece: Watched,
    size: usize,
    key: Option<String>,
}

/// One watcher's pieces on their way: straight into its queue while it keeps
/// up, kept and merged here while it does not.
pub(super) struct Forwarder {
    sender: Pieces,
    waiting: BTreeMap<u64, Waiting>,
    next: u64,
    bytes: usize,
    /// Where each complete state that waits stands.
    keys: HashMap<String, u64>,
    /// The last piece about each item (turn, item) that waits.
    items: HashMap<(String, String), u64>,
    /// The last piece that waits that every later piece must stay behind:
    /// a turn's start or end, words heard, the history changed.
    barrier: u64,
}

impl Forwarder {
    pub fn new(sender: Pieces) -> Self {
        Self {
            sender,
            waiting: BTreeMap::new(),
            next: 0,
            bytes: 0,
            keys: HashMap::new(),
            items: HashMap::new(),
            barrier: 0,
        }
    }

    pub fn sender(&self) -> &Pieces {
        &self.sender
    }

    /// Whether pieces wait for room.
    pub fn waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// Hands `piece` on, or keeps it until there is room.
    pub fn push(&mut self, piece: Watched) -> Result<(), Lagged> {
        let (size, key) = watch_queue::watched_size(&piece);
        if self.waiting.is_empty() {
            match self.sender.try_send_measured(Ok(piece), size, key.clone()) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Closed(_)) => return Err(Lagged),
                Err(TrySendError::Full(Ok(piece))) => return self.keep(piece, size, key),
                Err(TrySendError::Full(Err(_))) => unreachable!("only pieces are pushed"),
            }
        }
        self.keep(piece, size, key)
    }

    /// Resolves once the first piece that waits has room; never while none
    /// waits.
    pub async fn room(&self) -> Result<(), Lagged> {
        match self.waiting.first_key_value() {
            Some((_, first)) => self.sender.space(first.size, first.key.as_deref()).await,
            None => std::future::pending().await,
        }
    }

    /// Hands on what waits, as far as there is room.
    pub fn flush(&mut self) -> Result<(), Lagged> {
        while let Some((number, first)) = self.waiting.pop_first() {
            match self.sender.try_send_measured(Ok(first.piece), first.size, first.key.clone()) {
                Ok(()) => {
                    self.bytes -= first.size;
                    if let Some(key) = &first.key
                        && self.keys.get(key) == Some(&number)
                    {
                        self.keys.remove(key);
                    }
                }
                Err(TrySendError::Closed(_)) => return Err(Lagged),
                Err(TrySendError::Full(piece)) => {
                    let piece = piece.expect("only pieces are pushed");
                    self.waiting.insert(number, Waiting { piece, size: first.size, key: first.key });
                    return Ok(());
                }
            }
        }
        // Nothing waits: what told where pieces stood is moot.
        self.items.clear();
        self.keys.clear();
        Ok(())
    }

    fn keep(&mut self, piece: Watched, size: usize, key: Option<String>) -> Result<(), Lagged> {
        let (piece, size) = match self.merged(piece, size) {
            Ok(merged) => merged,
            Err((piece, size)) => (piece, size),
        };
        if let Some(key) = &key
            && let Some(previous) = self.keys.remove(key)
            && let Some(old) = self.waiting.remove(&previous)
        {
            self.bytes -= old.size;
        }
        self.next += 1;
        let number = self.next;
        if let Some(key) = &key {
            self.keys.insert(key.clone(), number);
        }
        match about(&piece) {
            About::Item(turn, item) => {
                self.items.insert((turn.to_owned(), item.to_owned()), number);
            }
            About::Everything => self.barrier = number,
            About::Itself => {}
        }
        self.bytes += size;
        self.waiting.insert(number, Waiting { piece, size, key });
        if self.waiting.len() > 1 && (self.bytes > WAITING_BYTES || self.waiting.len() > WAITING_PIECES) {
            self.sender.lag();
            return Err(Lagged);
        }
        Ok(())
    }

    /// More of an item whose last piece waiting is also more of it, with
    /// nothing after that every piece must stay behind, joins that piece: it
    /// leaves its place and goes at the end, carrying the newer mark, so the
    /// marks a watcher sees still only rise.
    fn merged(&mut self, piece: Watched, size: usize) -> Result<(Watched, usize), (Watched, usize)> {
        let Watched::Live { event, mark, likeness } = piece else { return Err((piece, size)) };
        let Happened::Grew { turn, item, part, text, revision } = event.happened else {
            return Err((Watched::Live { event, mark, likeness }, size));
        };
        let earlier = self
            .items
            .get(&(turn.clone(), item.clone()))
            .copied()
            .filter(|number| *number > self.barrier)
            .filter(|number| {
                self.waiting.get(number).is_some_and(|waiting| {
                    matches!(&waiting.piece, Watched::Live { event: before, .. }
                        if matches!(&before.happened, Happened::Grew { turn: t, item: i, part: p, .. }
                            if *t == turn && *i == item && *p == part))
                })
            });
        let at = event.at;
        let Some(number) = earlier else {
            let event = fastctx_types::session::SessionEvent {
                at,
                happened: Happened::Grew { turn, item, part, text, revision },
            };
            return Err((Watched::Live { event, mark, likeness }, size));
        };
        let old = self.waiting.remove(&number).expect("checked above");
        self.bytes -= old.size;
        let Watched::Live { event: before, .. } = old.piece else { unreachable!("checked above") };
        let Happened::Grew { text: mut joined, .. } = before.happened else { unreachable!("checked above") };
        joined.push_str(&text);
        let event = fastctx_types::session::SessionEvent {
            at,
            happened: Happened::Grew { turn, item, part, text: joined, revision },
        };
        let likeness = likeness.and_then(|_| {
            serde_json::to_vec(&event).ok().map(|encoded| Box::new(crate::timeline::likeness(&event, &encoded)))
        });
        Ok((Watched::Live { event, mark, likeness }, old.size + size))
    }
}

enum About<'a> {
    /// One item of a turn.
    Item(&'a str, &'a str),
    /// The order of everything around it.
    Everything,
    /// Nothing another piece depends on.
    Itself,
}

fn about(piece: &Watched) -> About<'_> {
    let Watched::Live { event, .. } = piece else {
        return match piece {
            Watched::Now { .. } | Watched::Collaboration { .. } => About::Itself,
            _ => About::Everything,
        };
    };
    match &event.happened {
        Happened::Began { turn, item } | Happened::Finished { turn, item } | Happened::Updated { turn, item } => {
            About::Item(turn, &item.id)
        }
        Happened::Grew { turn, item, .. } => About::Item(turn, item),
        Happened::TurnStarted { .. } | Happened::TurnEnded { .. } | Happened::Heard { .. } => About::Everything,
        Happened::Changed { change, .. } => match change {
            fastctx_types::session::SessionChange::HistoryChanged
            | fastctx_types::session::SessionChange::HistoryCut { .. } => About::Everything,
            _ => About::Itself,
        },
        _ => About::Itself,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::session::{Mark, Part, SessionEvent};

    fn live(sequence: u64, happened: Happened) -> Watched {
        Watched::Live {
            event: SessionEvent { at: jiff::Timestamp::UNIX_EPOCH, happened },
            mark: Some(Mark { epoch: 1, sequence }),
            likeness: None,
        }
    }
    fn grew(sequence: u64, item: &str, text: &str) -> Watched {
        live(
            sequence,
            Happened::Grew { revision: None, turn: "t".into(), item: item.into(), part: Part::Text, text: text.into() },
        )
    }

    #[test]
    fn a_slow_watcher_gets_more_of_an_item_joined_with_marks_still_rising() {
        let (sender, mut receiver) = watch_queue::channel(|piece: &Result<Watched, Failure>| match piece {
            Ok(piece) => watch_queue::watched_size(piece),
            Err(error) => (error.message.len(), None),
        });
        let mut forwarder = Forwarder::new(sender);
        let mut sequence = 0;
        // Fill the watcher's queue.
        while !forwarder.waiting() {
            sequence += 1;
            forwarder.push(grew(sequence, &format!("filler {sequence}"), "x")).unwrap();
        }
        let filled = sequence - 1;
        // That last one waits; then two items grow, interleaved, with a turn
        // boundary after which nothing joins what came before it.
        forwarder.push(grew(sequence + 1, "a", "1")).unwrap();
        forwarder.push(grew(sequence + 2, "b", "1")).unwrap();
        forwarder.push(grew(sequence + 3, "a", "2")).unwrap();
        forwarder.push(grew(sequence + 4, "b", "2")).unwrap();
        forwarder.push(live(sequence + 5, Happened::TurnStarted { turn: "u".into() })).unwrap();
        forwarder.push(grew(sequence + 6, "a", "3")).unwrap();
        assert_eq!(forwarder.waiting.len(), 5);
        let mut seen = Vec::new();
        loop {
            while let Ok(piece) = receiver.try_recv() {
                seen.push(piece.unwrap());
            }
            if !forwarder.waiting() {
                break;
            }
            forwarder.flush().unwrap();
        }
        assert_eq!(seen.len() as u64, filled + 5);
        let marks: Vec<u64> = seen
            .iter()
            .map(|piece| match piece {
                Watched::Live { mark: Some(mark), .. } => mark.sequence,
                _ => panic!("a live piece"),
            })
            .collect();
        assert!(marks.windows(2).all(|pair| pair[0] < pair[1]), "marks rise: {marks:?}");
        let tail: Vec<String> = seen[seen.len() - 4..]
            .iter()
            .map(|piece| match piece {
                Watched::Live { event, .. } => match &event.happened {
                    Happened::Grew { item, text, .. } => format!("{item}:{text}"),
                    Happened::TurnStarted { turn } => format!("turn {turn}"),
                    _ => panic!("unexpected"),
                },
                _ => panic!("unexpected"),
            })
            .collect();
        assert_eq!(tail, ["a:12", "b:12", "turn u", "a:3"]);
    }
}
