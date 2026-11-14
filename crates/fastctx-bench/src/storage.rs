//! The rows of §18 measured in the storage core: a producer's library that
//! commits a conversation's timeline, a hub's that takes its frames, a
//! replica's that imports them — in one process, signed and checked with
//! real keys, nothing of the network. What is set up is not timed; only the
//! step a row names is.

use crate::fixture::{Seeded, event};
use crate::keys::{self, Checked, Device, Hub, Trusted};
use crate::stats::{self, Samples};
use fastctx_content::v2::{
    Authorizer, Commit, Holder, HubSigner, Imported, Kind as _, KindRecord, Library, OVERSIZED, RawFrame, Received, Signer, Taking,
    Timeline,
};
use fastctx_content::{Anchor, ObjectRef, Place, Record, State};
use fastctx_net::sign::SecretKey;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const ROWS: [&str; 7] =
    ["hub-frames", "replica-catch-up", "first-screen", "object-loop", "durable-word", "durable-attachment", "commit-to-readable"];

/// How much of each row's input: as §18 writes it, or a tenth to try the
/// harness.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scale {
    Full,
    Small,
}

impl Scale {
    fn of(self, full: usize) -> usize {
        match self {
            Self::Full => full,
            Self::Small => full / 10,
        }
    }
}

pub(crate) const SCOPE: &str = "c/w-bench/m1/claude-5a1ce000";

pub(crate) fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Runs `row` in `dir`; whether it met its target.
pub(crate) fn run(row: &str, dir: &Path, scale: Scale) -> Result<bool, String> {
    match row {
        "hub-frames" => hub_frames(dir, scale),
        "replica-catch-up" => replica_catch_up(dir, scale),
        "first-screen" => first_screen(dir, scale),
        "object-loop" => object_loop(dir, scale),
        "durable-word" => durable_word(dir),
        "durable-attachment" => durable_attachment(dir),
        "commit-to-readable" => commit_to_readable(dir),
        "replica-catch-up-parts" => replica_catch_up_parts(dir, scale),
        "first-screen-cold" => first_screen_cold(dir, scale),
        // Under 32 MiB the whole prefix is checked at each advance: begun at
        // 28, the record stays under it through its thousand lines.
        "advance-small" => crate::advance::advance(dir, scale, 28 << 20, "advance-small"),
        "advance-large" => crate::advance::advance(dir, scale, 1 << 30, "advance-large"),
        "first-import" => crate::imports::first_import(dir, scale),
        "large-value" => crate::imports::large_value(dir, scale),
        other => Err(format!("no row {other}")),
    }
}

/// A producer, a hub and a replica, each with the scope installed, and the
/// keys they sign with.
pub(crate) struct Three {
    pub(crate) producer: Library,
    hub: Library,
    replica: Library,
    pub(crate) device: Arc<dyn Signer>,
    hub_key: Arc<dyn HubSigner>,
    kinds: Arc<dyn Authorizer>,
}

pub(crate) fn three(dir: &Path) -> Result<Three, String> {
    let open = |name: &str, hub: bool| -> Result<Library, String> {
        let at = dir.join(name);
        std::fs::create_dir_all(&at).map_err(error)?;
        let (library, _) = if hub { Library::open_hub(&at) } else { Library::open(&at) }.map_err(error)?;
        Ok(library)
    };
    let (producer, hub, replica) = (open("producer", false)?, open("hub", true)?, open("replica", false)?);
    let device = Device(SecretKey::generate().map_err(error)?);
    let device_key = device.key();
    let kinds = keys::kinds();
    for library in [&producer, &hub, &replica] {
        library.know(kinds.clone());
        library.install(SCOPE, b"bench".to_vec(), 1).map_err(error)?;
    }
    replica.accept_epoch(SCOPE, keys::first_epoch(SCOPE, Timeline::STREAM, device_key), &Trusted).map_err(error)?;
    Ok(Three { producer, hub, replica, device: Arc::new(device), hub_key: Arc::new(Hub(SecretKey::generate().map_err(error)?)), kinds })
}

impl Three {
    /// Commits `count` events to the producer's timeline, `per` a commit,
    /// each `bytes(n)` long, from `seed`.
    fn produce(&self, count: usize, per: usize, bytes: impl Fn(&mut Seeded) -> usize, seed: u64) -> Result<(), String> {
        let mut seeded = Seeded::new(seed);
        let holding = self.producer.holding(SCOPE).map_err(error)?.ok_or("not held")?;
        let mut number = 0u64;
        while (number as usize) < count {
            let records: Vec<KindRecord> = (0..per.min(count - number as usize))
                .map(|_| {
                    let size = bytes(&mut seeded);
                    number += 1;
                    event(number, size, &mut seeded)
                })
                .collect();
            let commit = Commit { scope: SCOPE.into(), stream: Timeline::STREAM.into(), records, leases: Vec::new(), durable: false, op: None, holding };
            self.producer.raw_commit(commit, self.device.clone()).map_err(error)?;
        }
        Ok(())
    }

    fn take(&self, frame: RawFrame) -> Result<(), String> {
        let period = self.hub.holding(SCOPE).map_err(error)?.ok_or("not held")?;
        let anything = |_: &ObjectRef| true;
        let nothing = |_: &[u8; 32]| false;
        let taking = Taking { period, readable: &anything, proven: &nothing };
        match self.hub.take(SCOPE, frame, &Checked, &taking, self.kinds.clone(), self.hub_key.clone()).map_err(error)? {
            Received::Applied { .. } => Ok(()),
            other => Err(format!("the hub did not apply a frame: {other:?}")),
        }
    }

    /// The producer's frames from `from`, at most `most`.
    fn frames_of(library: &Library, from: u64, most: usize) -> Result<Vec<RawFrame>, String> {
        library.export(SCOPE, Timeline::STREAM, from, most).map_err(error)
    }

    fn next_of(library: &Library) -> Result<u64, String> {
        Ok(library.streams(SCOPE).map_err(error)?.into_iter().find(|(name, _)| name == Timeline::STREAM).map_or(0, |(_, next)| next))
    }

    /// Gives the hub everything the producer has, untimed.
    fn ship(&self) -> Result<(), String> {
        loop {
            let frames = Self::frames_of(&self.producer, Self::next_of(&self.hub)?, 256)?;
            if frames.is_empty() {
                return Ok(());
            }
            for frame in frames {
                self.take(frame)?;
            }
        }
    }

    /// Imports into the replica everything the hub has; the time the
    /// imports took, reading the hub's frames apart.
    fn import(&self) -> Result<Duration, String> {
        let holding = self.replica.holding(SCOPE).map_err(error)?.ok_or("not held")?;
        let mut took = Duration::ZERO;
        loop {
            let frames = Self::frames_of(&self.hub, Self::next_of(&self.replica)?, 64)?;
            if frames.is_empty() {
                return Ok(took);
            }
            for frame in frames {
                let started = Instant::now();
                let done = self.replica.import(SCOPE, frame, &Checked, Holder::Replica, holding).map_err(error)?;
                took += started.elapsed();
                if !matches!(done, Imported::Applied { .. }) {
                    return Err(format!("the replica did not apply a frame: {done:?}"));
                }
            }
        }
    }
}

/// The hub takes frames of 64 records of 512 bytes, one account, one stream:
/// at least 20,000 records a second.
fn hub_frames(dir: &Path, scale: Scale) -> Result<bool, String> {
    let three = three(dir)?;
    let count = scale.of(200_000);
    three.produce(count, 64, |_| 512, 18)?;
    let frames = Three::frames_of(&three.producer, 0, usize::MAX)?;
    let started = Instant::now();
    for frame in frames {
        three.take(frame)?;
    }
    Ok(stats::rate("hub-frames", "storage core: frames taken", count as f64, "records", started.elapsed(), 20_000.0, serde_json::json!({ "per_frame": 64, "bytes": 512 })))
}

/// A replica takes 300,000 records of 1 KiB in frames filled to 1 MiB: at
/// least 100,000 records a second.
fn replica_catch_up(dir: &Path, scale: Scale) -> Result<bool, String> {
    let three = three(dir)?;
    let count = scale.of(300_000);
    three.produce(count, 960, |_| 1024, 19)?;
    three.ship()?;
    let took = three.import()?;
    Ok(stats::rate("replica-catch-up", "storage core: frames imported", count as f64, "records", took, 100_000.0, serde_json::json!({ "bytes": 1024 })))
}

/// Where the replica's catch-up spends its time, apart from §18's rows: the
/// frames' checking and opening alone; their import into a replica that
/// knows no kind (each record kept aside in its frame, nothing judged or
/// indexed); and their import as `replica-catch-up` does it.
fn replica_catch_up_parts(dir: &Path, scale: Scale) -> Result<bool, String> {
    let three = three(dir)?;
    let count = scale.of(300_000);
    three.produce(count, 960, |_| 1024, 19)?;
    three.ship()?;
    // What a device catching up asks of its hub: the frames from where it
    // stands, a page of 64 at a time.
    let started = Instant::now();
    let (mut from, mut read) = (0u64, 0usize);
    loop {
        let page = Three::frames_of(&three.hub, from, 64)?;
        let Some(last) = page.last() else { break };
        let header = fastctx_content::v2::Header::decode(&last.header).map_err(error)?;
        from = header.first + header.count as u64;
        read += page.len();
    }
    stats::rate("replica-catch-up-parts", "frames read out of the hub, 64 a page", read as f64, "frames", started.elapsed(), 0.0, serde_json::json!({}));
    let frames = Three::frames_of(&three.hub, 0, usize::MAX)?;
    let started = Instant::now();
    let mut records = 0usize;
    for frame in &frames {
        let header = fastctx_content::v2::Header::decode(&frame.header).map_err(error)?;
        if !<Checked as fastctx_content::v2::Verifier>::verify(&Checked, &header, &frame.header, &frame.signature) {
            return Err("a frame did not verify".into());
        }
        records += fastctx_content::v2::open(&header, &frame.body)?.len();
    }
    stats::rate("replica-catch-up-parts", "frames checked and opened alone", records as f64, "records", started.elapsed(), 100_000.0, serde_json::json!({}));
    // A replica that knows no kind.
    let at = dir.join("unknowing");
    std::fs::create_dir_all(&at).map_err(error)?;
    let (unknowing, _) = Library::open(&at).map_err(error)?;
    // Told it knows no kind: with none told, a library takes every kind as
    // known (codex-c8f08e64 #6).
    unknowing.know(Arc::new(fastctx_content::v2::Kinds::new()));
    unknowing.install(SCOPE, b"bench".to_vec(), 1).map_err(error)?;
    let device_key = three.device.key();
    unknowing.accept_epoch(SCOPE, keys::first_epoch(SCOPE, Timeline::STREAM, device_key), &Trusted).map_err(error)?;
    let holding = unknowing.holding(SCOPE).map_err(error)?.ok_or("not held")?;
    let started = Instant::now();
    for frame in frames.iter().cloned() {
        unknowing.import(SCOPE, frame, &Checked, Holder::Replica, holding).map_err(error)?;
    }
    stats::rate("replica-catch-up-parts", "imported, no kind known", records as f64, "records", started.elapsed(), 100_000.0, serde_json::json!({}));
    let took = three.import()?;
    Ok(stats::rate("replica-catch-up-parts", "imported, every kind known", records as f64, "records", took, 100_000.0, serde_json::json!({})))
}

/// From a producer's commit of one 1 KiB record to a reader on another
/// device told it can read it, its local subscription woken (`view_moved`):
/// the hub's take and the replica's import between, all in this process,
/// nothing of the network. §18 asks at most 250 ms with the network; this
/// is the storage core's part of it, 20 warm-ups and 1,000 samples.
fn commit_to_readable(dir: &Path) -> Result<bool, String> {
    let three = three(dir)?;
    three.produce(1000, 100, |_| 1024, 41)?;
    three.ship()?;
    three.import()?;
    let producing = three.producer.holding(SCOPE).map_err(error)?.ok_or("not held")?;
    let holding = three.replica.holding(SCOPE).map_err(error)?.ok_or("not held")?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().map_err(error)?;
    let mut seeded = Seeded::new(42);
    let mut number = 1000u64;
    let mut samples = Samples::default();
    for round in 0..1020 {
        let at = runtime.block_on(three.replica.view_moved(SCOPE, Timeline::STREAM, 0, Duration::ZERO)).map_err(error)?.unwrap_or(0);
        // The reader waits first, on a thread of its own, as the interface's
        // reading does.
        let replica = three.replica.clone();
        let (waiting, waited) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || -> Result<Option<Instant>, String> {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().map_err(error)?;
            let _ = waiting.send(());
            let moved = runtime.block_on(replica.view_moved(SCOPE, Timeline::STREAM, at, Duration::from_secs(10))).map_err(error)?;
            Ok(moved.filter(|now| *now > at).map(|_| Instant::now()))
        });
        waited.recv().map_err(error)?;
        std::thread::sleep(Duration::from_millis(2));
        number += 1;
        let record = event(number, 1024, &mut seeded);
        let started = Instant::now();
        let commit = Commit {
            scope: SCOPE.into(),
            stream: Timeline::STREAM.into(),
            records: vec![record],
            leases: Vec::new(),
            durable: false,
            op: None,
            holding: producing,
        };
        three.producer.raw_commit(commit, three.device.clone()).map_err(error)?;
        three.ship()?;
        for frame in Three::frames_of(&three.hub, Three::next_of(&three.replica)?, 64)? {
            three.replica.import(SCOPE, frame, &Checked, Holder::Replica, holding).map_err(error)?;
        }
        let told = reader.join().map_err(|_| "the reader ended without an answer".to_string())??;
        if round < 20 {
            continue;
        }
        match told {
            Some(at) => samples.push(at.saturating_duration_since(started)),
            None => samples.failed += 1,
        }
    }
    Ok(samples.report(
        "commit-to-readable",
        "storage core: a 1 KiB record committed, taken, imported and told to another device's reader",
        Some(250.0),
        serde_json::json!({ "network": false, "warm_up": 20 }),
    ))
}

/// The first page of a conversation of 300,000 events of 1–4 KiB, warm, 101
/// to a page: p99 at most 20 ms. Pages further down are measured beside it.
fn first_screen(dir: &Path, scale: Scale) -> Result<bool, String> {
    let three = three(dir)?;
    let count = scale.of(300_000);
    three.produce(count, 400, |seeded| seeded.between(1024, 4096), 20)?;
    three.ship()?;
    three.import()?;
    let library = &three.replica;
    let page = |anchor: &Anchor| library.view_page(SCOPE, Timeline::STREAM, anchor, 101, 4 << 20);
    // Warm.
    for _ in 0..20 {
        page(&Anchor::Latest).map_err(error)?;
    }
    let mut latest = Samples::default();
    for _ in 0..1000 {
        let started = Instant::now();
        match page(&Anchor::Latest) {
            Ok(read) if read.entries.len() == 101 => latest.push(started.elapsed()),
            _ => latest.failed += 1,
        }
    }
    let mut seeded = Seeded::new(21);
    let mut down = Samples::default();
    for _ in 0..1000 {
        let number = seeded.between(200, count) as u64;
        let key = number.to_be_bytes().to_vec();
        let anchor = Anchor::Before(Place { order: key.clone(), item: key });
        let started = Instant::now();
        match page(&anchor) {
            Ok(read) if !read.entries.is_empty() => down.push(started.elapsed()),
            _ => down.failed += 1,
        }
    }
    let met = latest.report("first-screen", "storage core: the latest page, warm", Some(20.0), serde_json::json!({ "events": count, "per_page": 101 }));
    down.report("first-screen", "storage core: a page further down, warm", None, serde_json::json!({ "events": count, "per_page": 101 }));
    Ok(met)
}

/// `first-screen` with nothing of the library in memory (§18: once cold):
/// the same conversation made and closed, then 20 times the machine's page
/// cache dropped, the library opened and its first page read — the latest,
/// and one further down. Only on Linux, as root (`/proc/sys/vm/drop_caches`),
/// as the reference machine runs it; elsewhere it says it cannot.
fn first_screen_cold(dir: &Path, scale: Scale) -> Result<bool, String> {
    let count = scale.of(300_000);
    {
        let three = three(dir)?;
        three.produce(count, 400, |seeded| seeded.between(1024, 4096), 20)?;
        three.ship()?;
        three.import()?;
    }
    let mut seeded = Seeded::new(22);
    let (mut opened, mut latest, mut down) = (Samples::default(), Samples::default(), Samples::default());
    for _ in 0..20 {
        drop_caches()?;
        let started = Instant::now();
        let (library, _) = Library::open(&dir.join("replica")).map_err(error)?;
        library.know(keys::kinds());
        opened.push(started.elapsed());
        let started = Instant::now();
        match library.view_page(SCOPE, Timeline::STREAM, &Anchor::Latest, 101, 4 << 20) {
            Ok(read) if read.entries.len() == 101 => latest.push(started.elapsed()),
            _ => latest.failed += 1,
        }
        drop(library);
        drop_caches()?;
        let (library, _) = Library::open(&dir.join("replica")).map_err(error)?;
        library.know(keys::kinds());
        let number = seeded.between(200, count) as u64;
        let key = number.to_be_bytes().to_vec();
        let started = Instant::now();
        match library.view_page(SCOPE, Timeline::STREAM, &Anchor::Before(Place { order: key.clone(), item: key }), 101, 4 << 20) {
            Ok(read) if !read.entries.is_empty() => down.push(started.elapsed()),
            _ => down.failed += 1,
        }
    }
    let shape = serde_json::json!({ "events": count, "per_page": 101, "rounds": 20 });
    opened.report("first-screen-cold", "storage core: the library opened, cold", None, shape.clone());
    let met = latest.report("first-screen-cold", "storage core: the latest page, cold", Some(20.0), shape.clone());
    down.report("first-screen-cold", "storage core: a page further down, cold", None, shape);
    Ok(met)
}

/// Empties the machine's page cache, what is written first put on the disk.
fn drop_caches() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("a cold cache is measured on Linux only, as root (the reference machine)".into());
    }
    let done = std::process::Command::new("sh")
        .args(["-c", "sync && echo 3 > /proc/sys/vm/drop_caches"])
        .status()
        .map_err(error)?;
    if done.success() { Ok(()) } else { Err("dropping the page cache needs root".into()) }
}

/// Bytes from a fixed seed, as a reader gives them.
struct SeededReader {
    seeded: Seeded,
    left: u64,
}

impl Read for SeededReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let length = (buffer.len() as u64).min(self.left) as usize;
        let bytes = self.seeded.bytes(length);
        buffer[..length].copy_from_slice(&bytes);
        self.left -= length as u64;
        Ok(length)
    }
}

/// 1 GiB of random bytes put into the object store and read back whole: at
/// least 150 MiB/s each way (the storage core; over a loopback connection
/// is the whole path's own row).
fn object_loop(dir: &Path, scale: Scale) -> Result<bool, String> {
    let (library, _) = Library::open(&dir.join("objects")).map_err(error)?;
    let size: u64 = if scale == Scale::Full { 1 << 30 } else { 64 << 20 };
    let started = Instant::now();
    let lease = library
        .put(&mut SeededReader { seeded: Seeded::new(22), left: size }, "bench", Duration::from_secs(3600))
        .map_err(error)?;
    let put = started.elapsed();
    let started = Instant::now();
    let (mut at, mut sum) = (0u64, 0u64);
    while at < size {
        let piece = library.read(&lease.object.hash, at, fastctx_content::v2::READ_BYTES).map_err(error)?;
        if piece.is_empty() {
            return Err("the object came back short".into());
        }
        sum = sum.wrapping_add(u64::from(piece[0]));
        at += piece.len() as u64;
    }
    let read = started.elapsed();
    let mib = size as f64 / (1 << 20) as f64;
    let wrote = stats::rate("object-loop", "storage core: put", mib, "MiB", put, 150.0, serde_json::json!({ "bytes": size }));
    let came = stats::rate("object-loop", "storage core: read back", mib, "MiB", read, 150.0, serde_json::json!({ "bytes": size, "sum": sum }));
    Ok(wrote && came)
}

/// How long a small write and its `fsync` take on this disk, `samples`
/// times: the baseline the durable rows are held to.
fn fsync_baseline(dir: &Path, bytes: usize, samples: usize) -> Result<Samples, String> {
    use std::io::Write as _;
    let path = dir.join("fsync-baseline");
    let data = vec![b'x'; bytes];
    let mut took = Samples::default();
    for _ in 0..samples {
        let started = Instant::now();
        let mut file = std::fs::File::create(&path).map_err(error)?;
        file.write_all(&data).map_err(error)?;
        file.sync_all().map_err(error)?;
        took.push(started.elapsed());
    }
    Ok(took)
}

fn p99(samples: &Samples) -> f64 {
    samples.p99_ms()
}

/// A word committed as a true source's write, waiting for the disk: p99 at
/// most one fsync and 2 ms.
fn durable_word(dir: &Path) -> Result<bool, String> {
    let three = three(dir)?;
    let holding = three.producer.holding(SCOPE).map_err(error)?.ok_or("not held")?;
    let baseline = fsync_baseline(dir, 4096, 1000)?;
    let fsync = p99(&baseline);
    let mut seeded = Seeded::new(23);
    let mut took = Samples::default();
    for number in 0..1000u64 {
        let records = vec![event(number + 1, 200, &mut seeded)];
        let commit = Commit { scope: SCOPE.into(), stream: Timeline::STREAM.into(), records, leases: Vec::new(), durable: true, op: None, holding };
        let started = Instant::now();
        match three.producer.raw_commit(commit, three.device.clone()) {
            Ok(_) => took.push(started.elapsed()),
            Err(_) => took.failed += 1,
        }
    }
    baseline.report("durable-word", "a 4 KiB write and fsync on this disk", None, serde_json::json!({}));
    Ok(took.report("durable-word", "storage core: a word committed durably", Some(fsync + 2.0), serde_json::json!({ "fsync_p99_ms": stats::round(fsync) })))
}

/// A word with a 1 MiB attachment whose bytes were not yet durable: the
/// object put (written, synced, renamed, its directory synced, present)
/// and the record that refers to it committed durably — p99 at most three
/// fsyncs, the 1 MiB write and 2 ms.
fn durable_attachment(dir: &Path) -> Result<bool, String> {
    let three = three(dir)?;
    let holding = three.producer.holding(SCOPE).map_err(error)?.ok_or("not held")?;
    let fsync = p99(&fsync_baseline(dir, 4096, 300)?);
    let write = p99(&fsync_baseline(dir, 1 << 20, 100)?);
    let mut seeded = Seeded::new(24);
    let mut took = Samples::default();
    for number in 0..200u64 {
        let bytes = seeded.bytes(1 << 20);
        let started = Instant::now();
        let done = (|| -> Result<(), String> {
            let lease = three.producer.put(&mut bytes.as_slice(), "bench", Duration::from_secs(600)).map_err(error)?;
            let mut body = vec![OVERSIZED];
            body.extend_from_slice(&serde_json::to_vec(&serde_json::json!({ "bytes": 1 << 20, "slim": null })).map_err(error)?);
            let key = (1_000_000 + number).to_be_bytes().to_vec();
            let record = KindRecord {
                kind: Timeline::KIND,
                schema: Timeline::SCHEMA,
                record: Record { item: key.clone(), order: key, state: State::Is { body, objects: vec![lease.object] } },
            };
            let commit = Commit { scope: SCOPE.into(), stream: Timeline::STREAM.into(), records: vec![record], leases: vec![lease], durable: true, op: None, holding };
            three.producer.raw_commit(commit, three.device.clone()).map_err(error)?;
            Ok(())
        })();
        match done {
            Ok(()) => took.push(started.elapsed()),
            Err(_) => took.failed += 1,
        }
    }
    let target = 2.0 * fsync + write + 2.0;
    Ok(took.report(
        "durable-attachment",
        "storage core: a word with a 1 MiB attachment committed durably",
        Some(target),
        serde_json::json!({ "fsync_p99_ms": stats::round(fsync), "write_1mib_and_fsync_p99_ms": stats::round(write) }),
    ))
}
