//! Lab-only kill points (plan 3-3): with `FASTCTX_LAB_DIE_AT` set to
//! `<point>[/<detail>][:<nth>]`, the process ends at once, with code
//! [`DIED`], the nth time (the first by default) it passes that point for
//! that detail (any detail when none is named): no destructors run and
//! nothing buffered is written, as when it is killed there. Unset, passing a
//! point costs one load.
//!
//! And lab-only faults (plan 3-6 five): with `FASTCTX_LAB_FAIL_AT` set the
//! same way, or a drill in the process having armed one (`arm_fault`), the
//! nth pass of a fault point gives an error instead of going on — a disk
//! full, a sync or a rename that failed — once. Unarmed, passing one costs
//! one load.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

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

/// A fault armed at a point: given once, the nth time it is passed.
struct Fault {
    point: String,
    detail: Option<String>,
    nth: u64,
    passed: u64,
}

static FAULTS: Mutex<Vec<Fault>> = Mutex::new(Vec::new());
static ARMED: AtomicBool = AtomicBool::new(false);

fn from_environment() {
    static READ: OnceLock<()> = OnceLock::new();
    READ.get_or_init(|| {
        let Ok(value) = std::env::var("FASTCTX_LAB_FAIL_AT") else { return };
        let (named, nth) = match value.rsplit_once(':') {
            Some((named, nth)) => (named.to_string(), nth.parse::<u64>().unwrap_or(1)),
            None => (value, 1),
        };
        let (point, detail) = match named.split_once('/') {
            Some((point, detail)) => (point.to_string(), Some(detail.to_string())),
            None => (named, None),
        };
        arm_fault(&point, detail.as_deref(), nth);
    });
}

/// Arms a fault at `point` for `detail` (any, when none): the `nth` pass
/// from now gives an error, once.
pub fn arm_fault(point: &str, detail: Option<&str>, nth: u64) {
    let mut faults = FAULTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    faults.push(Fault { point: point.to_string(), detail: detail.map(str::to_string), nth: nth.max(1), passed: 0 });
    ARMED.store(true, Ordering::SeqCst);
}

/// Takes away every fault armed and not yet given.
pub fn disarm_faults() {
    FAULTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    ARMED.store(false, Ordering::SeqCst);
}

/// Passes the fault point `point` for `detail`: an error where a fault is
/// armed there and this is its pass. A point whose name begins `full-` fails
/// as a full disk does.
pub fn fail_at(point: &str, detail: &str) -> std::io::Result<()> {
    from_environment();
    if !ARMED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut faults = FAULTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(at) =
        faults.iter().position(|fault| fault.point == point && fault.detail.as_deref().is_none_or(|wanted| wanted == detail))
    else {
        return Ok(());
    };
    faults[at].passed += 1;
    if faults[at].passed < faults[at].nth {
        return Ok(());
    }
    faults.remove(at);
    if faults.is_empty() {
        ARMED.store(false, Ordering::SeqCst);
    }
    let kind = if point.starts_with("full-") { std::io::ErrorKind::StorageFull } else { std::io::ErrorKind::Other };
    Err(std::io::Error::new(kind, format!("a lab fault at {point}")))
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
