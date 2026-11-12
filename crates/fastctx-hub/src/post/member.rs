//! One member's ordered mail state. No shared guard survives a disk await.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use arc_swap::ArcSwapOption;
use fastctx_net::sign::PublicKey;
use fastctx_net::window::Sending as Window;
use fastctx_types::MachineName;
use tokio::sync::mpsc;

use super::{Command, Result};
use crate::store::{Letters, Taken, shelf::Shelf};

#[derive(Default)]
pub(crate) struct Backlog {
    pub held: AtomicUsize,
    pub on_way: AtomicUsize,
    pub writing: AtomicUsize,
    pub pending: AtomicUsize,
    /// Whether `held` was counted from the mailbox in this run of the hub:
    /// until then it says nothing, not that none wait.
    pub counted: AtomicBool,
}

/// What the mailbox holds, without listing it: no letter numbered below
/// `first` is there, and `count` letters are. Each letter's size is looked
/// up as it goes, so a long backlog takes no memory here and no look at
/// every file when the hub starts.
#[derive(Clone, Copy, Default)]
struct Held {
    first: u64,
    count: usize,
}

struct Sending {
    generation: u64,
    out: crate::outq::Outq,
    viewed: bool,
    window: Window,
}

pub(super) struct State {
    root: std::path::PathBuf,
    shelf: Shelf,
    hub: Arc<OnceLock<Weak<crate::Hub>>>,
    key: PublicKey,
    name: MachineName,
    letters: Option<Letters>,
    published: Arc<ArcSwapOption<Letters>>,
    generation: Arc<AtomicU64>,
    retired: Arc<std::sync::atomic::AtomicBool>,
    backlog: Arc<Backlog>,
    held: Held,
    loaded: bool,
    persisted_next: u64,
    dirty_metadata: bool,
    dirty_count: bool,
    to: Option<Sending>,
    /// The last pump stopped at `LOOKED_AT_ONCE` with room still to send:
    /// it goes on between this mailbox's commands.
    unfinished: bool,
    /// Letters still to delete through this number, a deletion cut short at
    /// `LOOKED_AT_ONCE`: it goes on between this mailbox's commands.
    erase_to: Option<u64>,
}

/// Letter numbers one pump looks at most, letters or not: a long run of
/// numbers whose letters went before they were sent is passed a part at a
/// time, with what the mailbox is asked in between.
const LOOKED_AT_ONCE: u64 = 4096;

impl State {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        root: std::path::PathBuf,
        shelf: Shelf,
        hub: Arc<OnceLock<Weak<crate::Hub>>>,
        key: PublicKey,
        name: MachineName,
        letters: Option<Letters>,
        published: Arc<ArcSwapOption<Letters>>,
        generation: Arc<AtomicU64>,
        retired: Arc<std::sync::atomic::AtomicBool>,
        backlog: Arc<Backlog>,
    ) -> Self {
        let persisted_next = letters.as_ref().map_or(0, |letters| letters.next);
        Self {
            root,
            shelf,
            hub,
            key,
            name,
            letters,
            published,
            generation,
            retired,
            backlog,
            held: Held::default(),
            loaded: false,
            persisted_next,
            dirty_metadata: false,
            dirty_count: false,
            to: None,
            unfinished: false,
            erase_to: None,
        }
    }

    fn current(&self) -> Result<MachineName> {
        if self.retired.load(Ordering::Acquire) {
            return Err("member was removed".into());
        }
        let hub = self.hub.get().and_then(Weak::upgrade).ok_or("hub stopped")?;
        hub.store()
            .indexed()
            .by_key(&self.key)
            .map(|member| member.name.clone())
            .ok_or_else(|| "member was removed".into())
    }

    fn record(&self) -> &Letters {
        self.letters.as_ref().expect("initialized mail state")
    }
    fn record_mut(&mut self) -> &mut Letters {
        self.letters.as_mut().expect("initialized mail state")
    }
    fn directory(&self) -> std::path::PathBuf {
        self.root.join("outboxes").join(format!("{:016x}", self.record().out_epoch))
    }

    async fn initialize(&mut self) -> Result<()> {
        if self.letters.is_none() {
            self.letters = Some(Letters::fresh(None).map_err(|e| e.to_string())?);
            self.persist().await?;
        }
        Ok(())
    }

    async fn persist(&mut self) -> Result<()> {
        self.dirty_metadata = true;
        self.current()?;
        if self.dirty_count {
            self.count().await?;
        }
        self.name = super::storage::persist(
            &self.shelf,
            self.key,
            self.name.clone(),
            self.record().clone(),
            self.retired.clone(),
        )
        .await?;
        self.persisted_next = self.record().next;
        self.dirty_metadata = false;
        self.published.store(Some(Arc::new(self.record().clone())));
        Ok(())
    }

    async fn count(&mut self) -> Result<()> {
        self.dirty_count = true;
        let path = self.root.join("counts").join(format!("{:016x}", self.record().out_epoch));
        let bytes = crate::store::json(&Taken::of(self.record())).map_err(|e| e.to_string())?;
        blocking(move || {
            fastctx_platform::ensure_private_dir(path.parent().expect("counts directory"))?;
            fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
        })
        .await?;
        self.dirty_count = false;
        self.published.store(Some(Arc::new(self.record().clone())));
        Ok(())
    }

    async fn load(&mut self) -> Result<()> {
        if !self.loaded {
            let directory = self.directory();
            let (lowest, count) = blocking(move || crate::store::letter_span(&directory)).await?;
            self.held = Held { first: lowest.unwrap_or(self.record().next), count };
            self.loaded = true;
            self.backlog.held.store(count, Ordering::Relaxed);
            self.backlog.counted.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Deletes the letters `numbers`; a number with no letter is passed
    /// over. When a deletion fails, the mailbox is counted again before it
    /// is used next.
    async fn erase(&mut self, numbers: Vec<u64>) -> Result<()> {
        let directory = self.directory();
        self.settle(blocking(move || remove_letters(&directory, numbers)).await)
    }

    /// Deletes every letter numbered through `through`, at most
    /// `LOOKED_AT_ONCE` numbers at once: what is left goes on between this
    /// mailbox's commands.
    async fn erase_through(&mut self, through: u64) -> Result<()> {
        self.erase_to = None;
        let last = through.min(self.record().next.saturating_sub(1));
        let first = self.held.first;
        if self.held.count == 0 || first > last {
            return Ok(());
        }
        let cut = last.min(first.saturating_add(LOOKED_AT_ONCE - 1));
        let directory = self.directory();
        self.settle(blocking(move || remove_letters(&directory, first..=cut)).await)?;
        self.held.first = self.held.first.max(cut + 1);
        if cut < last && self.held.count > 0 {
            self.erase_to = Some(last);
        }
        Ok(())
    }

    fn settle(&mut self, removed: Result<usize>) -> Result<()> {
        match removed {
            Ok(removed) => {
                self.held.count = self.held.count.saturating_sub(removed);
                if self.held.count == 0 {
                    self.held.first = self.record().next;
                }
                self.backlog.held.store(self.held.count, Ordering::Relaxed);
                Ok(())
            }
            Err(error) => {
                self.loaded = false;
                Err(error)
            }
        }
    }

    async fn pump(&mut self) -> Result<()> {
        self.unfinished = false;
        let Some(to) = &self.to else { return Ok(()) };
        if !to.viewed || !to.window.room() {
            return Ok(());
        }
        self.load().await?;
        let to = self.to.as_ref().expect("session");
        let (from, next) = ((to.window.sent() + 1).max(self.held.first), self.persisted_next);
        if from >= next {
            return Ok(());
        }
        let end = next.min(from.saturating_add(LOOKED_AT_ONCE));
        // The letters that fit, as they go, each sized and read off the
        // lines' way; a number with no letter goes as not there.
        let mut fitting = to.window.clone();
        let directory = self.directory();
        let out = to.out.clone();
        let loaded = blocking(move || {
            let mut loaded = Vec::new();
            for seq in from..end {
                if !fitting.room() {
                    break;
                }
                let Some(size) = crate::store::letter_size(&directory, seq)? else {
                    fitting.went(seq, None);
                    loaded.push((seq, None, false));
                    continue;
                };
                let letter = crate::store::read_letter(&directory, seq)?;
                let present = letter.is_some();
                if let Some(message) = letter {
                    out.send(crate::Out::Frame(fastctx_net::wire::Down::Deliver { seq, message }))
                        .map_err(|()| std::io::Error::other("member writer stopped"))?;
                }
                fitting.went(seq, present.then_some(size));
                loaded.push((seq, Some(size), present));
            }
            Ok(loaded)
        })
        .await?;
        let to = self.to.as_mut().expect("session");
        for (seq, size, present) in loaded {
            // A number with no letter at the front of what is held is passed
            // for good: no deletion looks at it again.
            if size.is_none() && seq == self.held.first {
                self.held.first = seq + 1;
            }
            to.window.went(seq, size.filter(|_| present));
            // Sized, then gone before it was read: it was counted.
            if size.is_some() && !present {
                self.held.count = self.held.count.saturating_sub(1);
            }
        }
        self.backlog.held.store(self.held.count, Ordering::Relaxed);
        self.backlog.on_way.store(to.window.on_way(), Ordering::Relaxed);
        self.unfinished = end < next && to.window.room();
        Ok(())
    }

    async fn retire(&mut self) -> Result<()> {
        self.to = None;
        self.generation.store(0, Ordering::Release);
        let Some(record) = &self.letters else { return Ok(()) };
        let epoch = record.out_epoch;
        let root = self.root.clone();
        let name = self.name.to_string();
        self.shelf
            .0
            .write(0, move |connection| {
                use fastctx_storage::rusqlite::OptionalExtension as _;
                let bytes: Option<Vec<u8>> = connection
                    .query_row("SELECT body FROM hub_rows WHERE kind='letters' AND key=?1", [&name], |row| row.get(0))
                    .optional()?;
                if bytes
                    .as_deref()
                    .map(crate::store::shelf::decode::<Letters>)
                    .transpose()?
                    .is_some_and(|letters| letters.out_epoch == epoch)
                {
                    connection.execute("DELETE FROM hub_rows WHERE kind='letters' AND key=?1", [&name])?;
                }
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())?;
        blocking(move || {
            let boxes = root.join("outboxes").join(format!("{epoch:016x}"));
            let trash = root.join(crate::store::TRASH);
            fastctx_platform::ensure_private_dir(&trash)?;
            if fastctx_platform::disk::is_there(&boxes) {
                fastctx_platform::disk::rename(&boxes, trash.join(format!("{epoch:016x}")))?;
            }
            match fastctx_platform::disk::remove_file(root.join("counts").join(format!("{epoch:016x}"))) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            Ok(())
        })
        .await?;
        self.published.store(None);
        self.held = Held::default();
        self.backlog.held.store(0, Ordering::Relaxed);
        self.backlog.on_way.store(0, Ordering::Relaxed);
        self.backlog.counted.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn perform(&mut self, command: Command) -> Result<()> {
        // Counted as the hub starts: a member that never had mail has no
        // mailbox to count, and is given none for it.
        if let Command::Tally { reply } = command {
            let result = if self.letters.is_some() {
                self.load().await
            } else {
                self.backlog.counted.store(true, Ordering::Relaxed);
                Ok(())
            };
            let _ = reply.send(result.clone());
            return result;
        }
        if let Command::Retire { reply } = command {
            let result = self.retire().await;
            let _ = reply.send(result.clone());
            return result;
        }
        if let Command::Flush { reply } = command {
            let result = if self.letters.is_some() && (self.dirty_metadata || self.dirty_count) {
                self.persist().await
            } else {
                Ok(())
            };
            let _ = reply.send(result);
            return Ok(());
        }
        self.initialize().await?;
        match command {
            Command::Open { generation, letters, capabilities: caps, out, register, reply } => {
                self.generation.store(0, Ordering::Release);
                if let Some(previous) = self.to.take() {
                    let _ = previous.out.send(crate::Out::End(fastctx_net::wire::Refusal::Replaced));
                }
                let renumbered = self.record().epoch != letters.epoch;
                let record = self.record_mut();
                if renumbered {
                    record.epoch = letters.epoch;
                    record.taken = 0;
                    record.refused.clear();
                }
                record.any_names = caps.any_names;
                record.renames = caps.renames;
                record.forgets = caps.forgets;
                record.roles = caps.roles;
                record.versions = caps.versions;
                record.due = caps.due;
                record.project_deletes = caps.project_deletes;
                record.conversation_deletions = caps.conversation_deletions;
                record.profiles = caps.profiles;
                record.machine_updates = caps.machine_updates;
                record.history_switch = caps.history_switch;
                record.connected_at = Some(jiff::Timestamp::now());
                let taken = fastctx_net::window::standing(letters.taken, record.out_epoch);
                let result = async {
                    if renumbered {
                        self.count().await?;
                    }
                    self.persist().await?;
                    self.load().await?;
                    self.erase_through(taken).await?;
                    self.generation.store(generation, Ordering::Release);
                    self.to = Some(Sending { generation, out, viewed: false, window: Window::from(taken) });
                    let record = self.record().clone();
                    fastctx_platform::task::spawn_blocking(move || register(&record))
                        .await
                        .map_err(|e| e.to_string())??;
                    Ok(self.record().clone())
                }
                .await;
                if result.is_err() {
                    self.to = None;
                    self.generation.store(0, Ordering::Release);
                    self.record_mut().connected_at = None;
                    self.record_mut().last_seen = Some(jiff::Timestamp::now());
                    let _ = self.persist().await;
                }
                let _ = reply.send(result);
            }
            Command::Close { generation } if self.generation.load(Ordering::Acquire) == generation => {
                self.to = None;
                self.generation.store(0, Ordering::Release);
                self.record_mut().connected_at = None;
                self.record_mut().last_seen = Some(jiff::Timestamp::now());
                self.persist().await?;
            }
            Command::Viewed { generation } => {
                if let Some(to) = self.to.as_mut().filter(|to| to.generation == generation) {
                    to.viewed = true;
                }
            }
            Command::Taken { generation, through } => {
                if let Some(to) = self.to.as_mut().filter(|to| to.generation == generation) {
                    let through = to.window.held(through);
                    self.backlog.on_way.store(to.window.on_way(), Ordering::Relaxed);
                    self.erase_through(through).await?;
                }
            }
            Command::Heard { generation, after, through } if self.generation.load(Ordering::Acquire) == generation => {
                let before = self.record().refused.len();
                self.record_mut().refused.retain(|seq, _| !(*seq <= through && after.is_none_or(|after| *seq > after)));
                self.dirty_count |= before != self.record().refused.len();
                self.published.store(Some(Arc::new(self.record().clone())));
            }
            Command::Count { generation, seq, refused, reply } => {
                let result = if self.generation.load(Ordering::Acquire) != generation {
                    Err("session replaced".into())
                } else {
                    self.record_mut().taken = self.record().taken.max(seq);
                    if let Some(why) = refused {
                        self.record_mut().refused.insert(seq, why);
                    }
                    self.count().await
                };
                let _ = reply.send(result);
            }
            Command::Put { message, reply } => {
                let result = async {
                    self.current()?;
                    // Counted first, so what is held is told whole, not as
                    // the one letter added to a mailbox never read.
                    self.load().await?;
                    let seq = self.record().next;
                    let path = self.directory().join(format!("{seq:020}"));
                    let size = blocking(move || crate::store::write_letter(&path, &message)).await?;
                    let _ = size;
                    self.record_mut().next = seq.checked_add(1).ok_or("letter sequence exhausted")?;
                    if self.held.count == 0 {
                        self.held.first = seq;
                    }
                    self.held.count += 1;
                    self.persist().await?;
                    self.backlog.held.store(self.held.count, Ordering::Relaxed);
                    Ok(())
                }
                .await;
                let _ = reply.send(result);
            }
            Command::DropFor { resident, reply } => {
                let result = async {
                    self.load().await?;
                    let directory = self.directory();
                    let gone = blocking(move || {
                        let mut gone = Vec::new();
                        for seq in crate::store::letter_numbers(&directory)? {
                            let Some(letter) = crate::store::read_letter(&directory, seq)? else { continue };
                            let Ok(body) = letter.body::<fastctx_net::message::LetterBody>() else { continue };
                            use fastctx_types::remote::Letter;
                            if matches!(body.letter, Letter::JobFacts { to, .. } | Letter::Notice { to, .. } | Letter::Speak { to, .. } if to == resident) { gone.push(seq); }
                        }
                        Ok(gone)
                    }).await?;
                    let count = gone.len(); self.erase(gone).await?; Ok(count)
                }.await;
                let _ = reply.send(result);
            }
            Command::Expire { before, reply } => {
                let result = async {
                    if self.to.is_some() || self.record().last_seen.is_none_or(|seen| seen >= before) {
                        return Ok(0);
                    }
                    self.load().await?;
                    if self.held.count == 0 {
                        return Ok(0);
                    }
                    let directory = self.directory();
                    let senders = blocking(move || {
                        let mut senders = BTreeMap::<String, usize>::new();
                        for seq in crate::store::letter_numbers(&directory)? {
                            if let Some(letter) = crate::store::read_letter(&directory, seq)? {
                                *senders.entry(letter.from.to_string()).or_default() += 1;
                            }
                        }
                        Ok(senders)
                    })
                    .await?;
                    self.record_mut().settled = Some(jiff::Timestamp::now());
                    self.persist().await?;
                    let count = self.held.count;
                    self.erase_through(u64::MAX).await?;
                    if let Some(hub) = self.hub.get().and_then(Weak::upgrade) {
                        hub.log.line(format_args!(
                            "letters for {} settled as not delivered, by sender: {}",
                            self.name,
                            senders
                                .iter()
                                .map(|(from, count)| format!("{from} {count}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    Ok(count)
                }
                .await;
                let _ = reply.send(result);
            }
            Command::Names { reply } => {
                self.record_mut().any_names = true;
                let _ = reply.send(self.persist().await);
            }
            Command::Flush { .. } | Command::Retire { .. } | Command::Tally { .. } => {
                unreachable!("handled before initialization")
            }
            Command::Close { .. } | Command::Heard { .. } => {}
        }
        self.pump().await
    }
}

pub(super) async fn run(mut state: State, mut commands: mpsc::Receiver<super::Queued>) {
    while let Some(super::Queued { command, _active, _pending }) = commands.recv().await {
        let retiring = matches!(&command, Command::Retire { .. });
        let mut done = state.perform(command).await;
        // A pump or a deletion cut short at its look goes on while nothing
        // else is asked.
        while done.is_ok() && !retiring && (state.unfinished || state.erase_to.is_some()) && commands.is_empty() {
            done = match state.erase_to.take() {
                Some(through) => state.erase_through(through).await,
                None => state.pump().await,
            };
        }
        if let Err(error) = done {
            if let Some(hub) = state.hub.get().and_then(Weak::upgrade) {
                hub.log.line(format_args!("mail for {}: {error}", state.name));
            }
            if let Some(to) = state.to.take() {
                let _ = to.out.send(crate::Out::End(fastctx_net::wire::Refusal::Broken { reason: error }));
            }
        }
        if retiring {
            break;
        }
    }
}

/// Deletes the letters numbered `numbers` in `directory`: how many were
/// there.
fn remove_letters(directory: &std::path::Path, numbers: impl IntoIterator<Item = u64>) -> std::io::Result<usize> {
    let mut removed = 0;
    for seq in numbers {
        match fastctx_platform::disk::remove_file(directory.join(format!("{seq:020}"))) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(removed)
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> Result<T> {
    fastctx_platform::task::spawn_blocking(work).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}
