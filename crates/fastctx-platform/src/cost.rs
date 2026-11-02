//! What work costs, by what it was done for (`specs/world-engine.md`, 量与告警):
//! how long its tasks held a task thread, how long it blocked (in place, or
//! on the blocking pool), and how many bytes it read from files. Work begun
//! for a call is charged to the call's kind and who asked, and so is every
//! task it starts and every blocking closure it hands over; a task started
//! for nothing in particular is charged to where it was started. The sums
//! are counters, `cost.<what>.task_us`, `cost.<what>.blocking_us` and
//! `cost.<what>.read_bytes`: what costs most is read off a node's readings.

use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::panic::Location;
use std::pin::Pin;
use std::sync::{LazyLock, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::readings::{self, Counter};

/// Accounts kept, enough for every kind of call from every machine of a
/// large network; past it, the rest share `other`.
const ACCOUNTS: usize = 4096;

pub struct Account {
    task: &'static Counter,
    blocking: &'static Counter,
    read: &'static Counter,
}

thread_local! {
    static CHARGED: Cell<Option<&'static Account>> = const { Cell::new(None) };
    /// The time this thread's slices spent in work charged to an account of
    /// its own (`charged`), which the slice around it leaves out.
    static ELSEWHERE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

/// The account named `what`, kept for the process.
pub fn account(what: &str) -> &'static Account {
    static KEPT: LazyLock<Mutex<HashMap<String, &'static Account>>> = LazyLock::new(Default::default);
    let mut kept = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(account) = kept.get(what) {
        return account;
    }
    let what = if kept.len() < ACCOUNTS { what } else { "other" };
    if let Some(account) = kept.get(what) {
        return account;
    }
    let named = |measure: &str| readings::counter(Box::leak(format!("cost.{what}.{measure}").into_boxed_str()));
    let made: &'static Account = Box::leak(Box::new(Account {
        task: named("task_us"),
        blocking: named("blocking_us"),
        read: named("read_bytes"),
    }));
    kept.insert(what.to_string(), made);
    made
}

/// The account of a task started at `at` for nothing in particular.
pub(crate) fn started_at(at: &'static Location<'static>) -> &'static Account {
    static AT: LazyLock<Mutex<HashMap<usize, &'static Account>>> = LazyLock::new(Default::default);
    let key = std::ptr::from_ref(at) as usize;
    if let Some(account) = AT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key) {
        return account;
    }
    let made = account(&format!("task at {}:{}", at.file(), at.line()));
    AT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(key, made);
    made
}

/// The account work on this thread is charged to now.
pub fn current() -> Option<&'static Account> {
    CHARGED.with(Cell::get)
}

/// Charges what this thread does to `account` until the result is dropped.
pub(crate) fn enter(account: Option<&'static Account>) -> Entered {
    Entered(CHARGED.with(|charged| charged.replace(account)))
}

pub(crate) struct Entered(Option<&'static Account>);

impl Drop for Entered {
    fn drop(&mut self) {
        CHARGED.with(|charged| charged.set(self.0));
    }
}

/// How long this thread's slices have spent in work charged on its own.
pub(crate) fn elsewhere_so_far() -> Duration {
    ELSEWHERE.with(Cell::get)
}

/// A slice of a task, `took` long, charged to `account`; a slice around it
/// leaves it out.
pub(crate) fn slice(account: &Account, took: Duration) {
    account.task.add(micros(took));
    ELSEWHERE.with(|so_far| so_far.set(so_far.get() + took));
}

/// Blocking work, `took` long, charged to the current account.
pub(crate) fn blocked(took: Duration) {
    if let Some(account) = current() {
        account.blocking.add(micros(took));
    }
}

/// Bytes read from a file, charged to the current account.
pub(crate) fn read(bytes: usize) {
    if let Some(account) = current() {
        account.read.add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }
}

fn micros(took: Duration) -> u64 {
    u64::try_from(took.as_micros()).unwrap_or(u64::MAX)
}

/// `future`, each of its slices charged to `account`, with every task it
/// starts and every blocking closure it hands over, rather than to the task
/// that awaits it.
pub fn charged<F: Future>(account: &'static Account, future: F) -> Charged<F> {
    Charged { future: Box::pin(future), account }
}

pub struct Charged<F: Future> {
    future: Pin<Box<F>>,
    account: &'static Account,
}

impl<F: Future> Future for Charged<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let _entered = enter(Some(self.account));
        let started = Instant::now();
        let (blocked, elsewhere) = (crate::roles::in_place_so_far(), elsewhere_so_far());
        let polled = self.future.as_mut().poll(cx);
        let took = started.elapsed().saturating_sub(crate::roles::in_place_so_far().saturating_sub(blocked));
        let own = took.saturating_sub(elsewhere_so_far().saturating_sub(elsewhere));
        slice(self.account, own);
        polled
    }
}
