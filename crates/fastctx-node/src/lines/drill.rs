//! A reader holding a line source leaves the rest of the source going: what
//! waits on it for a change and what watches it read what it shows
//! (`super::Shown`), never the reader's lock. The release test
//! `line_sources` runs it as a link's own thread would, one thread for every
//! task: one of them waiting on the lock holds all the others, and a reader
//! whose lines that thread was to send holds it in turn, for good.
//!
//! What is checked while the reader holds is checked as it still holds: a
//! thread held behind it would come to it only once the drill let the reader
//! go itself. Every wait said to be waiting was seen to have looked first.

use std::io::Write as _;
use std::path::{Path, PathBuf};
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
/// How long a wait the drill expects to end may take.
const SOON: Duration = Duration::from_secs(1);
/// How long the drill gives what it expects not to happen.
const MEANWHILE: Duration = Duration::from_millis(250);
const TICK: Duration = Duration::from_millis(10);
/// The version the source shows as the drill starts, and the first line its
/// last change touched.
const VERSION: u64 = 3;
const FIRST: u64 = 7;
const LINES: u64 = 3000;

type Events = tokio::sync::watch::Receiver<Result<u64, String>>;
type Waiting = tokio::task::JoinHandle<Result<Answer, Failure>>;

pub fn run() -> Result<(), String> {
    let root = tempfile::tempdir().map_err(err)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(err)?;
    runtime.block_on(async {
        closed_while_looking(root.path()).await?;
        drill(root.path()).await
    })
}

async fn drill(directory: &Path) -> Result<(), String> {
    let path = directory.join("output.txt");
    fastctx_platform::disk::write(&path, numbered(0, LINES)).map_err(err)?;
    let (source, events) = opened(&path, directory)?;
    shown_alone(&source)?;

    let longest = Arc::new(AtomicU64::new(0));
    let ticking = fastctx_platform::task::spawn(tick(longest.clone()));

    // A reader takes the source and gives its first line; nothing takes it.
    let holding = Arc::new(AtomicBool::new(true));
    let (held, holds) = std::sync::mpsc::channel();
    let (sender, mut lines) = tokio::sync::mpsc::channel(1);
    let reader = {
        let (source, holding) = (source.clone(), holding.clone());
        std::thread::spawn(move || {
            let read = (|| {
                let mut state = source.state.lock().expect("line source");
                let index = state.index.as_mut().ok_or("the source is closed")?;
                let _ = held.send(());
                let cancel = AtomicBool::new(false);
                index.visit(0, u64::MAX, &cancel, |_, text, _| sender.blocking_send(text).map_err(err))
            })();
            holding.store(false, Ordering::SeqCst);
            read
        })
    };
    fastctx_platform::task::blocking(move || holds.recv()).await.map_err(|_| "the reader never took the source")?;
    // The reader's lines are let go of as the drill is done with it, or past
    // `HOLD` whatever the drill's thread is doing.
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (first, taken) = std::sync::mpsc::channel();
    let letting_go = std::thread::spawn(move || {
        let _ = released.recv_timeout(HOLD);
        let _ = first.send(lines.try_recv().is_ok());
        drop(lines);
    });
    let still = |what: &str| -> Result<(), String> {
        if holding.load(Ordering::SeqCst) { Ok(()) } else { Err(format!("{what} only once the reader was let go")) }
    };

    // The source is watched as it is opened, a job's output still running,
    // and something changed.
    fastctx_platform::task::spawn(monitor(
        Arc::downgrade(&source),
        events,
        source.closed.subscribe(),
        source.again.clone(),
    ));
    told(&source);
    tokio::time::sleep(MEANWHILE).await;
    still("the drill's thread went on as the watch looked")?;

    // A change shown already is told at once, without a look.
    let not_looked = || async { Err::<(), _>(failed("it looked at the source again")) };
    expect_changed(changed(&source, VERSION - 1, not_looked()).await, VERSION, FIRST, "one version behind")?;
    expect_changed(changed(&source, 1, not_looked()).await, VERSION, 0, "two versions behind")?;
    match changed(&source, VERSION + 1, not_looked()).await {
        Err(error) if error.message.contains("ahead") => {}
        other => return Err(format!("a version ahead of the source was answered {other:?}")),
    }
    still("waits behind the version shown were answered")?;

    // One at the version shown looks at the source afresh, behind the reader,
    // and is told a change another look shows meanwhile without waiting for
    // its own.
    let behind = waiting(&source, VERSION, None);
    tokio::time::sleep(MEANWHILE).await;
    if behind.is_finished() {
        return Err(format!("a wait at the version shown ended with nothing shown: {:?}", behind.await));
    }
    source.shown.send_modify(|shown| {
        shown.extent.version = VERSION + 1;
        shown.first = FIRST + 1;
    });
    expect_changed(within(behind).await?, VERSION + 1, FIRST + 1, "looking behind the reader")?;
    still("a change shown was told to a wait looking behind the reader")?;
    source.shown.send_modify(|shown| {
        shown.extent.version = VERSION;
        shown.first = FIRST;
    });

    // One at the version shown waits on, its look behind the reader.
    let (looked, looking) = tokio::sync::oneshot::channel();
    let waits = waiting(&source, VERSION, Some(looked));
    tokio::time::sleep(MEANWHILE).await;
    if waits.is_finished() {
        return Err(format!("a wait at the version shown ended while the reader held the source: {:?}", waits.await));
    }
    still("a wait at the version shown went on")?;
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

    // Let go of, the source is looked at again: nothing changed, and the wait
    // goes on, its look done.
    tokio::time::timeout(SOON * 2, looking).await.map_err(|_| "the wait never looked")?.map_err(err)?;
    if waits.is_finished() {
        return Err(format!("a wait at the version shown ended with nothing changed: {:?}", waits.await));
    }
    // The job writes more: the watch looks, shows it, and the wait hears it.
    let before = source.shown.borrow().extent.length;
    let mut file = fastctx_platform::disk::open_with(std::fs::OpenOptions::new().append(true), &path).map_err(err)?;
    file.write_all(numbered(LINES, 5).as_bytes()).map_err(err)?;
    drop(file);
    told(&source);
    match within(waits).await? {
        Ok(Answer::Changed { extent, first_changed_line })
            if extent.version == VERSION + 1
                && extent.length > before
                && first_changed_line + 1 >= before
                && first_changed_line <= before => {}
        other => return Err(format!("the job wrote more after {before} lines, and a wait heard {other:?}")),
    }
    // Then its output is cut short: the watch's look ends the source, and a
    // wait already waiting hears so.
    let (looked, looking) = tokio::sync::oneshot::channel();
    let ends = waiting(&source, VERSION + 1, Some(looked));
    tokio::time::timeout(SOON, looking).await.map_err(|_| "the last wait never looked")?.map_err(err)?;
    fastctx_platform::disk::write(&path, numbered(0, 10)).map_err(err)?;
    told(&source);
    match within(ends).await? {
        Err(error) if error.message.contains("changed") => {}
        other => return Err(format!("the source ended, and a wait on it heard {other:?}")),
    }
    if source.shown.borrow().error.is_none() {
        return Err("the source ended, and shows no error".into());
    }

    ticking.abort();
    let longest = longest.load(Ordering::Relaxed);
    if held_for > HOLD.as_millis() as u64 / 2 || longest > HOLD.as_millis() as u64 / 2 {
        return Err(format!(
            "the drill's thread went {held_for} ms without a tick while the reader held, {longest} ms in all"
        ));
    }
    Ok(())
}

/// A source closed is heard at once by a wait looking at it behind a
/// reader.
async fn closed_while_looking(directory: &Path) -> Result<(), String> {
    let path = directory.join("closing.txt");
    fastctx_platform::disk::write(&path, numbered(0, 10)).map_err(err)?;
    let (source, _events) = opened(&path, directory)?;
    let (held, holds) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let holder = {
        let source = source.clone();
        std::thread::spawn(move || {
            let _state = source.state.lock().expect("line source");
            let _ = held.send(());
            let _ = released.recv_timeout(HOLD);
        })
    };
    fastctx_platform::task::blocking(move || holds.recv()).await.map_err(|_| "nothing held the source")?;
    let behind = waiting(&source, VERSION, None);
    tokio::time::sleep(MEANWHILE).await;
    source.closed.send_replace(true);
    let heard = within(behind).await;
    let _ = release.send(());
    fastctx_platform::task::blocking(move || holder.join()).await.map_err(|_| "the holder panicked")?;
    match heard? {
        Err(error) if error.message.contains("closed") => Ok(()),
        other => Err(format!("a source closed while a wait looked at it behind a reader was heard as {other:?}")),
    }
}

/// A wait for a change after `version`, looking at the source afresh: told
/// as its look is done, where `looked` is given.
fn waiting(source: &Arc<Source>, version: u64, looked: Option<tokio::sync::oneshot::Sender<()>>) -> Waiting {
    let source = source.clone();
    fastctx_platform::task::spawn(async move {
        let look = async {
            let done = refresh(source.clone(), None).await;
            if let Some(looked) = looked {
                let _ = looked.send(());
            }
            done
        };
        changed(&source, version, look).await
    })
}

/// What a wait the drill expects to end soon came to.
async fn within(waiting: Waiting) -> Result<Result<Answer, Failure>, String> {
    match tokio::time::timeout(SOON, waiting).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(error)) => Err(format!("a wait panicked: {error}")),
        Err(_) => Err("a wait went on that should have ended".into()),
    }
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

/// The output of a job still running at `path`, opened here, showing
/// `VERSION`, its last change from `FIRST`. Its record is never read: the
/// source stands as one written out from the copy here would, not whole.
fn opened(path: &Path, directory: &Path) -> Result<(Arc<Source>, Events), String> {
    let snapshot = observe(path).map_err(err)?;
    let index = fastctx_hands::lines::Index::open(path, directory, true, None, Arc::new(AtomicBool::new(false)))?;
    let extent = Extent { length: index.length, complete: false, version: VERSION };
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
        job_record: Some(PathBuf::from(directory).join("job.json")),
        notes_record: None,
        state: Mutex::new(state),
        shown: tokio::sync::watch::channel(shown).0,
        again: Arc::default(),
        closed: tokio::sync::watch::channel(false).0,
        events,
        _watcher: Mutex::new(fastctx_watch::Watcher::new(|_: notify::Result<notify::Event>| {})),
        cancelled: Arc::new(AtomicBool::new(false)),
        requests: Mutex::new(Vec::new()),
        copy_whole: Some(false),
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

fn expect_changed(answer: Result<Answer, Failure>, version: u64, first: u64, behind: &str) -> Result<(), String> {
    match answer {
        Ok(Answer::Changed { extent, first_changed_line })
            if extent.version == version && first_changed_line == first =>
        {
            Ok(())
        }
        other => Err(format!("a wait {behind} was answered {other:?}")),
    }
}

fn numbered(from: u64, count: u64) -> String {
    (from..from + count).map(|line| format!("line {line}\n")).collect()
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
