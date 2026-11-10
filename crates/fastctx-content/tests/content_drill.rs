//! Measures the content database on this machine: what an import and a page
//! cost, and that neither grows with what is already stored.
//!
//! content_drill --root ABSOLUTE_DIRECTORY_THAT_DOES_NOT_EXIST [--stored RECORDS] [--batch RECORDS] [--strict]
//!
//! `--stored` fills the database with that many records before measuring.
//! `--strict` fails the drill when a budget is missed; it holds for a
//! release build only.

use fastctx_content::{Anchor, Batch, Content, ObjectRef, Record, State, StreamKey};
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
    named(&content)?;
    let began = Instant::now();
    let audited = content.audit(&stream)?;
    println!(
        "audited {} frames, {} records, {} items in {:.1?}: the frames give the index",
        audited.frames,
        audited.records,
        audited.items,
        began.elapsed()
    );
    objects(&content, root)?;
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
        if state.is_multiple_of(5) {
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

/// The items whose identities begin with a prefix are found by the prefix,
/// those and no others, whatever bytes it ends with.
fn named(content: &Content) -> Outcome<()> {
    let stream = StreamKey::new("drill/named", "items");
    let identities: [&[u8]; 7] =
        [&[1, 1, 9], &[1, 1, 0xFF], &[1, 2], &[1, 0xFF, 0xFF, 7], &[1, 0xFF, 0xFF], &[2], &[0xFF, 0xFF, 3]];
    let records = identities
        .iter()
        .enumerate()
        .map(|(at, identity)| Record {
            item: identity.to_vec(),
            order: (at as u64).to_be_bytes().to_vec(),
            state: State::Is { body: vec![b'x'], objects: Vec::new() },
        })
        .collect();
    content.append(Batch { stream: stream.clone(), generation: 0, records, checkpoint: None, durable: false })?;
    let found = |prefix: &[u8], most: u32| -> Outcome<Vec<Vec<u8>>> {
        Ok(content.named(&stream, prefix, most)?.into_iter().map(|place| place.item).collect())
    };
    // A prefix, how many are asked for at most, and the identities it finds.
    type Found<'a> = (&'a [u8], u32, Vec<&'a [u8]>);
    let expected: [Found; 7] = [
        (&[1, 1], 9, vec![&[1, 1, 9], &[1, 1, 0xFF]]),
        (&[1, 1], 1, vec![&[1, 1, 9]]),
        (&[1, 0xFF, 0xFF], 9, vec![&[1, 0xFF, 0xFF], &[1, 0xFF, 0xFF, 7]]),
        (&[1], 9, vec![&[1, 1, 9], &[1, 1, 0xFF], &[1, 2], &[1, 0xFF, 0xFF], &[1, 0xFF, 0xFF, 7]]),
        (&[0xFF, 0xFF], 9, vec![&[0xFF, 0xFF, 3]]),
        (&[3], 9, vec![]),
        (&[2, 0], 9, vec![]),
    ];
    for (prefix, most, items) in expected {
        let found = found(prefix, most)?;
        if found != items.iter().map(|item| item.to_vec()).collect::<Vec<_>>() {
            return Err(format!("the items named by {prefix:?} came as {found:?}, not {items:?}").into());
        }
    }
    if !content.named(&StreamKey::new("drill/named", "none"), &[1], 9)?.is_empty()
        || content.named(&stream, &[], 9).is_ok()
    {
        return Err("a stream that is not there named items, or an empty prefix was taken".into());
    }
    println!("named: prefixes of identities find their items and no others");
    Ok(())
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

/// Objects are kept once by their content, read a range at a time, and
/// readable by a scope for as long as its records refer to them.
fn objects(content: &Content, root: &Path) -> Outcome<()> {
    let store = content.objects();
    let small = body(1, 9_000);
    let large = body(2, 3 * 1024 * 1024 + 17);
    let began = Instant::now();
    let kept_small = store.put(&small)?;
    let kept_large = store.put(&large)?;
    let took = began.elapsed();
    if store.put(&large)? != kept_large || store.size(&kept_large.hash)? != Some(large.len() as u64) {
        return Err("the same bytes were not the same object".into());
    }
    if store.read(&kept_small.hash, 100, 50)?.as_deref() != Some(&small[100..150])
        || store.read(&kept_large.hash, 2_000_000, 4_096)?.as_deref() != Some(&large[2_000_000..2_004_096])
        || store.read(&kept_large.hash, large.len() as u64 - 5, 100)?.as_deref() != Some(&large[large.len() - 5..])
    {
        return Err("a range of an object is not the bytes put".into());
    }
    if store.read(&[7; 32], 0, 10)?.is_some() {
        return Err("an object that was never put was read".into());
    }
    let stream = StreamKey::new("drill/objects", "conversation/objects/timeline");
    let referring = |state| Record { item: b"with-objects".to_vec(), order: vec![1], state };
    let both: Vec<ObjectRef> = vec![kept_small, kept_large];
    let append = |state| {
        content.append(Batch {
            stream: stream.clone(),
            generation: 0,
            records: vec![referring(state)],
            checkpoint: None,
            durable: false,
        })
    };
    append(State::Is { body: b"refers to two".to_vec(), objects: both })?;
    if !content.refers(&stream.scope, &kept_large.hash)? || content.refers("drill/other", &kept_large.hash)? {
        return Err("a scope's records do not say which objects it may read".into());
    }
    append(State::Is { body: b"refers to one".to_vec(), objects: vec![kept_small] })?;
    if content.refers(&stream.scope, &kept_large.hash)? || !content.refers(&stream.scope, &kept_small.hash)? {
        return Err("an object a record no longer refers to is still counted".into());
    }
    append(State::Gone)?;
    if content.refers(&stream.scope, &kept_small.hash)? {
        return Err("an object of an item that is gone is still counted".into());
    }
    store.remove(&kept_large.hash)?;
    if store.size(&kept_large.hash)?.is_some() {
        return Err("a removed object is still here".into());
    }
    println!("objects: {} and {} bytes put in {took:.1?}; ranges, counts and removal hold", small.len(), large.len());
    reclaiming(content, &stream, root)
}

/// An object goes once nothing refers to it and it was put a while ago; one
/// put again, or referred to, stays.
fn reclaiming(content: &Content, stream: &StreamKey, root: &Path) -> Outcome<()> {
    let store = content.objects();
    let (row, file, held) = (body(11, 5_000), body(12, 200_000), body(13, 70_000));
    let (kept_row, kept_file, kept_held) = (store.put(&row)?, store.put(&file)?, store.put(&held)?);
    let refer = |state| {
        content.append(Batch {
            stream: stream.clone(),
            generation: 0,
            records: vec![Record { item: b"held".to_vec(), order: vec![2], state }],
            checkpoint: None,
            durable: false,
        })
    };
    refer(State::Is { body: b"holds one".to_vec(), objects: vec![kept_held] })?;
    // What a put that was stopped halfway leaves behind.
    let left = root.join(fastctx_content::OBJECTS_DIRECTORY).join("incoming").join("left-by-a-stopped-put");
    std::fs::write(&left, b"half")?;
    let all = |grace: Duration| -> Outcome<fastctx_content::Reclaimed> {
        let mut whole = fastctx_content::Reclaimed::default();
        loop {
            let pass = content.reclaim(grace)?;
            whole.looked += pass.looked;
            whole.removed += pass.removed;
            whole.bytes += pass.bytes;
            if !pass.more {
                return Ok(whole);
            }
        }
    };
    let early = all(Duration::from_secs(3600))?;
    if early.removed != 0 || store.size(&kept_row.hash)?.is_none() || !left.exists() {
        return Err(format!("objects put just now were reclaimed: {early:?}").into());
    }
    std::thread::sleep(Duration::from_millis(30));
    let began = Instant::now();
    let late = all(Duration::ZERO)?;
    let took = began.elapsed();
    if store.size(&kept_row.hash)?.is_some() || store.size(&kept_file.hash)?.is_some() {
        return Err(format!("objects nothing refers to stayed: {late:?}").into());
    }
    if store.read(&kept_held.hash, 0, 16)?.as_deref() != Some(&held[..16]) {
        return Err("an object a record refers to was reclaimed".into());
    }
    if left.exists() {
        return Err("what a stopped put left stayed".into());
    }
    if late.bytes < (row.len() + file.len()) as u64 {
        return Err(format!("reclaiming does not say what it removed: {late:?}").into());
    }
    // Put again, it is here again; and its mark keeps it through a pass
    // that looks only at what was put before.
    if store.put(&file)? != kept_file || store.read(&kept_file.hash, 199_990, 100)?.as_deref() != Some(&file[199_990..])
    {
        return Err("an object put again after it was reclaimed is not here".into());
    }
    refer(State::Gone)?;
    std::thread::sleep(Duration::from_millis(30));
    all(Duration::ZERO)?;
    if store.size(&kept_held.hash)?.is_some() || store.size(&kept_file.hash)?.is_some() {
        return Err("an object whose last record is gone stayed".into());
    }
    println!(
        "reclaiming: {} objects looked at, {} removed ({} bytes) in {took:.1?}",
        late.looked, late.removed, late.bytes
    );
    forgetting(content)
}

/// A scope that is forgotten leaves nothing: no item, no frame, no count of
/// an object, and no object only it referred to; another scope's stay.
fn forgetting(content: &Content) -> Outcome<()> {
    let store = content.objects();
    let (gone, kept) = (StreamKey::new("drill/forgotten", "timeline"), StreamKey::new("drill/remembered", "timeline"));
    let (own, shared) = (store.put(&body(21, 90_000))?, store.put(&body(22, 100_000))?);
    let records = |objects: &[ObjectRef]| -> Vec<Record> {
        (0u64..5_000)
            .map(|number| Record {
                item: number.to_be_bytes().to_vec(),
                order: number.to_be_bytes().to_vec(),
                state: State::Is {
                    body: body(number, 200),
                    objects: if number == 0 { objects.to_vec() } else { Vec::new() },
                },
            })
            .collect()
    };
    for (stream, objects) in [(&gone, vec![own, shared]), (&kept, vec![shared])] {
        for chunk in records(&objects).chunks(1_000) {
            content.append(Batch {
                stream: stream.clone(),
                generation: 0,
                records: chunk.to_vec(),
                checkpoint: None,
                durable: false,
            })?;
        }
    }
    let began = Instant::now();
    let (mut passes, mut items) = (0, 0);
    loop {
        let pass = content.forget(&gone.scope)?;
        passes += 1;
        items += pass.items;
        if !pass.more {
            break;
        }
    }
    let took = began.elapsed();
    if items != 5_000 || content.revision(&gone)?.is_some() || content.refers(&gone.scope, &shared.hash)? {
        return Err(format!("a forgotten scope left something: {items} items went in {passes} passes").into());
    }
    if content.page(&kept, &Anchor::Latest, 10, 1 << 20)?.items.len() != 10
        || !content.refers(&kept.scope, &shared.hash)?
        || store.size(&shared.hash)?.is_none()
    {
        return Err("forgetting one scope took from another".into());
    }
    if content.forget(&gone.scope)? != fastctx_content::Forgotten::default() {
        return Err("forgetting a scope twice found something the second time".into());
    }
    // The object only the forgotten scope referred to was put just now, so
    // it is left for reclaiming, which takes it once it is no longer new.
    std::thread::sleep(Duration::from_millis(30));
    while content.reclaim(Duration::ZERO)?.more {}
    if store.size(&own.hash)?.is_some() || store.size(&shared.hash)?.is_none() {
        return Err("the objects of a forgotten scope are not the ones that went".into());
    }
    println!("forgetting: 5000 items in {passes} passes in {took:.1?}");
    Ok(())
}
