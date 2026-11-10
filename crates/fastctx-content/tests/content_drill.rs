//! Measures the content database on this machine: what an import and a page
//! cost, and that neither grows with what is already stored.
//!
//! content_drill --root ABSOLUTE_DIRECTORY_THAT_DOES_NOT_EXIST [--stored RECORDS] [--batch RECORDS] [--strict]
//!
//! `--stored` fills the database with that many records before measuring.
//! `--strict` fails the drill when a budget is missed; it holds for a
//! release build only.

use fastctx_content::{Anchor, Batch, Content, Record, State, StreamKey};
use std::path::Path;
use std::time::{Duration, Instant};

/// The shape of a small conversation's first import: its records, and the
/// bytes of each.
const IMPORTED: usize = 600;
const BODY: usize = 3_500;
/// One host record in at most this long, storage and index together.
const RECORD_BUDGET: Duration = Duration::from_micros(40);
/// A first page within this, however much is stored.
const PAGE_BUDGET: Duration = Duration::from_millis(20);

type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> Outcome<()> {
    let args: Vec<String> = std::env::args().collect();
    let root = match args.iter().position(|arg| arg == "--root").and_then(|at| args.get(at + 1)) {
        Some(root) => Path::new(root),
        None => return Err("usage: content_drill --root ABSOLUTE_DIRECTORY_THAT_DOES_NOT_EXIST".into()),
    };
    if !root.is_absolute() || root.exists() {
        return Err("the drill needs an absolute root that does not exist".into());
    }
    let stored: usize = match args.iter().position(|arg| arg == "--stored").and_then(|at| args.get(at + 1)) {
        Some(stored) => stored.parse()?,
        None => 0,
    };
    let batch: usize = match args.iter().position(|arg| arg == "--batch").and_then(|at| args.get(at + 1)) {
        Some(batch) => batch.parse()?,
        None => 64,
    };
    let strict = args.iter().any(|arg| arg == "--strict");
    std::fs::create_dir_all(root)?;
    let content = Content::open(root)?;

    if stored > 0 {
        let began = Instant::now();
        fill(&content, stored)?;
        println!("stored {stored} records in {:.1?}", began.elapsed());
    }

    let stream = StreamKey::new("drill/scope", "conversation/import/timeline");
    let import = import(&content, &stream, batch)?;
    let each = import / IMPORTED as u32;
    println!(
        "import: {IMPORTED} records of {BODY} bytes, {batch} a batch, in {import:.2?}, {each:.1?} a record, {:.0} MiB/s",
        (IMPORTED * BODY) as f64 / (1024.0 * 1024.0) / import.as_secs_f64()
    );

    let (median, high, worst) = first_pages(&content, &stream)?;
    println!("first page of 101: p50 {median:.1?}, p99 {high:.1?}, worst {worst:.1?}");
    let (pages, slowest) = walk(&content, &stream, IMPORTED)?;
    println!("walked back {pages} pages, slowest {slowest:.1?}");

    revisions(&content, &stream)?;
    let began = Instant::now();
    let audited = content.audit(&stream)?;
    println!(
        "audited {} frames, {} records, {} items in {:.1?}: the frames give the index",
        audited.frames,
        audited.records,
        audited.items,
        began.elapsed()
    );
    let began = Instant::now();
    content.settle()?;
    println!("settled to the disk in {:.1?}", began.elapsed());

    let storage = content.statistics();
    println!("writes over the writer's budget: {}", storage.writes_over_budget);
    println!("writer operations by time: {}", spread(&storage.write_micros));
    println!("commits by time: {}", spread(&storage.commit_micros));
    println!("database {} KiB, log {} KiB", storage.database_bytes / 1024, storage.wal_bytes / 1024);
    if strict {
        if each > RECORD_BUDGET {
            return Err(format!("a record took {each:?}; its budget is {RECORD_BUDGET:?}").into());
        }
        if high > PAGE_BUDGET {
            return Err(format!("the first page's p99 was {high:?}; its budget is {PAGE_BUDGET:?}").into());
        }
    }
    println!("content drill passed");
    Ok(())
}

/// Text that compresses about as a transcript does, the same for the same seed.
fn body(seed: u64, bytes: usize) -> Vec<u8> {
    const WORDS: [&str; 16] = [
        "the",
        "tool",
        "result",
        "was",
        "read",
        "from",
        "conversation",
        "and",
        "its",
        "output",
        "{\"type\":",
        "\"text\"",
        "error",
        "ran",
        "file",
        "line",
    ];
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut text = Vec::with_capacity(bytes + 16);
    while text.len() < bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        if state % 5 == 0 {
            text.extend_from_slice(format!("{:x}", state >> 40).as_bytes());
        } else {
            text.extend_from_slice(WORDS[(state >> 20) as usize % WORDS.len()].as_bytes());
        }
        text.push(b' ');
    }
    text.truncate(bytes);
    text
}

fn record(number: usize, bytes: usize) -> Record {
    Record {
        item: format!("item-{number:010}").into_bytes(),
        order: (number as u64).to_be_bytes().to_vec(),
        state: State::Is { body: body(number as u64, bytes), objects: Vec::new() },
    }
}

fn fill(content: &Content, stored: usize) -> Outcome<()> {
    const STREAM: usize = 5_000;
    const BATCH: usize = 4_096;
    let mut written = 0;
    while written < stored {
        let stream = StreamKey::new("drill/stored", format!("conversation/{}/timeline", written / STREAM));
        let until = (written + BATCH).min(stored).min((written / STREAM + 1) * STREAM);
        let records = (written..until).map(|number| record(number, 160)).collect();
        content.append(Batch { stream, generation: 0, records, checkpoint: None, durable: false })?;
        written = until;
    }
    Ok(())
}

/// A histogram's filled buckets, each as the time its values stay below.
fn spread(buckets: &[u64; 32]) -> String {
    buckets
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(bucket, count)| format!("{count} under {:?}", Duration::from_micros(1 << bucket)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn import(content: &Content, stream: &StreamKey, batch: usize) -> Outcome<Duration> {
    let batches: Vec<Vec<Record>> = (0..IMPORTED)
        .collect::<Vec<_>>()
        .chunks(batch)
        .map(|numbers| numbers.iter().map(|number| record(*number, BODY)).collect())
        .collect();
    let began = Instant::now();
    for records in batches {
        content.append(Batch { stream: stream.clone(), generation: 0, records, checkpoint: None, durable: false })?;
    }
    Ok(began.elapsed())
}

fn first_pages(content: &Content, stream: &StreamKey) -> Outcome<(Duration, Duration, Duration)> {
    let mut took = Vec::new();
    for _ in 0..2_000 {
        let began = Instant::now();
        let page = content.page(stream, &Anchor::Latest, 101, 1024 * 1024)?;
        took.push(began.elapsed());
        if page.items.len() != 101 || page.items[100].order != (IMPORTED as u64 - 1).to_be_bytes() {
            return Err("the first page is not the stream's last 101 items".into());
        }
    }
    took.sort();
    Ok((took[took.len() / 2], took[took.len() * 99 / 100], took[took.len() - 1]))
}

/// Reads the whole stream back a page at a time and checks it is whole and
/// in order.
fn walk(content: &Content, stream: &StreamKey, expected: usize) -> Outcome<(usize, Duration)> {
    let mut anchor = Anchor::Latest;
    let (mut pages, mut seen, mut slowest) = (0, 0, Duration::ZERO);
    let mut before: Option<Vec<u8>> = None;
    loop {
        let began = Instant::now();
        let page = content.page(stream, &anchor, 100, 256 * 1024)?;
        slowest = slowest.max(began.elapsed());
        pages += 1;
        for pair in page.items.windows(2) {
            if pair[0].order >= pair[1].order {
                return Err("a page is out of order".into());
            }
        }
        if let (Some(last), Some(before)) = (page.items.last(), &before)
            && last.order >= *before
        {
            return Err("a page overlaps the one after it".into());
        }
        seen += page.items.len();
        before = page.items.first().map(|item| item.order.clone());
        match page.older {
            Some(place) => anchor = Anchor::Before(place),
            None => break,
        }
    }
    if seen != expected {
        return Err(format!("walked {seen} items of {expected}").into());
    }
    Ok((pages, slowest))
}

/// An item set again stays where it is with its new state; one given another
/// order moves; one gone is gone.
fn revisions(content: &Content, stream: &StreamKey) -> Outcome<()> {
    let before = content.revision(stream)?.ok_or("the stream is not there")?;
    let mut again = record(7, 64);
    again.state = State::Is { body: b"revised".to_vec(), objects: Vec::new() };
    let mut moved = record(9, 64);
    moved.order = (IMPORTED as u64 + 5).to_be_bytes().to_vec();
    let mut gone = record(11, 0);
    gone.state = State::Gone;
    let appended = content.append(Batch {
        stream: stream.clone(),
        generation: 0,
        records: vec![again, moved.clone(), gone],
        checkpoint: None,
        durable: false,
    })?;
    if appended.revision != before + 1 || appended.first != IMPORTED as u64 {
        return Err("a batch did not move the stream by one revision".into());
    }
    let revised = content.item(stream, b"item-0000000007")?.ok_or("a revised item is missing")?;
    if revised.body != b"revised" || revised.sequence != IMPORTED as u64 {
        return Err("an item set again does not hold its new state".into());
    }
    if content.item(stream, b"item-0000000011")?.is_some() {
        return Err("an item that is gone is still read".into());
    }
    let latest = content.page(stream, &Anchor::Latest, 1, 1024)?;
    if latest.items.first().map(|item| &item.item) != Some(&moved.item) {
        return Err("a moved item is not where its new order puts it".into());
    }
    let (_, _) = walk(content, stream, IMPORTED - 1)?;
    let refused = content.append(Batch {
        stream: stream.clone(),
        generation: 3,
        records: vec![record(1, 8)],
        checkpoint: None,
        durable: false,
    });
    refused?;
    let stale = content.append(Batch {
        stream: stream.clone(),
        generation: 2,
        records: vec![record(2, 8)],
        checkpoint: None,
        durable: false,
    });
    if stale.is_ok() {
        return Err("a batch of a generation the stream has left was taken".into());
    }
    Ok(())
}
