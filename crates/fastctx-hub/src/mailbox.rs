//! The letters waiting for each member, as the hub sends them: a window at
//! a time, from where the member said it took them, never all at once
//! (`world-concurrency.md`, 机器之间).
//!
//! Which numbers wait on disk for a member, and how large each is, is kept
//! in memory beside the files (`Mailbox`), read from its directory once,
//! when first needed, and kept as letters come and go, so neither a receipt
//! nor a session opening lists the directory again. While the member is
//! connected, at most `WINDOW` letters or `WINDOW_BYTES` bytes it has not
//! said it took are on their way to it; each receipt makes room for the
//! next, read from disk off any lock. Letters go in the order the hub
//! numbered them: a number given out is not passed until its letter is
//! written or given up, so a letter written sooner never goes ahead of one
//! numbered before it, and a receipt never counts for a letter not sent.
//!
//! A letter the member took is deleted only once the records number past
//! it (`crate::records`): until then a stop would give its number out again,
//! to a letter the member would take for one it took.
//!
//! The records of where every member's letters stand (`letters.json`) are
//! kept by one writer (`keep_records`) as they change, a burst of changes in
//! one write; what keeps a sender's letters from being kept twice, its
//! count, is written in a small file of its own before the hub says it holds
//! them (`crate::store::Store::write_count`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use fastctx_net::envelope::Envelope;
use fastctx_net::wire::Down;
use fastctx_types::{Address, MachineName};
use tokio::sync::Notify;

use crate::outq::Outq;
use crate::store::Store;
use crate::{Hub, Out};

/// The most letters on their way to a member that it has not said it took.
const WINDOW: usize = 256;
/// The most bytes of them.
const WINDOW_BYTES: u64 = 8 * 1024 * 1024;

/// What the hub knows of the letters waiting for one member, of one
/// numbering of them (`crate::store::Letters::out_epoch`).
pub(crate) struct Mailbox {
    /// The numbering they are of.
    epoch: u64,
    /// Where its letters are kept, once known.
    dir: Option<PathBuf>,
    /// The letters on disk for it, by number, and how large each is: those
    /// its directory held when it was read, and those kept since.
    held: BTreeMap<u64, u64>,
    /// The numbers given to letters for it that are being written now.
    writing: BTreeSet<u64>,
    /// Whether its directory was read.
    loaded: bool,
    /// The last letter it said it took.
    taken: u64,
    /// The session its letters go to now.
    to: Option<Sending>,
}

impl Mailbox {
    fn new(epoch: u64) -> Self {
        Self { epoch, dir: None, held: BTreeMap::new(), writing: BTreeSet::new(), loaded: false, taken: 0, to: None }
    }

    /// The first number no letter goes past yet: one still being written.
    fn frontier(&self) -> u64 {
        self.writing.first().copied().unwrap_or(u64::MAX)
    }

    /// Whether a letter waits that may go to `to` now.
    fn waiting(&self, to: &Sending) -> bool {
        self.held.range(to.sent + 1..self.frontier().max(to.sent + 1)).next().is_some()
    }
}

/// Where one session of a member stands with its letters.
struct Sending {
    generation: u64,
    out: Outq,
    /// The last letter put in its queue.
    sent: u64,
    /// The letters put in its queue that it has not said it took, and how
    /// large each is.
    on_way: VecDeque<(u64, u64)>,
    on_way_bytes: u64,
    /// Whether letters are being read for it now.
    pumping: bool,
}

impl Sending {
    fn full(&self) -> bool {
        self.on_way.len() >= WINDOW || self.on_way_bytes >= WINDOW_BYTES
    }
}

/// How many letters wait for one member, for the hub's health.
#[derive(Clone)]
pub(crate) struct Backlog {
    pub member: MachineName,
    pub held: usize,
    pub writing: usize,
    pub on_way: usize,
}

/// How many letters wait for each member the hub knows of now.
pub(crate) fn backlog(store: &Store) -> Vec<Backlog> {
    store
        .mailboxes
        .iter()
        .map(|(member, mailbox)| Backlog {
            member: member.clone(),
            held: mailbox.held.len(),
            writing: mailbox.writing.len(),
            on_way: mailbox.to.as_ref().map_or(0, |to| to.on_way.len()),
        })
        .collect()
}

/// The mailbox of `name`'s letters numbered `epoch`. One of another
/// numbering is of a member taken off whose name `name` took, and goes.
fn of<'a>(store: &'a mut Store, name: &MachineName, epoch: u64) -> &'a mut Mailbox {
    let mailbox = store.mailboxes.entry(name.clone()).or_insert_with(|| Mailbox::new(epoch));
    if mailbox.epoch != epoch {
        *mailbox = Mailbox::new(epoch);
    }
    mailbox
}

/// The mailbox of `name`'s letters if they are numbered `epoch`.
fn numbered_as<'a>(store: &'a mut Store, name: &MachineName, epoch: u64) -> Option<&'a mut Mailbox> {
    store.mailboxes.get_mut(name).filter(|mailbox| mailbox.epoch == epoch)
}

/// The session `generation` of `name`, whose letters are numbered `epoch`,
/// opened, having taken the hub's letters through `taken`: those are
/// deleted, and the next go to it.
pub(crate) fn opened(
    hub: &Arc<Hub>,
    store: &mut Store,
    name: &MachineName,
    epoch: u64,
    generation: u64,
    out: Outq,
    taken: u64,
) {
    let mailbox = of(store, name, epoch);
    // A member that numbers the hub's letters afresh takes them all again.
    mailbox.taken = taken;
    mailbox.to =
        Some(Sending { generation, out, sent: taken, on_way: VecDeque::new(), on_way_bytes: 0, pumping: false });
    drop_taken(hub, store, name);
    pump(hub, store, name);
}

/// The session `generation` of `name` ended.
pub(crate) fn closed(store: &mut Store, name: &MachineName, generation: u64) {
    if let Some(mailbox) = store.mailboxes.get_mut(name)
        && mailbox.to.as_ref().is_some_and(|to| to.generation == generation)
    {
        mailbox.to = None;
    }
}

/// The number `seq` of the numbering `epoch` was given to a letter for
/// `to`, being written now (`Store::next_letter`).
pub(crate) fn numbered(store: &mut Store, to: &MachineName, epoch: u64, seq: u64) {
    of(store, to, epoch).writing.insert(seq);
}

/// The letter numbered `seq` of `epoch`, of `size` bytes, is on disk for
/// `to`: it goes out as soon as there is room for it.
pub(crate) fn kept(hub: &Arc<Hub>, store: &mut Store, to: &MachineName, epoch: u64, seq: u64, size: u64) {
    let mailbox = of(store, to, epoch);
    mailbox.writing.remove(&seq);
    mailbox.held.insert(seq, size);
    pump(hub, store, to);
}

/// The letter numbered `seq` of `epoch` for `to` was not written, or not
/// for the member `to` names now: its number is a gap, and the letters
/// after it go on. Nothing of another numbering's is touched.
pub(crate) fn abandoned(hub: &Arc<Hub>, store: &mut Store, to: &MachineName, epoch: u64, seq: u64) {
    if let Some(mailbox) = numbered_as(store, to, epoch) {
        mailbox.writing.remove(&seq);
    }
    pump(hub, store, to);
}

/// The member called `old` is called `new`: what is known of its letters,
/// those being written among them, goes on under the new name, its session
/// having ended with the rename.
pub(crate) fn renamed(
    mailboxes: &mut std::collections::HashMap<MachineName, Mailbox>,
    old: &MachineName,
    new: &MachineName,
) {
    if let Some(mut mailbox) = mailboxes.remove(old) {
        mailbox.to = None;
        mailboxes.insert(new.clone(), mailbox);
    }
}

/// `name`'s session `generation` took the letters through `through`: they
/// are deleted, and the next go out. A receipt counts only for letters its
/// session was sent; one from a session replaced since is not needed, the
/// new one having said where the member stands.
pub(crate) fn taken(hub: &Arc<Hub>, store: &mut Store, name: &MachineName, generation: u64, through: u64) {
    let Some(mailbox) = store.mailboxes.get_mut(name) else { return };
    let Some(to) = mailbox.to.as_mut().filter(|to| to.generation == generation) else { return };
    let through = through.min(to.sent);
    while let Some((seq, size)) = to.on_way.front().copied()
        && seq <= through
    {
        to.on_way.pop_front();
        to.on_way_bytes -= size;
    }
    mailbox.taken = mailbox.taken.max(through);
    drop_taken(hub, store, name);
    pump(hub, store, name);
}

/// Deletes the letters `name` took, off any lock, once the records number
/// past them: those of the revision now, which are written soon however it
/// came, as a sender's count moves it without them.
fn drop_taken(hub: &Arc<Hub>, store: &mut Store, name: &MachineName) {
    if store.records.written_revision() < store.revision() {
        let _ = store.save_letters();
    }
    let (revision, records) = (store.revision(), store.records.clone());
    let Some(mailbox) = store.mailboxes.get_mut(name) else { return };
    let Some(dir) = mailbox.dir.clone() else { return };
    let kept = mailbox.held.split_off(&(mailbox.taken + 1));
    let gone: Vec<u64> = std::mem::replace(&mut mailbox.held, kept).into_keys().collect();
    if gone.is_empty() {
        return;
    }
    let log = hub.log.clone();
    let held = crate::moving::Moving::hold(hub);
    tokio::spawn(async move {
        let _held = held;
        records.written(revision).await;
        let _ = tokio::task::spawn_blocking(move || {
            for seq in gone {
                let path = dir.join(format!("{seq:020}"));
                if let Err(error) = std::fs::remove_file(&path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    log.line(format_args!("deleting a letter taken, {}: {error}", path.display()));
                }
            }
        })
        .await;
    });
}

/// Has letters read for `name` and put on their way, while its session has
/// room for them and none are being read already.
pub(crate) fn pump(hub: &Arc<Hub>, store: &mut Store, name: &MachineName) {
    let Some(mailbox) = store.mailboxes.get_mut(name) else { return };
    let Some(to) = mailbox.to.as_ref() else { return };
    if to.pumping || to.full() || (mailbox.loaded && !mailbox.waiting(to)) {
        return;
    }
    let generation = to.generation;
    mailbox.to.as_mut().expect("looked at above").pumping = true;
    let (hub, name) = (Arc::downgrade(hub), name.clone());
    tokio::spawn(async move { pumping(hub, name, generation).await });
}

/// The mailbox of `name`, while the session its letters go to is still
/// `generation`.
fn sending<'a>(store: &'a mut Store, name: &MachineName, generation: u64) -> Option<&'a mut Mailbox> {
    let mailbox = store.mailboxes.get_mut(name)?;
    mailbox.to.as_ref().filter(|to| to.generation == generation)?;
    Some(mailbox)
}

/// Reads the next letters for the session `generation` of `name` and
/// queues them, until its window is full or no letter is left. A session
/// replaced since has its own reading: this one leaves everything to it.
async fn pumping(hub: Weak<Hub>, name: MachineName, generation: u64) {
    loop {
        let Some(hub) = hub.upgrade() else { return };
        // What to read: the directory once, then the next numbers that fit.
        let (dir, load, next) = {
            let mut store = hub.store();
            let Some((_, dir)) = store.outbox_of(&name) else {
                hub.log.line(format_args!("finding the letters of {name}: it has no records of them"));
                stop_pumping(&hub, &mut store, &name, generation, true);
                return;
            };
            let Some(mailbox) = sending(&mut store, &name, generation) else { return };
            mailbox.dir = Some(dir.clone());
            let frontier = mailbox.frontier();
            let to = mailbox.to.as_mut().expect("its session");
            let (mut room, mut bytes) = (WINDOW.saturating_sub(to.on_way.len()), to.on_way_bytes);
            let mut next = Vec::new();
            for (seq, size) in mailbox.held.range(to.sent + 1..frontier.max(to.sent + 1)) {
                // The first always goes, whatever its size, once the window
                // is empty enough to take any.
                if room == 0 || (bytes + size > WINDOW_BYTES && !next.is_empty()) {
                    break;
                }
                next.push(*seq);
                room -= 1;
                bytes += size;
            }
            // Nothing left to send: done, under the same lock a letter kept
            // takes, so one kept just now starts the reading anew.
            if mailbox.loaded && next.is_empty() {
                to.pumping = false;
                return;
            }
            (dir, !mailbox.loaded, next)
        };
        if load {
            let listing = {
                let dir = dir.clone();
                tokio::task::spawn_blocking(move || crate::store::letter_numbers(&dir)).await
            };
            let mut store = hub.store();
            match listing.map_err(|error| std::io::Error::other(error.to_string())).and_then(|listed| listed) {
                Ok(numbers) => {
                    let Some(mailbox) = sending(&mut store, &name, generation) else { return };
                    // Those being written now are kept as they are written.
                    let writing = mailbox.writing.clone();
                    mailbox.held.extend(numbers.into_iter().filter(|(seq, _)| !writing.contains(seq)));
                    mailbox.loaded = true;
                    drop_taken(&hub, &mut store, &name);
                }
                Err(error) => {
                    hub.log.line(format_args!("listing the letters of {name}: {error}; listed again in a moment"));
                    stop_pumping(&hub, &mut store, &name, generation, true);
                    return;
                }
            }
            continue;
        }
        let read = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || {
                next.into_iter()
                    .map(|seq| (seq, crate::store::read_letter(&dir, seq)))
                    .collect::<Vec<(u64, std::io::Result<Option<Envelope>>)>>()
            })
            .await
        };
        let mut store = hub.store();
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                hub.log.line(format_args!("reading the letters of {name}: {error}; read again in a moment"));
                stop_pumping(&hub, &mut store, &name, generation, true);
                return;
            }
        };
        let Some(mailbox) = sending(&mut store, &name, generation) else { return };
        let to = mailbox.to.as_mut().expect("its session");
        let mut failed = false;
        for (seq, letter) in read {
            match letter {
                Ok(Some(message)) => {
                    let size = mailbox.held.get(&seq).copied().unwrap_or_default();
                    if to.out.send(Out::Frame(Down::Deliver { seq, message })).is_err() {
                        return;
                    }
                    to.on_way.push_back((seq, size));
                    to.on_way_bytes += size;
                    to.sent = seq;
                }
                // Deleted meanwhile, as a conversation's letters are when it
                // is deleted.
                Ok(None) => {
                    mailbox.held.remove(&seq);
                    to.sent = to.sent.max(seq);
                }
                Err(error) => {
                    hub.log.line(format_args!("reading letter {seq} of {name}: {error}; read again in a moment"));
                    failed = true;
                    break;
                }
            }
        }
        let done = to.full() || !mailbox.waiting(mailbox.to.as_ref().expect("its session"));
        if failed || done {
            stop_pumping(&hub, &mut store, &name, generation, failed);
            return;
        }
    }
}

/// Ends the reading for the session `generation` of `name`; `again`, after
/// a failure, has it tried again in a moment, as no receipt may come to set
/// it going.
fn stop_pumping(hub: &Arc<Hub>, store: &mut Store, name: &MachineName, generation: u64, again: bool) {
    let Some(mailbox) = sending(store, name, generation) else { return };
    mailbox.to.as_mut().expect("its session").pumping = false;
    if again {
        let (hub, name) = (Arc::downgrade(hub), name.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if let Some(hub) = hub.upgrade() {
                pump(&hub, &mut hub.store(), &name);
            }
        });
    }
}

/// Deletes the letters waiting for the conversation `conversation`, being
/// deleted, off any lock, once the records number past them; says how many.
/// A part of a letter too large for one message names nobody, and stays.
pub(crate) async fn drop_for(hub: &Arc<Hub>, conversation: &Address) -> std::io::Result<usize> {
    let (epoch, dir, records) = {
        let store = hub.store();
        let Some((epoch, dir)) = store.outbox_of(&conversation.machine) else { return Ok(0) };
        (epoch, dir, store.records.clone())
    };
    let resident = conversation.resident.clone();
    let found = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<(u64, PathBuf)>> {
        use fastctx_types::remote::Letter;
        let mut found = Vec::new();
        for (seq, _) in crate::store::letter_numbers(&dir)? {
            let Some(message) = crate::store::read_letter(&dir, seq)? else { continue };
            let Ok(body) = message.body::<fastctx_net::message::LetterBody>() else { continue };
            let to = match &body.letter {
                Letter::JobFacts { to, .. } | Letter::Notice { to, .. } | Letter::Speak { to, .. } => to,
                Letter::Fragment { .. }
                | Letter::Left { .. }
                | Letter::Control { .. }
                | Letter::ControlResult { .. } => continue,
            };
            if *to == resident {
                found.push((seq, dir.join(format!("{seq:020}"))));
            }
        }
        Ok(found)
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    if found.is_empty() {
        return Ok(0);
    }
    // Each letter found was numbered before it was written, so before it
    // was found: the records kept from now on number past all of them,
    // also one numbered as they were being looked for.
    let revision = {
        let mut store = hub.store();
        store.save_letters()?;
        store.revision()
    };
    records.written(revision).await;
    {
        let mut store = hub.store();
        if let Some(mailbox) = numbered_as(&mut store, &conversation.machine, epoch) {
            for (seq, _) in &found {
                mailbox.held.remove(seq);
            }
        }
    }
    let dropped = found.len();
    tokio::task::spawn_blocking(move || {
        for (_, path) in found {
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    Ok(dropped)
}

/// The letters `seqs` of `name`'s numbering `epoch` are settled
/// (`crate::expiry`): they are no longer waiting.
pub(crate) fn settled(store: &mut Store, name: &MachineName, epoch: u64, seqs: impl IntoIterator<Item = u64>) {
    if let Some(mailbox) = numbered_as(store, name, epoch) {
        for seq in seqs {
            mailbox.held.remove(&seq);
        }
    }
}

/// Keeps the records of where every member's letters stand as they change,
/// one write for a burst of changes, off the store's lock.
pub(crate) async fn keep_records(hub: Weak<Hub>, changed: Arc<Notify>) {
    loop {
        changed.notified().await;
        // A burst of changes goes in one write.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let Some(hub) = hub.upgrade() else { return };
        let ((revision, letters, shelf), records) = {
            let mut store = hub.store();
            (store.letters_record(), store.records.clone())
        };
        let written = tokio::task::spawn_blocking(move || records.write(revision, &shelf, &letters)).await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                hub.log.line(format_args!("recording where the letters stand: {error}; trying again"));
                tokio::time::sleep(Duration::from_secs(1)).await;
                changed.notify_one();
            }
            Err(error) => hub.log.line(format_args!("recording where the letters stand: {error}")),
        }
    }
}
