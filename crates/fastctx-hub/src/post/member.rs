//! One member's ordered mail state. No shared guard survives a disk await.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use arc_swap::ArcSwapOption;
use fastctx_net::sign::PublicKey;
use fastctx_types::MachineName;
use tokio::sync::mpsc;

use super::{Command, Result};
use crate::store::{Letters, Taken, shelf::Shelf};

const WINDOW: usize = 256;
const WINDOW_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Backlog {
    pub held: AtomicUsize,
    pub on_way: AtomicUsize,
    pub writing: AtomicUsize,
    pub pending: AtomicUsize,
}

struct Sending {
    generation: u64,
    out: crate::outq::Outq,
    viewed: bool,
    sent: u64,
    taken: u64,
    on_way: VecDeque<(u64, u64)>,
    bytes: u64,
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
    files: BTreeMap<u64, u64>,
    loaded: bool,
    persisted_next: u64,
    dirty_metadata: bool,
    dirty_count: bool,
    to: Option<Sending>,
}

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
            files: BTreeMap::new(),
            loaded: false,
            persisted_next,
            dirty_metadata: false,
            dirty_count: false,
            to: None,
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
            self.files = blocking(move || Ok(crate::store::letter_numbers(&directory)?.into_iter().collect())).await?;
            self.loaded = true;
            self.backlog.held.store(self.files.len(), Ordering::Relaxed);
        }
        Ok(())
    }

    async fn erase(&mut self, numbers: Vec<u64>) -> Result<()> {
        let directory = self.directory();
        let mut files = std::mem::take(&mut self.files);
        self.loaded = false;
        self.files = blocking(move || {
            for seq in numbers {
                match std::fs::remove_file(directory.join(format!("{seq:020}"))) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
                files.remove(&seq);
            }
            Ok(files)
        })
        .await?;
        self.loaded = true;
        self.backlog.held.store(self.files.len(), Ordering::Relaxed);
        Ok(())
    }

    async fn erase_through(&mut self, through: u64) -> Result<()> {
        let files = std::mem::take(&mut self.files);
        let (files, numbers) = blocking(move || {
            let numbers = files.range(..=through).map(|(&seq, _)| seq).collect();
            Ok((files, numbers))
        })
        .await?;
        self.files = files;
        self.erase(numbers).await
    }

    async fn pump(&mut self) -> Result<()> {
        let Some(to) = &self.to else { return Ok(()) };
        if !to.viewed || to.on_way.len() >= WINDOW || to.bytes >= WINDOW_BYTES {
            return Ok(());
        }
        self.load().await?;
        let to = self.to.as_ref().expect("session");
        let mut size = to.bytes;
        let mut numbers = Vec::new();
        for (&seq, &bytes) in self.files.range(to.sent + 1..self.persisted_next.max(to.sent + 1)) {
            if numbers.len() + to.on_way.len() >= WINDOW || (!numbers.is_empty() && size + bytes > WINDOW_BYTES) {
                break;
            }
            numbers.push((seq, bytes));
            size += bytes;
        }
        if numbers.is_empty() {
            return Ok(());
        }
        let directory = self.directory();
        let out = to.out.clone();
        let loaded = blocking(move || {
            numbers
                .into_iter()
                .map(|(seq, size)| {
                    let letter = crate::store::read_letter(&directory, seq)?;
                    let present = letter.is_some();
                    if let Some(message) = letter {
                        out.send(crate::Out::Frame(fastctx_net::wire::Down::Deliver { seq, message }))
                            .map_err(|()| std::io::Error::other("member writer stopped"))?;
                    }
                    Ok((seq, size, present))
                })
                .collect::<std::io::Result<Vec<_>>>()
        })
        .await?;
        let to = self.to.as_mut().expect("session");
        for (seq, size, present) in loaded {
            if present {
                to.on_way.push_back((seq, size));
                to.bytes += size;
            } else {
                self.files.remove(&seq);
            }
            to.sent = seq;
        }
        self.backlog.on_way.store(to.on_way.len(), Ordering::Relaxed);
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
            if boxes.exists() {
                std::fs::rename(&boxes, trash.join(format!("{epoch:016x}")))?;
            }
            match std::fs::remove_file(root.join("counts").join(format!("{epoch:016x}"))) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            Ok(())
        })
        .await?;
        self.published.store(None);
        let files = std::mem::take(&mut self.files);
        blocking(move || {
            drop(files);
            Ok(())
        })
        .await?;
        self.backlog.held.store(0, Ordering::Relaxed);
        self.backlog.on_way.store(0, Ordering::Relaxed);
        Ok(())
    }

    async fn perform(&mut self, command: Command) -> Result<()> {
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
                record.connected_at = Some(jiff::Timestamp::now());
                let taken = letters.taken.filter(|(epoch, _)| *epoch == record.out_epoch).map_or(0, |(_, taken)| taken);
                let result = async {
                    if renumbered {
                        self.count().await?;
                    }
                    self.persist().await?;
                    self.load().await?;
                    self.erase_through(taken).await?;
                    self.generation.store(generation, Ordering::Release);
                    self.to = Some(Sending {
                        generation,
                        out,
                        viewed: false,
                        sent: taken,
                        taken,
                        on_way: VecDeque::new(),
                        bytes: 0,
                    });
                    let record = self.record().clone();
                    tokio::task::spawn_blocking(move || register(&record)).await.map_err(|e| e.to_string())??;
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
                    let through = through.min(to.sent);
                    to.taken = to.taken.max(through);
                    while let Some((seq, size)) = to.on_way.front().copied()
                        && seq <= through
                    {
                        to.on_way.pop_front();
                        to.bytes -= size;
                    }
                    self.backlog.on_way.store(to.on_way.len(), Ordering::Relaxed);
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
                    let seq = self.record().next;
                    let path = self.directory().join(format!("{seq:020}"));
                    let size = blocking(move || crate::store::write_letter(&path, &message)).await?;
                    self.record_mut().next = seq.checked_add(1).ok_or("letter sequence exhausted")?;
                    self.files.insert(seq, size);
                    self.persist().await?;
                    self.backlog.held.store(self.files.len(), Ordering::Relaxed);
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
                        for (seq, _) in crate::store::letter_numbers(&directory)? {
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
                    let gone = self.files.keys().copied().collect::<Vec<_>>();
                    if gone.is_empty() {
                        return Ok(0);
                    }
                    let directory = self.directory();
                    let senders = blocking(move || {
                        let mut senders = BTreeMap::<String, usize>::new();
                        for (seq, _) in crate::store::letter_numbers(&directory)? {
                            if let Some(letter) = crate::store::read_letter(&directory, seq)? {
                                *senders.entry(letter.from.to_string()).or_default() += 1;
                            }
                        }
                        Ok(senders)
                    })
                    .await?;
                    self.record_mut().settled = Some(jiff::Timestamp::now());
                    self.persist().await?;
                    let count = gone.len();
                    self.erase(gone).await?;
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
            Command::Flush { .. } | Command::Retire { .. } => unreachable!("handled before initialization"),
            Command::Close { .. } | Command::Heard { .. } => {}
        }
        self.pump().await
    }
}

pub(super) async fn run(mut state: State, mut commands: mpsc::Receiver<super::Queued>) {
    while let Some(super::Queued { command, _active, _pending }) = commands.recv().await {
        let retiring = matches!(&command, Command::Retire { .. });
        if let Err(error) = state.perform(command).await {
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

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> Result<T> {
    fastctx_platform::task::spawn_blocking(work).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}
