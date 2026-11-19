//! A hub that cannot start waits longer before each try. Its service starts
//! it again at once whenever it ends, so a hub that fails as it starts would
//! otherwise be started and fail again without end, and without a word to
//! anyone: on 2026-11-19 a hub whose database was damaged started and failed
//! 980,000 times in three and a half hours. A start that ends in an error
//! soon after it began is counted beside the hub's directory, and the next
//! start waits by that count before it opens anything, while the process can
//! still be stopped at once. A start that runs is the end of the count. What
//! cannot be counted leaves the starts as fast as they were.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A start that ends in an error sooner than this after it began failed.
const FAILED_START: Duration = Duration::from_secs(120);
/// Failed starts further apart than this begin the count again.
const FORGOTTEN: Duration = Duration::from_secs(600);
/// The longest a start waits.
const LONGEST: u64 = 120;

fn place(dir: &Path) -> PathBuf {
    dir.join("start-failures")
}

/// The failed starts in a row, and when the last one ended.
fn counted(dir: &Path) -> Option<(u32, jiff::Timestamp)> {
    let text = std::fs::read_to_string(place(dir)).ok()?;
    let mut words = text.split_whitespace();
    let count = words.next()?.parse().ok()?;
    let at = jiff::Timestamp::from_millisecond(words.next()?.parse().ok()?).ok()?;
    Some((count, at))
}

/// How long a start waits after `count` failed starts in a row: five
/// seconds after the first, twice as long after each, two minutes at most.
fn waiting(count: u32) -> u64 {
    if count == 0 { 0 } else { (5u64 << count.saturating_sub(1).min(5)).min(LONGEST) }
}

/// Waits, before a start, as long as the failed starts before it say.
pub(crate) fn wait(dir: &Path) {
    let Some((count, at)) = counted(dir) else { return };
    if jiff::Timestamp::now().duration_since(at).unsigned_abs() > FORGOTTEN {
        return;
    }
    let seconds = waiting(count);
    eprintln!("fastctx hub: it failed to start {count} time(s) in a row; it waits {seconds} s before it tries again");
    std::thread::sleep(Duration::from_secs(seconds));
}

/// Counts a start that ended in an error `lasted` after it began as failed;
/// any other end is the end of the count.
pub(crate) fn ended(dir: &Path, failed: bool, lasted: Duration) {
    if failed && lasted < FAILED_START {
        let now = jiff::Timestamp::now();
        let before = counted(dir)
            .filter(|(_, at)| now.duration_since(*at).unsigned_abs() <= FORGOTTEN)
            .map_or(0, |(count, _)| count);
        let _ = std::fs::write(place(dir), format!("{} {}\n", before.saturating_add(1), now.as_millisecond()));
    } else {
        let _ = std::fs::remove_file(place(dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_waits_longer_after_each_failed_one() {
        let waits: Vec<u64> = (0..9).map(waiting).collect();
        assert_eq!(waits, vec![0, 5, 10, 20, 40, 80, 120, 120, 120]);
    }

    #[test]
    fn failed_starts_are_counted_until_one_runs() {
        let dir = std::env::temp_dir().join(format!("fastctx-hub-starts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ended(&dir, true, Duration::from_secs(1));
        ended(&dir, true, Duration::from_secs(1));
        assert_eq!(counted(&dir).map(|(count, _)| count), Some(2));
        // One that failed after it ran a while is not a failed start.
        ended(&dir, true, Duration::from_secs(600));
        assert!(counted(&dir).is_none());
        ended(&dir, true, Duration::from_secs(1));
        ended(&dir, false, Duration::from_secs(1));
        assert!(counted(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
