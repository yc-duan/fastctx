//! The letters waiting for each member, as the hub sends them: a window at
//! a time, from where the member said it took them, never all at once
//! (`world-concurrency.md`, 机器之间).
//!
//! Which numbers wait on disk for a member is kept in memory beside the
//! files (`Mailbox`), read from its directory once, when first needed, and
//! kept as letters come and go, so neither a receipt nor a session opening
//! lists the directory again. While the member is connected, at most
//! `WINDOW` letters or `WINDOW_BYTES` bytes it has not said it took are on
//! their way to it; each receipt makes room for the next, read from disk off
//! any lock. A letter kept while the member is connected goes to it as soon
//! as the window has room for it, in order, after every letter before it.
//!
//! The records of where every member's letters stand (`letters.json`) are
//! kept by one writer (`keep_records`) as they change, a burst of changes in
//! one write; what keeps a sender's letters from being kept twice, its
//! count, is written in a small file of its own before the hub says it holds
//! them (`crate::store::Store::write_taken`).

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use fastctx_net::envelope::Envelope;
use fastctx_net::wire::Down;
use fastctx_types::MachineName;
use tokio::sync::Notify;

use crate::outq::Outq;
use crate::store::Store;
use crate::{Hub, Out};

/// The most letters on their way to a member that it has not said it took.
const WINDOW: usize = 256;
/// The most bytes of them.
const WINDOW_BYTES: usize = 8 * 1024 * 1024;

/// What the hub knows of the letters waiting for one member.
#[derive(Default)]
pub(crate) struct Mailbox {
    /// Where its letters are kept, once known.
    dir: Option<PathBuf>,
    /// The numbers of the letters on disk for it: those its directory held
    /// when it was read, and those kept since.
    held: BTreeSet<u64>,
    /// Whether its directory was read.
    loaded: bool,
    /// The last letter it said it took.
    taken: u64,
    /// The session its letters go to now.
    to: Option<Sending>,
}

impl Mailbox {
    /// Forgets the letter `seq`, deleted with what it was for.
    pub(crate) fn forget(&mut self, seq: u64) {
        self.held.remove(&seq);
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
    on_way: VecDeque<(u64, usize)>,
    /// Whether letters are being read for it now.
    pumping: bool,
}

impl Sending {
    fn full(&self) -> bool {
        self.on_way.len() >= WINDOW || self.on_way.iter().map(|(_, size)| size).sum::<usize>() >= WINDOW_BYTES
    }
}

/// The session `generation` of `name` opened, having taken the hub's
/// letters through `taken`: those are deleted, and the next go to it.
pub(crate) fn opened(hub: &Arc<Hub>, store: &mut Store, name: &MachineName, generation: u64, out: Outq, taken: u64) {
    let mailbox = store.mailboxes.entry(name.clone()).or_default();
    // A member that numbers the hub's letters afresh takes them all again.
    mailbox.taken = taken;
    mailbox.to = Some(Sending { generation, out, sent: taken, on_way: VecDeque::new(), pumping: false });
    drop_taken(hub, mailbox);
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

/// A letter numbered `seq` is on disk for `to`: it goes out as soon as there
/// is room for it.
pub(crate) fn kept(hub: &Arc<Hub>, store: &mut Store, to: &MachineName, seq: u64) {
    store.mailboxes.entry(to.clone()).or_default().held.insert(seq);
    pump(hub, store, to);
}

/// `name`'s session `generation` took the letters through `through`: they
/// are deleted, and the next go out.
pub(crate) fn taken(hub: &Arc<Hub>, store: &mut Store, name: &MachineName, generation: u64, through: u64) {
    let Some(mailbox) = store.mailboxes.get_mut(name) else { return };
    mailbox.taken = mailbox.taken.max(through);
    if let Some(to) = mailbox.to.as_mut().filter(|to| to.generation == generation) {
        while to.on_way.front().is_some_and(|(seq, _)| *seq <= through) {
            to.on_way.pop_front();
        }
        to.sent = to.sent.max(through);
    }
    drop_taken(hub, mailbox);
    pump(hub, store, name);
}

/// Deletes the letters the member took, off any lock.
fn drop_taken(hub: &Hub, mailbox: &mut Mailbox) {
    let Some(dir) = mailbox.dir.clone() else { return };
    let kept = mailbox.held.split_off(&(mailbox.taken + 1));
    let gone = std::mem::replace(&mut mailbox.held, kept);
    if gone.is_empty() {
        return;
    }
    let log = hub.log.clone();
    tokio::task::spawn_blocking(move || {
        for seq in gone {
            let path = dir.join(format!("{seq:020}"));
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                log.line(format_args!("deleting a letter taken, {}: {error}", path.display()));
            }
        }
    });
}

/// Has letters read for `name` and put on their way, while its session has
/// room for them and none are being read already.
pub(crate) fn pump(hub: &Arc<Hub>, store: &mut Store, name: &MachineName) {
    let Some(mailbox) = store.mailboxes.get_mut(name) else { return };
    let Some(to) = mailbox.to.as_mut() else { return };
    if to.pumping || to.full() {
        return;
    }
    if mailbox.loaded && mailbox.held.range(to.sent + 1..).next().is_none() {
        return;
    }
    to.pumping = true;
    let (hub, name) = (Arc::downgrade(hub), name.clone());
    tokio::spawn(async move { pumping(hub, name).await });
}

/// Reads the next letters for `name`'s session and queues them, until its
/// window is full or no letter is left.
async fn pumping(hub: Weak<Hub>, name: MachineName) {
    loop {
        let Some(hub) = hub.upgrade() else { return };
        // What to read: the directory once, then the next numbers that fit.
        let (dir, load, next, generation) = {
            let mut store = hub.store();
            let dir = match store.outbox_of(&name) {
                Ok(dir) => dir,
                Err(error) => {
                    hub.log.line(format_args!("finding the letters of {name}: {error}"));
                    stop_pumping(&hub, &mut store, &name, true);
                    return;
                }
            };
            let Some(mailbox) = store.mailboxes.get_mut(&name) else { return };
            mailbox.dir = Some(dir.clone());
            let loaded = mailbox.loaded;
            let Some(to) = mailbox.to.as_mut() else { return };
            let room = WINDOW.saturating_sub(to.on_way.len());
            let next: Vec<u64> = mailbox.held.range(to.sent + 1..).take(room).copied().collect();
            // Nothing left to send: done, under the same lock a letter kept
            // takes, so one kept just now starts the reading anew.
            if loaded && next.is_empty() {
                to.pumping = false;
                return;
            }
            (dir, !loaded, next, to.generation)
        };
        if load {
            let listing = {
                let dir = dir.clone();
                tokio::task::spawn_blocking(move || crate::store::letter_numbers(&dir)).await
            };
            let mut store = hub.store();
            match listing.map_err(|error| std::io::Error::other(error.to_string())).and_then(|listed| listed) {
                Ok(numbers) => {
                    let Some(mailbox) = store.mailboxes.get_mut(&name) else { return };
                    mailbox.held.extend(numbers);
                    mailbox.loaded = true;
                    drop_taken(&hub, mailbox);
                }
                Err(error) => {
                    hub.log.line(format_args!("listing the letters of {name}: {error}; listed again in a moment"));
                    stop_pumping(&hub, &mut store, &name, true);
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
                stop_pumping(&hub, &mut store, &name, true);
                return;
            }
        };
        let Some(mailbox) = store.mailboxes.get_mut(&name) else { return };
        let Some(to) = mailbox.to.as_mut().filter(|to| to.generation == generation) else { return };
        let mut failed = false;
        for (seq, letter) in read {
            match letter {
                Ok(Some(message)) => {
                    let frame = Down::Deliver { seq, message };
                    let size = fastctx_net::wire::encode(&frame).len();
                    if to.out.send(Out::Frame(frame)).is_err() {
                        return;
                    }
                    to.on_way.push_back((seq, size));
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
        if failed || to.full() || mailbox.held.range(to.sent + 1..).next().is_none() {
            stop_pumping(&hub, &mut store, &name, failed);
            return;
        }
    }
}

/// Ends the reading for `name`'s session; `again`, after a failure, has it
/// tried again in a moment, as no receipt may come to set it going.
fn stop_pumping(hub: &Arc<Hub>, store: &mut Store, name: &MachineName, again: bool) {
    if let Some(to) = store.mailboxes.get_mut(name).and_then(|mailbox| mailbox.to.as_mut()) {
        to.pumping = false;
    }
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

/// Keeps the records of where every member's letters stand as they change,
/// one write for a burst of changes, off the store's lock.
pub(crate) async fn keep_records(hub: Weak<Hub>, changed: Arc<Notify>) {
    loop {
        changed.notified().await;
        // A burst of changes goes in one write.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let Some(hub) = hub.upgrade() else { return };
        let record = hub.store().letters_record();
        let (bytes, path) = match record {
            Ok(record) => record,
            Err(error) => {
                hub.log.line(format_args!("recording where the letters stand: {error}"));
                continue;
            }
        };
        let written = tokio::task::spawn_blocking(move || {
            fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
        })
        .await;
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
