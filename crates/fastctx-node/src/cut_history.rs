//! The segments a conversation's history was last cut into for its viewers,
//! kept a while (`held_history`): one that comes next is sent them and the
//! events told since they were cut, and its history is not read again. A
//! cut goes once its events are no longer all kept (`watch::Watchers`), or
//! the history changed after it, or the cuts kept together grow past their
//! bound, the one least lately sent first.
//!
//! A cut read from the timeline keeps its segments on disk beside the node,
//! only its outline in memory, each segment read as a viewer is sent it
//! (design §28.52 52-1): a long history is never in memory whole for its
//! viewers.

use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use fastctx_net::history::{Outline, Planned};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::Mark;
use fastctx_types::session::Watched;
use fastctx_types::session::held::Segment;

/// The bytes the cuts of every conversation may hold together, in memory
/// or on disk.
const KEPT_BYTES: usize = 128 << 20;

/// The bytes the cuts hold now, every conversation's together.
static HELD: LazyLock<&'static fastctx_platform::readings::Gauge> =
    LazyLock::new(|| fastctx_platform::readings::gauge("history.cut_bytes"));

/// The cuts on disk, under one lock: the holding of the machine they are
/// of — one more each time its holder changed (`assets::session`) — and the
/// files in use. A cut of an earlier holding is read no more, nor kept, and
/// its file goes with its last holder; a file is made, and the ones not in
/// use taken away, only under the lock, so neither falls between the other
/// (codex-e7b1552a review-s19 P1-1, review-s21 P1-1).
#[derive(Default)]
struct Cuts {
    epoch: u64,
    live: std::collections::HashSet<PathBuf>,
}

static CUTS: LazyLock<Mutex<Cuts>> = LazyLock::new(Default::default);

fn cuts() -> std::sync::MutexGuard<'static, Cuts> {
    CUTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The holding a reading for a viewer begins in: what it cuts is of it,
/// however long the reading takes.
pub(crate) fn holding() -> u64 {
    cuts().epoch
}

/// The holder of the machine changed: cuts begun before are of the holding
/// before.
pub(crate) fn advance() {
    cuts().epoch += 1;
}

/// A history cut into segments: its outline, where its segments are, and
/// the holding it was cut in.
pub(crate) struct Kept {
    pub outline: Outline,
    segments: Segments,
    epoch: u64,
}

/// Where a cut's segments are: held, or on disk.
enum Segments {
    Held(Vec<Segment>),
    Disk(SegmentFile),
}

impl Kept {
    /// A cut held in memory, as one read from the history's sources is.
    pub fn held(cut: fastctx_net::history::Cut, epoch: u64) -> Self {
        let mut outline = Outline::of(&cut);
        outline.seal();
        Self { outline, segments: Segments::Held(cut.segments), epoch }
    }

    /// Whether it is of the machine's holding now.
    pub fn current(&self) -> bool {
        self.epoch == holding()
    }

    /// The segment at `at` of the cut. Call on a thread that may block.
    fn segment(&self, at: usize) -> std::io::Result<Segment> {
        match &self.segments {
            Segments::Held(segments) => {
                segments.get(at).cloned().ok_or_else(|| std::io::Error::other("no such segment"))
            }
            Segments::Disk(file) => file.read(at),
        }
    }

    /// The piece `planned` of what a viewer is sent, its segments read.
    /// Call on a thread that may block.
    pub fn made(&self, planned: Planned) -> std::io::Result<Watched> {
        if !self.current() {
            return Err(std::io::Error::other("the machine's holder changed since the history was cut"));
        }
        match planned {
            Planned::Made(watched) => Ok(watched),
            Planned::Segments { version, layout, segments, complete } => Ok(Watched::Segments {
                version,
                layout: (*layout).clone(),
                segments: segments.into_iter().map(|at| self.segment(at)).collect::<std::io::Result<_>>()?,
                complete,
            }),
        }
    }
}

/// A cut's segments on disk, each as JSON at its place in the file; the
/// file goes with it.
struct SegmentFile {
    path: PathBuf,
    places: Vec<(u64, u64)>,
}

impl SegmentFile {
    fn read(&self, at: usize) -> std::io::Result<Segment> {
        let (offset, length) = *self.places.get(at).ok_or_else(|| std::io::Error::other("no such segment"))?;
        let mut file = fastctx_platform::disk::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; usize::try_from(length).map_err(std::io::Error::other)?];
        file.read_exact(&mut bytes)?;
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    }
}

impl Drop for SegmentFile {
    /// Its file goes; not in use any more, one that could not be removed
    /// is left for `cleared` to find.
    fn drop(&mut self) {
        let mut cuts = cuts();
        let _ = fastctx_platform::disk::remove_file(&self.path);
        cuts.live.remove(&self.path);
    }
}

/// Where the cuts on disk are kept, in the node's own directory: what an
/// earlier run of the node left there goes at the first cut of this one.
fn cuts_dir(world: &Path) -> std::io::Result<PathBuf> {
    static CLEARED: std::sync::Once = std::sync::Once::new();
    let dir = world.join("timeline-cuts");
    CLEARED.call_once(|| {
        let _ = fastctx_platform::disk::remove_dir_all(&dir);
    });
    fastctx_platform::disk::create_dir_all(&dir)?;
    Ok(dir)
}

/// The holder of the machine changed (`assets::session`), the holding moved
/// on (`advance`) and the cuts kept let go: those no cut holds any longer
/// go.
/// Whether none is left — none in use, and none on disk — for none of what
/// the holder before read to be found after; while one is, the caller waits
/// and asks again (its viewers end at their next piece).
pub(crate) fn cleared(world: &Path) -> std::io::Result<bool> {
    let dir = world.join("timeline-cuts");
    // Held while the files are looked at: none is made meanwhile.
    let cuts = cuts();
    let live = &cuts.live;
    let entries = match fastctx_platform::disk::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(live.is_empty()),
        Err(error) => return Err(error),
    };
    let mut left = !live.is_empty();
    for entry in entries {
        let path = entry?.path();
        if live.contains(&path) {
            left = true;
            continue;
        }
        if let Err(error) = fastctx_platform::disk::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error);
        }
    }
    Ok(!left)
}

/// A cut written to disk as it is made.
pub(crate) struct Writing {
    epoch: u64,
    file: std::io::BufWriter<std::fs::File>,
    places: Vec<(u64, u64)>,
    offset: u64,
    outline: Outline,
    /// Taken away should the cut not be finished.
    guard: Option<SegmentFile>,
}

impl Writing {
    /// A cut begun in `world`'s directory of cuts, of the holding `epoch`
    /// its reading began in (`holding`): one of an earlier holding is not
    /// begun.
    pub fn create(world: &Path, epoch: u64) -> std::io::Result<Self> {
        let name = format!(
            "{:032x}.segments",
            u128::from_le_bytes(fastctx_net::sign::random::<16>().map_err(std::io::Error::other)?)
        );
        let dir = cuts_dir(world)?;
        let mut cuts = cuts();
        if cuts.epoch != epoch {
            return Err(std::io::Error::other("the machine's holder changed as the history was read"));
        }
        let path = dir.join(name);
        let file = fastctx_platform::disk::open_with(std::fs::OpenOptions::new().write(true).create_new(true), &path)?;
        cuts.live.insert(path.clone());
        drop(cuts);
        Ok(Self {
            epoch,
            file: std::io::BufWriter::new(file),
            places: Vec::new(),
            offset: 0,
            outline: Outline::default(),
            guard: Some(SegmentFile { path, places: Vec::new() }),
        })
    }

    /// The next segment of the cut, of `size` bytes as JSON.
    pub fn add(&mut self, segment: &Segment, size: usize) -> std::io::Result<()> {
        self.outline.add(segment, size);
        let bytes = serde_json::to_vec(segment).map_err(std::io::Error::other)?;
        self.file.write_all(&bytes)?;
        self.places.push((self.offset, bytes.len() as u64));
        self.offset += bytes.len() as u64;
        Ok(())
    }

    /// The cut, its segments on disk.
    pub fn finish(mut self) -> std::io::Result<Kept> {
        self.file.flush()?;
        let mut file = self.guard.take().expect("a cut finished once");
        file.places = std::mem::take(&mut self.places);
        let mut outline = std::mem::take(&mut self.outline);
        outline.seal();
        let kept = Kept { outline, segments: Segments::Disk(file), epoch: self.epoch };
        // Begun before the holder changed, it is of no use: it goes now.
        if !kept.current() {
            return Err(std::io::Error::other("the machine's holder changed as the history was cut"));
        }
        Ok(kept)
    }
}

/// One conversation's history as it was cut.
pub(crate) struct CutHistory {
    /// The mark of the last event told before the history was read: the
    /// events after it follow the cut.
    pub at: Mark,
    pub version: String,
    pub cut: Arc<Kept>,
    /// Where the timeline it was read from stood: corrected since, the cut
    /// is not the history any more (codex-e7b1552a review-s19 P2-2).
    pub revision: Option<u64>,
    /// What was under way as the events after `at` began.
    pub snapshots: Vec<Arc<crate::watch::Marked>>,
    bytes: usize,
}

impl CutHistory {
    pub fn new(
        at: Mark,
        version: String,
        cut: Arc<Kept>,
        revision: Option<u64>,
        snapshots: Vec<Arc<crate::watch::Marked>>,
    ) -> Self {
        let bytes = cut.outline.sizes.iter().sum::<usize>()
            + cut.outline.bytes()
            + snapshots.iter().map(|marked| fastctx_net::watch_queue::event_size(&marked.event).0).sum::<usize>();
        Self { at, version, cut, revision, snapshots, bytes }
    }
}

/// What a viewer is sent of `cut` as `held` holds it, named `version`: each
/// piece read as it is sent (`crate::talk::OpeningPiece`).
pub(crate) fn pieces(
    cut: &Arc<Kept>,
    held: &fastctx_types::session::held::Held,
    version: String,
) -> Vec<crate::talk::OpeningPiece> {
    fastctx_net::history::plan(&cut.outline, held, version)
        .into_iter()
        .map(|planned| match planned {
            Planned::Made(watched) => crate::talk::OpeningPiece::Made(watched),
            planned => crate::talk::OpeningPiece::Kept { cut: cut.clone(), planned },
        })
        .collect()
}

/// A piece read off `cut`, on a thread that may block.
pub(crate) async fn made(cut: Arc<Kept>, planned: Planned) -> Result<Watched, Failure> {
    fastctx_platform::task::spawn_blocking(move || cut.made(planned))
        .await
        .map_err(|error| Failure::new(format!("reading the history's segments: {error}")))?
        .map_err(|error| Failure::new(format!("reading the history's segments: {error}")))
}

#[derive(Default)]
pub(crate) struct CutHistories(Mutex<Shelf>, crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<()>>);

#[derive(Default)]
struct Shelf {
    cuts: HashMap<ResidentId, (Arc<CutHistory>, u64)>,
    bytes: usize,
    sent: u64,
}

impl CutHistories {
    /// Held while `id`'s history is read and cut for a viewer: viewers that
    /// open it together read it once, the rest take the cut it leaves, and
    /// no more than one whole history of it is in memory at a time.
    pub fn reading(&self, id: &ResidentId) -> crate::leases::Lease<ResidentId, fastctx_platform::doing::Mutex<()>> {
        self.1.get_or_insert_with(id, || {
            fastctx_platform::doing::Mutex::new(fastctx_platform::doing::Rank::OPENING, "opening", id.to_string(), ())
        })
    }

    /// The cut kept of `id`'s history, now sent once more.
    pub fn get(&self, id: &ResidentId) -> Option<Arc<CutHistory>> {
        let mut kept = self.0.lock().expect("never poisoned");
        kept.sent += 1;
        let sent = kept.sent;
        let (cut, used) = kept.cuts.get_mut(id)?;
        if !cut.cut.current() {
            return None;
        }
        *used = sent;
        Some(cut.clone())
    }

    /// Keeps `cut` as `id`'s, in place of any before it, letting the least
    /// lately sent go past the bound. One larger than the bound is not kept.
    pub fn keep(&self, id: &ResidentId, cut: CutHistory) {
        // What is let go is dropped after the lock, which every viewer of
        // every conversation takes: a cut may be the history of a long one.
        let mut gone = Vec::new();
        let mut kept = self.0.lock().expect("never poisoned");
        // Looked at under the lock that clearing takes after the holding
        // moved on (`clear`): one of an earlier holding is not kept.
        if !cut.cut.current() {
            drop(kept);
            return;
        }
        if let Some((old, _)) = kept.cuts.remove(id) {
            kept.bytes -= old.bytes;
            gone.push(old);
        }
        if cut.bytes > KEPT_BYTES {
            HELD.set(kept.bytes as i64);
            drop(kept);
            drop(gone);
            return;
        }
        while kept.bytes + cut.bytes > KEPT_BYTES {
            let Some(oldest) = kept.cuts.iter().min_by_key(|(_, (_, used))| *used).map(|(id, _)| id.clone()) else {
                break;
            };
            if let Some((old, _)) = kept.cuts.remove(&oldest) {
                kept.bytes -= old.bytes;
                gone.push(old);
            }
        }
        kept.sent += 1;
        let sent = kept.sent;
        kept.bytes += cut.bytes;
        kept.cuts.insert(id.clone(), (Arc::new(cut), sent));
        HELD.set(kept.bytes as i64);
        drop(kept);
        drop(gone);
    }

    /// Lets every cut go.
    pub fn clear(&self) {
        let gone: Vec<_> = {
            let mut kept = self.0.lock().expect("never poisoned");
            kept.bytes = 0;
            HELD.set(0);
            kept.cuts.drain().collect()
        };
        drop(gone);
    }

    /// Lets `id`'s cut go, when it is still `cut`.
    pub fn forget(&self, id: &ResidentId, cut: &Arc<CutHistory>) {
        let mut kept = self.0.lock().expect("never poisoned");
        if kept.cuts.get(id).is_some_and(|(current, _)| Arc::ptr_eq(current, cut))
            && let Some((gone, _)) = kept.cuts.remove(id)
        {
            kept.bytes -= gone.bytes;
        }
        HELD.set(kept.bytes as i64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::session::held::Held;
    use fastctx_types::session::{Happened, SessionEvent};

    fn words(at: i64, text: &str, turn: Option<&str>) -> SessionEvent {
        SessionEvent {
            at: jiff::Timestamp::from_second(1_800_000_000 + at).unwrap(),
            happened: Happened::Heard {
                delivery: None,
                turn: turn.map(str::to_string),
                from: None,
                text: text.into(),
                message: None,
                notice: None,
            },
        }
    }

    /// A cut kept on disk as it is made is sent as the cut held: the same
    /// pieces, each read as it is sent; its file goes with it.
    #[test]
    fn a_cut_kept_on_disk_is_sent_as_the_cut_held() {
        let world = std::env::temp_dir().join(format!("fastctx-cuts-{}", std::process::id()));
        let _ = fastctx_platform::disk::remove_dir_all(&world);
        fastctx_platform::disk::create_dir_all(&world).unwrap();
        let events: Vec<SessionEvent> = (0..600)
            .map(|at| words(at, &"x".repeat(((at * 7919) % 50_000) as usize), Some(&format!("t{}", at / 40))))
            .collect();
        let held = Kept::held(fastctx_net::history::cut(events.clone()), holding());
        let mut writing = Writing::create(&world, holding()).unwrap();
        let mut segmenter = fastctx_net::history::Segmenter::default();
        for event in events {
            for (segment, size) in segmenter.push(event).into_iter().flatten() {
                writing.add(&segment, size).unwrap();
            }
        }
        if let Some((segment, size)) = segmenter.finish() {
            writing.add(&segment, size).unwrap();
        }
        let on_disk = Arc::new(writing.finish().unwrap());
        let held = Arc::new(held);
        assert_eq!(on_disk.outline.ids, held.outline.ids);
        assert!(held.outline.ids.len() > 3);
        let some: Vec<_> = held.outline.ids.iter().step_by(2).cloned().collect();
        for viewer in [
            Held { latest_first: true, ..Held::default() },
            Held::default(),
            Held { segments: some, ..Held::default() },
            Held { segments: held.outline.ids.clone(), ..Held::default() },
        ] {
            let sent = |cut: &Arc<Kept>| -> Vec<String> {
                fastctx_net::history::plan(&cut.outline, &viewer, "v".into())
                    .into_iter()
                    .map(|planned| serde_json::to_string(&cut.made(planned).unwrap()).unwrap())
                    .collect()
            };
            assert_eq!(sent(&on_disk), sent(&held));
        }
        let dir = world.join("timeline-cuts");
        assert_eq!(fastctx_platform::disk::read_dir(&dir).unwrap().count(), 1);
        drop(on_disk);
        assert_eq!(fastctx_platform::disk::read_dir(&dir).unwrap().count(), 0, "its file goes with it");
        let _ = fastctx_platform::disk::remove_dir_all(&world);
    }
}
