//! The moment something went wrong, written down while it still lasts
//! (`specs/world-engine.md`, 量与告警): what every task waits for and holds,
//! the violations, the measures, and the log's last lines. A restart wipes
//! all of that from memory; the file keeps it.
//!
//! At most one scene a minute, the newest twenty kept, in the place the
//! process names at startup; a process that named none writes none.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

const KEEP: usize = 20;
const SPACING_SECONDS: i64 = 60;
const PREFIX: &str = "scene-";

type Tail = Box<dyn Fn() -> Vec<String> + Send + Sync>;

static PLACE: OnceLock<PathBuf> = OnceLock::new();
static TAIL: OnceLock<Tail> = OnceLock::new();
static LAST: AtomicI64 = AtomicI64::new(i64::MIN);

/// Where this process writes its scenes.
pub fn set_place(dir: PathBuf) {
    let _ = PLACE.set(dir);
}

/// What gives the log's last lines.
pub fn set_tail(tail: impl Fn() -> Vec<String> + Send + Sync + 'static) {
    let _ = TAIL.set(Box::new(tail));
}

/// Writes the scene down on a thread of its own.
pub fn capture(reason: impl Into<String>) {
    if !due() {
        return;
    }
    let reason = reason.into();
    let _ = std::thread::Builder::new().name("scene".into()).spawn(move || {
        let _ = write(&reason);
    });
}

/// Writes the scene down on this thread, for a thread about to end the
/// process.
pub fn capture_now(reason: &str) -> Option<PathBuf> {
    if !due() {
        return None;
    }
    write(reason).ok()
}

/// The scenes kept, newest last.
pub fn kept() -> Vec<PathBuf> {
    let Some(dir) = PLACE.get() else { return Vec::new() };
    scenes(dir)
}

fn due() -> bool {
    if PLACE.get().is_none() {
        return false;
    }
    let now = jiff::Timestamp::now().as_second();
    let last = LAST.load(Ordering::Relaxed);
    if last != i64::MIN && now - last < SPACING_SECONDS {
        return false;
    }
    LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
}

fn write(reason: &str) -> io::Result<PathBuf> {
    let dir = PLACE.get().ok_or_else(|| io::Error::other("this process names no place for scenes"))?;
    crate::ensure_private_dir(dir)?;
    let at = jiff::Timestamp::now();
    let mut text = format!("{at} {reason}\n\n== What tasks wait for and hold\n");
    let snapshot = crate::doing::snapshot(Duration::ZERO, Duration::ZERO);
    for stuck in &snapshot.stuck {
        let _ = writeln!(text, "{} s: {} {}", stuck.since.as_secs(), stuck.doing, stuck.what);
    }
    for cycle in &snapshot.cycles {
        let _ = writeln!(text, "in a circle: {}", cycle.links.join(" -> "));
    }
    for active in &snapshot.active {
        let _ = writeln!(text, "{} s: {} {}", active.since.elapsed().as_secs(), active.doing, active.what);
    }
    let readings = crate::readings::snapshot();
    let _ = writeln!(text, "\n== Violations ({} in all)", readings.violations_total);
    for violation in &readings.violations {
        let _ = writeln!(
            text,
            "{} x{} {} (worst {} ms, last {}, while {})",
            violation.kind, violation.count, violation.place, violation.worst_ms, violation.last, violation.doing
        );
    }
    text.push_str("\n== Timings (count, p50, p99, max in microseconds)\n");
    for timing in &readings.timings {
        let _ =
            writeln!(text, "{} {} {} {} {}", timing.name, timing.count, timing.p50_us, timing.p99_us, timing.max_us);
    }
    text.push_str("\n== Counts and levels\n");
    for count in &readings.counts {
        let _ = writeln!(text, "{} {}", count.name, count.value);
    }
    for level in &readings.levels {
        let _ = writeln!(text, "{} {}", level.name, level.value);
    }
    text.push_str("\n== The log's last lines\n");
    if let Some(tail) = TAIL.get() {
        for line in tail() {
            text.push_str(&line);
            if !line.ends_with('\n') {
                text.push('\n');
            }
        }
    }
    let path = dir.join(format!("{PREFIX}{}.txt", at.as_millisecond()));
    crate::publish(&path, text.as_bytes(), crate::Permissions::Private)?;
    for old in scenes(dir).iter().rev().skip(KEEP) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

fn scenes(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut scenes: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with(PREFIX)))
        .collect();
    scenes.sort();
    scenes
}
