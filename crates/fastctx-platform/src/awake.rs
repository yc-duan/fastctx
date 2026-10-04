//! Elapsed running time and a measured interval containing suspended time.

use std::io;
use std::time::Duration;

pub struct Sample {
    pub awake: Duration,
    sleep_low: Duration,
    sleep_high: Duration,
}

impl Sample {
    pub fn now() -> io::Result<Self> {
        let before = active()?;
        let boot = continuous()?;
        let after = active()?;
        Ok(Self { awake: after, sleep_low: boot.saturating_sub(after), sleep_high: boot.saturating_sub(before) })
    }

    /// The clocks are sampled in active/continuous/active order. Therefore
    /// continuous - active lies in [boot - after, boot - before]. Disjoint
    /// intervals prove that suspended time increased, without an invented
    /// tolerance or confusion with a wall-clock adjustment.
    pub fn suspended_since(&self, earlier: &Self) -> bool {
        self.sleep_low > earlier.sleep_high
    }
}

#[cfg(target_os = "linux")]
fn clock(id: libc::clockid_t) -> io::Result<Duration> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: an initialized output and a supported system clock.
    if unsafe { libc::clock_gettime(id, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Duration::new(time.tv_sec as _, time.tv_nsec as _))
}
#[cfg(target_os = "linux")]
fn active() -> io::Result<Duration> {
    clock(libc::CLOCK_MONOTONIC)
}
#[cfg(target_os = "linux")]
fn continuous() -> io::Result<Duration> {
    clock(libc::CLOCK_BOOTTIME)
}

#[cfg(windows)]
fn active() -> io::Result<Duration> {
    let mut ticks = 0;
    // SAFETY: writable output; units are 100 ns and exclude sleep.
    unsafe {
        windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTimePrecise(&mut ticks);
    }
    Ok(Duration::from_nanos(ticks * 100))
}
#[cfg(windows)]
fn continuous() -> io::Result<Duration> {
    let mut ticks = 0;
    // SAFETY: writable output; the same clock includes sleep here.
    unsafe {
        windows_sys::Win32::System::WindowsProgramming::QueryInterruptTimePrecise(&mut ticks);
    }
    Ok(Duration::from_nanos(ticks * 100))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn active() -> io::Result<Duration> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "the running-time clock is not implemented on this platform"))
}
#[cfg(not(any(target_os = "linux", windows)))]
fn continuous() -> io::Result<Duration> {
    active()
}
