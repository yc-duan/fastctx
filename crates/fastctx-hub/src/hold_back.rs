//! A failure the hub goes on through, told in its log at most once a minute
//! however often it recurs.

use std::sync::atomic::{AtomicI64, Ordering};

use crate::Hub;

/// Logs `line` unless `last` says it was told less than a minute ago.
pub(crate) fn log(hub: &Hub, last: &AtomicI64, line: std::fmt::Arguments<'_>) {
    let now = jiff::Timestamp::now().as_second();
    let told = last.load(Ordering::Relaxed);
    if now - told >= 60 && last.compare_exchange(told, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        hub.log.line(line);
    }
}
