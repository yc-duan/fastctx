//! The window of letters between a machine and its hub, both ways
//! (`specs/world-network.md`, 信): a sending end keeps at most `WINDOW`
//! letters, and about `WINDOW_BYTES` of them, on their way until the other
//! end says it holds them, every letter through a number together; a
//! receiving end takes each number of a numbering once, and says through
//! which it holds them. When each end keeps what on disk is its own; this
//! is only what both decide alike, so that one piece of code decides it.

use std::collections::VecDeque;

/// The most letters on their way from one end before the other says it
/// holds them.
pub const WINDOW: usize = 256;

/// The bytes of letters on their way past which no more go: a letter goes
/// while fewer are, so the last one may pass the mark.
pub const WINDOW_BYTES: u64 = 8 * 1024 * 1024;

/// One session's sending end of a numbering of letters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sending {
    sent: u64,
    taken: u64,
    on_way: VecDeque<(u64, u64)>,
    bytes: u64,
}

impl Sending {
    /// A session opened with the other end holding the letters through
    /// `taken`.
    pub fn from(taken: u64) -> Self {
        Self { sent: taken, taken, on_way: VecDeque::new(), bytes: 0 }
    }

    /// Whether another letter may go now.
    pub fn room(&self) -> bool {
        self.on_way.len() < WINDOW && self.bytes < WINDOW_BYTES
    }

    /// The last number this session sent, or found not there to send.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Through which number the other end said it holds them.
    pub fn taken(&self) -> u64 {
        self.taken
    }

    pub fn on_way(&self) -> usize {
        self.on_way.len()
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Letter `seq`, the next after those sent, went with `size` bytes; or,
    /// with none, it was not there to go: held or set aside meanwhile.
    pub fn went(&mut self, seq: u64, size: Option<u64>) {
        debug_assert!(seq > self.sent, "letters go in order");
        if let Some(size) = size {
            self.on_way.push_back((seq, size));
            self.bytes += size;
        }
        self.sent = seq;
    }

    /// The other end holds the letters through `through`, as far as this
    /// session sent them; returns how far that is.
    pub fn held(&mut self, through: u64) -> u64 {
        let through = through.min(self.sent);
        self.taken = self.taken.max(through);
        while let Some(&(seq, size)) = self.on_way.front()
            && seq <= through
        {
            self.on_way.pop_front();
            self.bytes -= size;
        }
        through
    }

    /// The other end held the letters through `through` before this
    /// session could send them: none of those goes.
    pub fn skip_through(&mut self, through: u64) {
        self.held(through);
        self.sent = self.sent.max(through);
    }
}

/// Whether letter `seq` of the numbering `epoch` is new to an end that
/// holds through `taken` of the numbering it knows.
pub fn unheld(taken: Option<(u64, u64)>, epoch: u64, seq: u64) -> bool {
    !taken.is_some_and(|(known, taken)| known == epoch && seq <= taken)
}

/// Where an end that holds through `taken` stands in the numbering `epoch`
/// the other end gives its letters now: as far as it held, or at nothing
/// in a numbering it does not know.
pub fn standing(taken: Option<(u64, u64)>, epoch: u64) -> u64 {
    taken.filter(|(known, _)| *known == epoch).map_or(0, |(_, taken)| taken)
}
