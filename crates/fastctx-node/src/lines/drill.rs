//! A reader holding a line source leaves the rest of the source going: what
//! waits on it for a change and what watches it read what it shows
//! (`super::Shown`), never the reader's lock. The release test
//! `line_sources` runs it as a link's own thread would, one thread for every
//! task: one of them waiting on the lock holds all the others, and a reader
//! whose lines that thread was to send holds it in turn, for good.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::ipc::Failure;
use fastctx_types::lines::{Extent, Result as Answer};

use super::{Shown, Source, State, changed, failed, monitor, observe, refresh, show};

/// How long the reader holds the source at the most: past it the drill lets
/// it go itself, so that a thread held behind it shows as held, not as a
/// drill that never ends.
const HOLD: Duration = Duration::from_secs(3);
/// How long the drill's one thread may go without a tick.
const GAP: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(10);
/// The version the source shows as the drill starts, and the first line its
/// last change touched.
const VERSION: u64 = 3;
const FIRST: u64 = 7;

type Events = tokio::sync::watch::Receiver<Result<u64, String>>;

pub fn run() -> Result<(), String> {
    let root = tempfile::tempdir().map_err(err)?;
    let path = root.path().join("source.txt");
    fastctx_platform::disk::write(&path, numbered(3000)).map_err(err)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(err)?;
    runtime.block_on(drill(root.path(), &path))
}

async fn drill(directory: &Path, path: &Path) -> Result<(), String> {
    let (source, events) = opened(path, directory)?;
    shown_alone(&source)?;

    let longest = Arc::new(AtomicU64::new(0));
    let ticking = fastctx_platform::task::spawn(tick(longest.clone()));

    // A reader takes the source and gives its first line; nothing takes it.
    let (held, holding) = std::sync::mpsc::channel();
    let (sender, mut lines) = tokio::sync::mpsc::channel(1);
    let reading = source.clone();
    let reader = std::thread::spawn(move || {
        let mut state = reading.state.lock().expect("line source");
        let index = state.index.as_mut().ok_or("the source is closed")?;
        let _ = held.send(());
        let cancel = AtomicBool::new(false);
        index.visit(0, u64::MAX, &cancel, |_, text, _| sender.blocking_send(text).map_err(err))
    });
    fastctx_platform::task::blocking(move || holding.recv()).await.map_err(|_| "the reader never took the source")?;
    // The reader's lines are let go of as the drill is done with it, or past
    // `HOLD` whatever the drill's thread is doing.
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (first, taken) = std::sync::mpsc::channel();
    let letting_go = std::thread::spawn(move || {
        let _ = released.recv_timeout(HOLD);
        let _ = first.send(lines.try_recv().is_ok());
        drop(lines);
    });

    // The source is watched as it is opened, and something changed.
    fastctx_platform::task::spawn(monitor(Arc::downgrade(&source), events, source.closed.subscribe()));
    told(&source);
    tokio::time::sleep(GAP / 4).await;

    // A change shown already is told at once, without a look.
    let not_looked = || async { Err::<(), _>(failed("it looked at the source again")) };
    expect_changed(changed(&source, VERSION - 1, not_looked()).await, FIRST, "one version behind")?;
    expect_changed(changed(&source, 1, not_looked()).await, 0, "two versions behind")?;
    match changed(&source, VERSION + 1, not_looked()).await {
        Err(error) if error.message.contains("ahead") => {}
        other => return Err(format!("a version ahead of the source was answered {other:?}")),
    }
    // One at the version shown looks at the source afresh, behind the
    // reader, and waits.
    let waiting = {
        let source = source.clone();
        fastctx_platform::task::spawn(async move {
            let look = refresh(source.clone(), None);
            changed(&source, VERSION, look).await
        })
    };
    tokio::time::sleep(GAP / 4).await;
    if waiting.is_finished() {
        return Err(format!("a wait at the version shown ended while the reader held the source: {:?}", waiting.await));
    }
    let held_for = longest.load(Ordering::Relaxed);
    let _ = release.send(());
    let read = fastctx_platform::task::blocking(move || reader.join()).await.map_err(|_| "the reader panicked")?;
    if read.is_ok() {
        return Err("the reader gave all its lines though nothing took them".into());
    }
    fastctx_platform::task::blocking(move || letting_go.join()).await.map_err(|_| "letting go panicked")?;
    if !taken.recv().map_err(err)? {
        return Err("the reader gave no line before it held".into());
    }
    if held_for > GAP.as_millis() as u64 {
        return Err(format!("the drill's thread went {held_for} ms without a tick while the reader held the source"));
    }

    // Let go of, the source is looked at again; nothing changed, so the wait
    // goes on.
    tokio::time::sleep(GAP / 4).await;
    if waiting.is_finished() {
        return Err(format!("a wait at the version shown ended with nothing changed: {:?}", waiting.await));
    }
    // A change that ends the source wakes it.
    fastctx_platform::disk::write(path, numbered(3001)).map_err(err)?;
    told(&source);
    match tokio::time::timeout(GAP * 2, waiting).await {
        Ok(Ok(Err(error))) if error.message.contains("changed") => {}
        Ok(other) => return Err(format!("the source ended, and a wait on it heard {other:?}")),
        Err(_) => return Err("the source ended, and a wait on it went on".into()),
    }
    if source.shown.borrow().error.is_none() {
        return Err("the source ended, and shows no error".into());
    }

    ticking.abort();
    let longest = longest.load(Ordering::Relaxed);
    if longest > GAP.as_millis() as u64 {
        return Err(format!("the drill's thread went {longest} ms without a tick"));
    }
    Ok(())
}

/// Whether to look at the source again, changing alone, is shown without
/// waking what waits on it.
fn shown_alone(source: &Source) -> Result<(), String> {
    let mut shown = source.shown.subscribe();
    let _ = shown.borrow_and_update();
    {
        let mut state = source.state.lock().expect("line source");
        state.stale = true;
        show(source, &state, false);
    }
    let woken = shown.has_changed().map_err(err)?;
    let stale = shown.borrow().stale;
    let mut state = source.state.lock().expect("line source");
    state.stale = false;
    show(source, &state, false);
    match (woken, stale) {
        (false, true) => Ok(()),
        (true, _) => Err("whether to look again, changing alone, woke what waits".into()),
        (false, false) => Err("whether to look again was not shown".into()),
    }
}

/// A source of `path` opened here, showing `VERSION`, its last change from
/// `FIRST`.
fn opened(path: &Path, directory: &Path) -> Result<(Arc<Source>, Events), String> {
    let snapshot = observe(path).map_err(err)?;
    let index = fastctx_hands::lines::Index::open(path, directory, false, None, Arc::new(AtomicBool::new(false)))?;
    let extent = Extent { length: index.length, complete: true, version: VERSION };
    let state = State {
        index: Some(index),
        snapshot,
        extent,
        first: FIRST,
        error: None,
        stale: false,
        unsaid: None,
        projection: None,
        notes_broken: false,
        writes: Vec::new(),
        writes_revision: None,
    };
    let (events, watched) = tokio::sync::watch::channel(Ok(0u64));
    let shown = Shown::of(&state);
    let source = Arc::new(Source {
        path: path.to_path_buf(),
        immutable: false,
        annotation: None,
        job_record: None,
        notes_record: None,
        state: Mutex::new(state),
        shown: tokio::sync::watch::channel(shown).0,
        closed: tokio::sync::watch::channel(false).0,
        events,
        _watcher: Mutex::new(fastctx_watch::Watcher::new(|_: notify::Result<notify::Event>| {})),
        cancelled: Arc::new(AtomicBool::new(false)),
        requests: Mutex::new(Vec::new()),
        copy_whole: None,
        _copied: None,
    });
    Ok((source, watched))
}

/// Tells the source's watch that its file changed, as its events do.
fn told(source: &Source) {
    source.events.send_modify(|state| {
        if let Ok(version) = state {
            *version += 1;
        }
    });
}

/// Notes the longest the drill's thread went between ticks.
async fn tick(longest: Arc<AtomicU64>) {
    let mut last = Instant::now();
    loop {
        tokio::time::sleep(TICK).await;
        let now = Instant::now();
        longest.fetch_max(now.duration_since(last).as_millis() as u64, Ordering::Relaxed);
        last = now;
    }
}

fn expect_changed(answer: Result<Answer, Failure>, first: u64, behind: &str) -> Result<(), String> {
    match answer {
        Ok(Answer::Changed { extent, first_changed_line })
            if extent.version == VERSION && first_changed_line == first =>
        {
            Ok(())
        }
        other => Err(format!("a wait {behind} was answered {other:?}")),
    }
}

fn numbered(count: u64) -> String {
    (0..count).map(|line| format!("line {line}\n")).collect()
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
