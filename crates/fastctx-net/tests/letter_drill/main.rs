//! The letters between machines and their hub, run thousands of times over
//! by seed with every fault a line and a process can have
//! (`specs/world-engine.md`, 验证的设施, 确定性模拟).
//!
//! A sending machine posts letters to another through the hub. Each end is
//! modelled as the code does it, with what it keeps on disk apart from what
//! it holds in memory, and its steps in the order the code takes them
//! (`fastctx-hub/src/post/member.rs`, `fastctx-hub/src/control.rs`,
//! `fastctx-net/src/link/session.rs`, `fastctx-net/src/link/letters/taking.rs`,
//! `fastctx-node/src/letters/delivered.rs`); what both ends decide alike is
//! the code's own window (`fastctx_net::window`). Lines are TCP: within one
//! connection nothing is lost or reordered; a cut loses what was on its way.
//! Any end may be stopped between any two of its steps on disk and start
//! again with what it kept; the sending machine may lose its letters' record
//! and number them afresh; the hub refuses letters to a member it does not
//! have.
//!
//! Then, once nothing goes wrong any more and everything has settled:
//! every letter posted reached the receiving machine's business, but those
//! the hub refused; each letter's effect happened once; the effects came in
//! the order the letters were posted; and neither end ever had more on its
//! way than its window lets.
//!
//!     letter_drill [--seeds N] [--seed S] [--letters N]

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use fastctx_net::window::{Sending, WINDOW, WINDOW_BYTES, standing, unheld};

/// A letter as the sending machine's business posts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Letter {
    /// Its identity: the order it was posted in, too.
    id: u64,
    size: u64,
    /// To a member the hub has: one it does not have it refuses.
    known: bool,
}

/// What goes over a line.
#[derive(Clone, Debug)]
enum Frame {
    /// The sender's letter `seq`, up to the hub.
    Send { seq: u64, letter: Letter },
    /// The hub holds the sender's letters through `seq`; `refused`, it
    /// will not deliver this one.
    Held { seq: u64, refused: bool },
    /// The hub's letter `seq` of its numbering for the receiver.
    Deliver { seq: u64, letter: Letter },
    /// The receiver holds the hub's letters through `seq`.
    Taken { seq: u64 },
    /// The sender's heartbeat, saying how far the hub's word on its letters
    /// is kept.
    Ping { held: u64 },
}

/// One connection: what is on its way each way, in order.
#[derive(Default)]
struct Line {
    up: VecDeque<Frame>,
    down: VecDeque<Frame>,
    open: bool,
    /// Which connection it is: what a session sends goes on its own.
    generation: u64,
}

struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn chance(&mut self, per_thousand: u64) -> bool {
        self.below(1000) < per_thousand
    }
}

/// The sending machine (`link/session.rs` `flush`, `held`; `link/calls.rs`
/// `send_letter`; `link/mod.rs` `Link::start`).
struct Sender {
    // Kept: `letters.json` and the outbox.
    epoch: u64,
    held: u64,
    outbox: BTreeMap<u64, Letter>,
    refusals: BTreeSet<u64>,
    // Held in memory.
    next: u64,
    window: Option<Sending>,
    /// Between keeping what the hub holds and deleting those letters.
    deleting: Option<(u64, u64)>,
}

impl Sender {
    fn start(&mut self) {
        // Deletes what the hub held, then numbers on past what is left.
        let held = self.held;
        self.outbox.retain(|seq, _| *seq > held);
        self.next = self.held.max(self.outbox.keys().next_back().copied().unwrap_or(0)) + 1;
        self.window = None;
        self.deleting = None;
    }
}

/// What the hub keeps of one member (`post/member.rs`, `store.rs`).
#[derive(Default)]
struct Member {
    // Kept: the record's row, the count file, the outbox.
    epoch: u64,
    out_epoch: u64,
    /// Through which of the member's own letters the hub counted them.
    taken: u64,
    refused: BTreeMap<u64, ()>,
    /// The row's next number, which a stop may leave behind the outbox.
    next: u64,
    outbox: BTreeMap<u64, Letter>,
    // Held in memory: the session's window, and whether its view is had.
    window: Option<Sending>,
    viewed: bool,
}

impl Member {
    /// What a session of the member is told the hub holds of its letters:
    /// short of the first refusal it has not heard (`control.rs`
    /// `counted_through`).
    fn counted_through(&self) -> u64 {
        match self.refused.keys().next() {
            Some(first) => self.taken.min(first - 1),
            None => self.taken,
        }
    }
}

/// The hub (`control.rs` `intake`, `keep`; `post/member.rs`).
struct Hub {
    sender: Member,
    receiver: Member,
    /// The hub's step that stopped between its letter on disk and its
    /// count: the letter it kept for the receiver, the sender's number.
    putting: Option<(u64, Letter)>,
}

/// The receiving machine (`link/letters/taking.rs`, `link/session.rs`
/// `take_letters`; the business's once, `letters/delivered.rs`).
#[derive(Default)]
struct Receiver {
    // Kept: `letters.json`'s place in the hub's numbering, the receipts of
    // letters taken, the business's receipts and effects.
    taken: Option<(u64, u64)>,
    receipts: BTreeSet<u64>,
    business: BTreeSet<u64>,
    effects: Vec<u64>,
    // Held in memory: the session's connection and the hub's numbering it
    // was welcomed with, the letters delivered and not yet taken, the run
    // being taken and the effects done in it not yet received.
    session: (u64, u64),
    delivered: VecDeque<(u64, Letter)>,
    run: Option<Run>,
}

/// One run of letters being taken, a step at a time, of the session that
/// delivered them.
struct Run {
    session: (u64, u64),
    letters: Vec<(u64, Letter)>,
    at: usize,
    pending: Vec<u64>,
    /// An effect done whose business receipt is not kept yet.
    effect: Option<u64>,
}

/// What happened over a run, to know the faults were there to be weathered.
#[derive(Clone, Copy, Debug, Default)]
struct Tally {
    cuts: u64,
    stops: u64,
    forgotten: u64,
    renumbered: u64,
    refused: u64,
    /// Letters the hub kept twice for the receiver.
    twice: u64,
    /// Letters handed to the business again after a stop.
    again: u64,
    /// The most letters, and bytes, either end had on their way.
    most_on_way: usize,
    most_bytes: u64,
}

impl Tally {
    fn add(&mut self, other: Tally) {
        self.cuts += other.cuts;
        self.stops += other.stops;
        self.forgotten += other.forgotten;
        self.renumbered += other.renumbered;
        self.refused += other.refused;
        self.twice += other.twice;
        self.again += other.again;
        self.most_on_way = self.most_on_way.max(other.most_on_way);
        self.most_bytes = self.most_bytes.max(other.most_bytes);
    }
}

struct Network {
    tally: Tally,
    dice: Dice,
    sender: Sender,
    hub: Hub,
    receiver: Receiver,
    /// The sender's line to the hub, and the receiver's.
    upper: Line,
    lower: Line,
    posted: Vec<Letter>,
    /// How many times the business was handed each letter, and which may
    /// be handed again: their effect was done and its receipt not kept
    /// when the receiver stopped.
    handed: HashMap<u64, u32>,
    again: HashMap<u64, u32>,
    /// Letters lost with the hub's records.
    lost: BTreeSet<u64>,
    lines: u64,
    largest: u64,
    trouble: Vec<String>,
}

impl Network {
    fn new(seed: u64) -> Self {
        let mut network = Self {
            tally: Tally::default(),
            dice: Dice(seed.max(1)),
            sender: Sender {
                epoch: 1,
                held: 0,
                outbox: BTreeMap::new(),
                refusals: BTreeSet::new(),
                next: 1,
                window: None,
                deleting: None,
            },
            hub: Hub {
                sender: Member { epoch: 1, out_epoch: 11, next: 1, ..Member::default() },
                receiver: Member { epoch: 2, out_epoch: 21, next: 1, ..Member::default() },
                putting: None,
            },
            receiver: Receiver::default(),
            upper: Line::default(),
            lower: Line::default(),
            posted: Vec::new(),
            handed: HashMap::new(),
            again: HashMap::new(),
            lost: BTreeSet::new(),
            lines: 0,
            largest: 0,
            trouble: Vec::new(),
        };
        network.sender.start();
        network
    }

    fn check_window(&mut self, window: &Sending, who: &str) {
        self.tally.most_on_way = self.tally.most_on_way.max(window.on_way());
        self.tally.most_bytes = self.tally.most_bytes.max(window.bytes());
        if window.on_way() > WINDOW || window.bytes() > WINDOW_BYTES + self.largest {
            self.trouble.push(format!("{who} had {} letters, {} bytes on their way", window.on_way(), window.bytes()));
        }
    }

    // The sending machine's steps.

    fn post(&mut self) {
        let id = self.posted.len() as u64 + 1;
        let size = if self.dice.chance(100) { 150_000 + self.dice.below(150_000) } else { 200 + self.dice.below(2_000) };
        self.largest = self.largest.max(size);
        let letter = Letter { id, size, known: !self.dice.chance(15) };
        // Written whole, then numbered on (`send_letter`).
        let seq = self.sender.next;
        self.sender.outbox.insert(seq, letter);
        self.sender.next += 1;
        self.posted.push(letter);
    }

    fn connect_upper(&mut self) {
        // The hub opens the session: a numbering it does not know is
        // counted afresh (`member.rs`, `Open`).
        let member = &mut self.hub.sender;
        if member.epoch != self.sender.epoch {
            member.epoch = self.sender.epoch;
            member.taken = 0;
            member.refused.clear();
        }
        let taken = member.counted_through();
        self.lines += 1;
        self.upper = Line { open: true, generation: self.lines, ..Line::default() };
        // The welcome: the machine keeps what the hub holds before sending.
        self.sender_held(taken);
        self.finish_deleting();
        self.sender.window = Some(Sending::from(taken));
    }

    fn sender_held(&mut self, seq: u64) {
        // Kept before the letters are deleted (`session.rs` `held`), one
        // such change at a time.
        self.finish_deleting();
        if seq > self.sender.held {
            let before = self.sender.held;
            self.sender.held = seq;
            self.sender.next = self.sender.next.max(seq + 1);
            self.sender.deleting = Some((before, seq));
        }
    }

    fn finish_deleting(&mut self) {
        if let Some((before, seq)) = self.sender.deleting.take() {
            self.sender.outbox.retain(|number, _| *number <= before || *number > seq);
        }
    }

    fn flush(&mut self) {
        let Some(mut window) = self.sender.window.take() else { return };
        window.skip_through(self.sender.held);
        if window.sent() + 1 < self.sender.next && window.room() {
            let seq = window.sent() + 1;
            match self.sender.outbox.get(&seq).copied() {
                Some(letter) => {
                    self.upper.up.push_back(Frame::Send { seq, letter });
                    window.went(seq, Some(letter.size));
                }
                None => window.went(seq, None),
            }
            self.check_window(&window, "the sending machine");
        }
        self.sender.window = Some(window);
    }

    fn sender_hears(&mut self) {
        let Some(frame) = self.upper.down.pop_front() else { return };
        if let Frame::Held { seq, refused } = frame {
            if refused {
                self.sender.refusals.insert(seq);
            }
            self.sender_held(seq);
            if let Some(window) = self.sender.window.as_mut() {
                window.held(seq);
            }
        }
    }

    fn ping(&mut self) {
        if self.upper.open {
            self.upper.up.push_back(Frame::Ping { held: self.sender.held });
        }
    }

    // The hub's steps.

    /// Takes the sending machine's next frame: a letter is put in the
    /// receiver's outbox, then counted, then said to be held (`keep`).
    fn intake(&mut self) {
        if let Some((seq, letter)) = self.hub.putting.take() {
            self.count(seq, letter);
            return;
        }
        let Some(frame) = self.upper.up.pop_front() else { return };
        match frame {
            Frame::Send { seq, letter } => {
                let member = &self.hub.sender;
                if seq <= member.taken {
                    let refused = member.refused.contains_key(&seq);
                    self.upper.down.push_back(Frame::Held { seq, refused });
                    return;
                }
                if !letter.known {
                    self.tally.refused += 1;
                    let member = &mut self.hub.sender;
                    member.refused.insert(seq, ());
                    member.taken = member.taken.max(seq);
                    self.upper.down.push_back(Frame::Held { seq, refused: true });
                    return;
                }
                // On disk under the receiver's next number; the row is
                // written after, and a stop between leaves it behind.
                let receiver = &mut self.hub.receiver;
                let number = receiver.next.max(receiver.outbox.keys().next_back().map_or(0, |last| last + 1));
                if receiver.outbox.values().any(|kept| kept.id == letter.id) {
                    self.tally.twice += 1;
                }
                receiver.outbox.insert(number, letter);
                receiver.next = number + 1;
                self.hub.putting = Some((seq, letter));
            }
            Frame::Ping { held } => {
                // Refusals the machine heard are let go (`Heard`).
                self.hub.sender.refused.retain(|seq, _| *seq > held);
            }
            _ => {}
        }
    }

    fn count(&mut self, seq: u64, _letter: Letter) {
        let member = &mut self.hub.sender;
        member.taken = member.taken.max(seq);
        if self.upper.open {
            self.upper.down.push_back(Frame::Held { seq, refused: false });
        }
    }

    fn connect_lower(&mut self) {
        // The receiver says where it stands in the hub's numbering; the
        // hub deletes what it holds, then sends on past it.
        let member = &mut self.hub.receiver;
        let taken = standing(self.receiver.taken, member.out_epoch);
        member.outbox.retain(|seq, _| *seq > taken);
        member.window = Some(Sending::from(taken));
        member.viewed = false;
        self.lines += 1;
        self.lower = Line { open: true, generation: self.lines, ..Line::default() };
        // The welcome: a numbering the receiver does not know starts it at
        // nothing (`session.rs` `connected`).
        let out_epoch = member.out_epoch;
        self.receiver.session = (self.lines, out_epoch);
        if self.receiver.taken.is_none_or(|(epoch, _)| epoch != out_epoch) {
            self.receiver.taken = Some((out_epoch, 0));
        }
    }

    fn pump(&mut self) {
        let member = &mut self.hub.receiver;
        let Some(mut window) = member.window.take() else { return };
        if member.viewed && window.room() {
            let next = member.outbox.range(window.sent() + 1..).next().map(|(seq, letter)| (*seq, *letter));
            if let Some((seq, letter)) = next {
                self.lower.down.push_back(Frame::Deliver { seq, letter });
                window.went(seq, Some(letter.size));
                self.check_window(&window, "the hub");
            }
        }
        self.hub.receiver.window = Some(window);
    }

    fn hub_hears(&mut self) {
        let Some(frame) = self.lower.up.pop_front() else { return };
        if let Frame::Taken { seq } = frame {
            let member = &mut self.hub.receiver;
            if let Some(window) = member.window.as_mut() {
                let through = window.held(seq);
                member.outbox.retain(|number, _| *number > through);
            }
        }
    }

    // The receiving machine's steps.

    fn deliver(&mut self) {
        if let Some(Frame::Deliver { seq, letter }) = self.lower.down.pop_front() {
            self.receiver.delivered.push_back((seq, letter));
        }
    }

    /// One step of taking a run: one letter, or the receipts, the place
    /// and the word to the hub at its end (`taking.rs` `take`).
    fn take(&mut self) {
        let receiver = &mut self.receiver;
        if receiver.run.is_none() {
            if receiver.delivered.is_empty() {
                return;
            }
            let count = receiver.delivered.len().min(64);
            let letters = receiver.delivered.drain(..count).collect();
            receiver.run = Some(Run { session: receiver.session, letters, at: 0, pending: Vec::new(), effect: None });
        }
        let run = receiver.run.as_mut().expect("a run");
        let (generation, epoch) = run.session;
        if let Some(id) = run.effect.take() {
            // The business's receipt, kept after its effect.
            receiver.business.insert(id);
            return;
        }
        if let Some(&(seq, letter)) = run.letters.get(run.at) {
            run.at += 1;
            // A receipt kept, or one of this run's (`recent`), takes it once.
            if unheld(receiver.taken, epoch, seq)
                && !receiver.receipts.contains(&letter.id)
                && !run.pending.contains(&letter.id)
            {
                *self.handed.entry(letter.id).or_default() += 1;
                if !receiver.business.contains(&letter.id) {
                    // The effect is kept under the letter's identity, so a
                    // second one finds it there (`received.rs` `keep`).
                    if !receiver.effects.contains(&letter.id) {
                        receiver.effects.push(letter.id);
                    }
                    run.effect = Some(letter.id);
                }
                run.pending.push(letter.id);
            }
            return;
        }
        // The run's end: receipts, then the place, then the word.
        let run = receiver.run.take().expect("a run");
        receiver.receipts.extend(run.pending);
        let last = run.letters.last().expect("runs are not empty").0;
        let before = receiver.taken;
        if unheld(receiver.taken, epoch, last) {
            receiver.taken = Some((epoch, last));
        }
        if let (Some((was, then)), Some((is, now))) = (before, receiver.taken)
            && was == is
            && now < then
        {
            self.trouble.push(format!("the receiver's place went back from {then} to {now}"));
        }
        // The word goes on the connection the letters came on, while it is
        // there.
        if self.lower.open && self.lower.generation == generation {
            self.lower.up.push_back(Frame::Taken { seq: last });
        }
    }

    // What goes wrong.

    fn cut_upper(&mut self) {
        self.tally.cuts += 1;
        self.upper = Line::default();
        self.sender.window = None;
        // A letter on disk whose session ended before its count is not
        // counted: the session it came on was replaced (`member.rs`, `Count`).
        self.hub.putting = None;
    }

    fn cut_lower(&mut self) {
        self.tally.cuts += 1;
        self.lower = Line::default();
        self.hub.receiver.window = None;
        self.receiver.delivered.clear();
    }

    fn stop_sender(&mut self) {
        self.tally.stops += 1;
        self.cut_upper();
        self.sender.start();
    }

    fn stop_hub(&mut self) {
        self.tally.stops += 1;
        self.cut_upper();
        self.cut_lower();
        // A letter on disk whose count was not kept is sent again.
        self.hub.putting = None;
    }

    fn stop_receiver(&mut self) {
        self.cut_lower();
        // The run's letters whose receipts were not kept are handed to the
        // business again; its own receipt keeps their effect to once.
        self.tally.stops += 1;
        if let Some(run) = self.receiver.run.take() {
            for id in run.pending.iter().chain(run.effect.iter()) {
                *self.again.entry(*id).or_default() += 1;
            }
        }
    }

    /// The hub loses its records of both members: it numbers its letters
    /// to the receiver afresh, and knows nothing of the sender's. What it
    /// held for the receiver is lost with them; what is posted after must
    /// all arrive.
    fn renumber_hub(&mut self) {
        self.tally.renumbered += 1;
        self.cut_upper();
        self.cut_lower();
        self.lost.extend(self.hub.receiver.outbox.values().map(|letter| letter.id));
        let receiver = Member { epoch: 2, out_epoch: self.hub.receiver.out_epoch + 1000, next: 1, ..Member::default() };
        let sender = Member { epoch: 0, out_epoch: self.hub.sender.out_epoch + 1000, next: 1, ..Member::default() };
        self.hub = Hub { sender, receiver, putting: None };
    }

    /// The sending machine loses its letters' record: it numbers afresh,
    /// and what is left in its outbox goes again under the new numbering.
    fn forget_sender(&mut self) {
        self.tally.forgotten += 1;
        self.cut_upper();
        self.sender.epoch += 1000;
        self.sender.held = 0;
        self.sender.start();
    }

    fn step(&mut self, faults: bool, posting: bool) {
        let dice = &mut self.dice;
        if faults {
            if dice.chance(4) {
                return self.cut_upper();
            }
            if dice.chance(4) {
                return self.cut_lower();
            }
            if dice.chance(2) {
                return self.stop_sender();
            }
            if dice.chance(2) {
                return self.stop_hub();
            }
            if dice.chance(2) {
                return self.stop_receiver();
            }
            if dice.chance(1) && dice.chance(200) {
                return self.forget_sender();
            }
            if dice.chance(1) && dice.chance(100) {
                return self.renumber_hub();
            }
        }
        match self.dice.below(12) {
            0 if posting => self.post(),
            1 if !self.upper.open && self.dice.chance(300) => self.connect_upper(),
            2 if !self.lower.open && self.dice.chance(300) => self.connect_lower(),
            3 => self.flush(),
            4 => self.intake(),
            5 => self.sender_hears(),
            6 => self.finish_deleting(),
            7 => self.pump(),
            8 => self.deliver(),
            9 => self.take(),
            10 => self.hub_hears(),
            11 => {
                if self.lower.open && self.dice.chance(100) {
                    self.hub.receiver.viewed = true;
                }
                if self.dice.chance(50) {
                    self.ping();
                }
            }
            _ => {}
        }
    }

    /// Whether everything has gone as far as it can: nothing posted waits
    /// anywhere.
    fn settled(&self) -> bool {
        self.sender.outbox.is_empty()
            && self.hub.receiver.outbox.is_empty()
            && self.hub.putting.is_none()
            && self.receiver.run.is_none()
            && self.receiver.delivered.is_empty()
    }

    /// What went wrong, if anything, once settled.
    fn judge(&self) -> Vec<String> {
        let mut wrong = self.trouble.clone();
        let effects: BTreeSet<u64> = self.receiver.effects.iter().copied().collect();
        if effects.len() != self.receiver.effects.len() {
            wrong.push("a letter's effect happened twice".into());
        }
        for letter in &self.posted {
            let reached = effects.contains(&letter.id);
            if letter.known && !reached && !self.lost.contains(&letter.id) && wrong.len() < 20 {
                wrong.push(format!("letter {} never reached the business", letter.id));
            }
            if !letter.known && reached {
                wrong.push(format!("letter {} to no one reached the business", letter.id));
            }
        }
        let mut last = 0;
        for id in &self.receiver.effects {
            if *id < last {
                wrong.push(format!("letter {id} took effect after letter {last}"));
                break;
            }
            last = *id;
        }
        for (id, handed) in &self.handed {
            let allowed = 1 + self.again.get(id).copied().unwrap_or(0);
            if *handed > allowed {
                wrong.push(format!("letter {id} was handed to the business {handed} times, {allowed} allowed"));
            }
        }
        wrong
    }
}

/// Runs one seed: posting with faults, then calm until settled.
fn run(seed: u64, letters: usize) -> Result<Tally, String> {
    let mut network = Network::new(seed);
    let mut steps = 0u64;
    while network.posted.len() < letters {
        network.step(true, true);
        steps += 1;
        if steps > letters as u64 * 4_000 {
            return Err(format!("seed {seed}: posting did not finish"));
        }
    }
    let calm = steps;
    while !network.settled() {
        network.step(false, false);
        steps += 1;
        if steps - calm > 20_000_000 {
            return Err(format!("seed {seed}: the letters did not settle once calm; {}", network.state()));
        }
    }
    let wrong = network.judge();
    let mut tally = network.tally;
    tally.again = network.handed.values().map(|handed| u64::from(*handed).saturating_sub(1)).sum();
    if wrong.is_empty() { Ok(tally) } else { Err(format!("seed {seed}: {}", wrong.join("; "))) }
}

impl Network {
    fn state(&self) -> String {
        format!(
            "sender outbox {} held {} next {}, hub outbox {} taken {}, receiver taken {:?} delivered {}",
            self.sender.outbox.len(),
            self.sender.held,
            self.sender.next,
            self.hub.receiver.outbox.len(),
            self.hub.sender.taken,
            self.receiver.taken,
            self.receiver.delivered.len()
        )
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str| args.iter().position(|arg| arg == name).and_then(|at| args.get(at + 1)).and_then(|v| v.parse::<u64>().ok());
    let letters = value("--letters").unwrap_or(600) as usize;
    let seeds: Vec<u64> = match value("--seed") {
        Some(seed) => vec![seed],
        None => (1..=value("--seeds").unwrap_or(2000)).map(|n| n.wrapping_mul(0x9e37_79b9_7f4a_7c15)).collect(),
    };
    let began = std::time::Instant::now();
    let mut failed = Vec::new();
    let mut tally = Tally::default();
    for seed in &seeds {
        match run(*seed, letters) {
            Ok(seen) => tally.add(seen),
            Err(error) => {
                println!("FAIL {error}");
                failed.push(*seed);
                if failed.len() >= 10 {
                    break;
                }
            }
        }
    }
    println!("WEATHERED {tally:?}");
    // Each fault must have happened for its outcome to mean anything.
    let weathered = tally.cuts > 0
        && tally.stops > 0
        && tally.forgotten > 0
        && tally.renumbered > 0
        && tally.refused > 0
        && tally.twice > 0
        && tally.again > 0
        && tally.most_on_way == WINDOW
        && tally.most_bytes >= WINDOW_BYTES;
    println!(
        "LETTERS: {} seeds of {letters} letters, {} failed, {} in {:.1} s",
        seeds.len(),
        failed.len(),
        if weathered { "every fault weathered" } else { "NOT every fault came" },
        began.elapsed().as_secs_f64()
    );
    if !failed.is_empty() || (seeds.len() > 1 && !weathered) {
        std::process::exit(1);
    }
}
