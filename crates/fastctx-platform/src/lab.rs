//! Lab-only kill points (plan 3-3): with `FASTCTX_LAB_DIE_AT` set to
//! `<point>[/<detail>][:<nth>]`, the process ends at once, with code
//! [`DIED`], the nth time (the first by default) it passes that point for
//! that detail (any detail when none is named): no destructors run and
//! nothing buffered is written, as when it is killed there. Unset, passing a
//! point costs one load.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The code a process ended at a kill point exits with.
pub const DIED: u32 = 86;

struct At {
    point: String,
    detail: Option<String>,
    nth: u64,
    passed: AtomicU64,
}

fn at() -> Option<&'static At> {
    static AT: OnceLock<Option<At>> = OnceLock::new();
    AT.get_or_init(|| {
        let value = std::env::var("FASTCTX_LAB_DIE_AT").ok()?;
        let (named, nth) = match value.rsplit_once(':') {
            Some((named, nth)) => (named.to_string(), nth.parse::<u64>().ok()?),
            None => (value, 1),
        };
        let (point, detail) = match named.split_once('/') {
            Some((point, detail)) => (point.to_string(), Some(detail.to_string())),
            None => (named, None),
        };
        Some(At { point, detail, nth: nth.max(1), passed: AtomicU64::new(0) })
    })
    .as_ref()
}

/// Passes the kill point `point` for `detail` (a stream's name, say).
pub fn die_at(point: &str, detail: &str) {
    let Some(at) = at() else { return };
    if at.point != point || at.detail.as_deref().is_some_and(|wanted| wanted != detail) {
        return;
    }
    if at.passed.fetch_add(1, Ordering::SeqCst) + 1 >= at.nth {
        end();
    }
}

#[cfg(windows)]
fn end() -> ! {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // SAFETY: ends this process; the pseudo handle needs no closing.
    unsafe { TerminateProcess(GetCurrentProcess(), DIED) };
    unreachable!("the process was terminated")
}

#[cfg(unix)]
fn end() -> ! {
    // SAFETY: ends this process without running anything of it.
    unsafe { libc::_exit(DIED as i32) }
}
