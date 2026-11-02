//! Which kind of work a thread is for (`specs/world-engine.md`, 线程的三种角色).
//!
//! A thread keeps a network line alive, runs asynchronous tasks a slice at a
//! time, or is meant to block. Work that blocks its thread (a blocking lock,
//! file input and output, a long computation) belongs only on the last kind;
//! anywhere else it is noted as a violation (`crate::conduct`).
//!
//! A runtime's threads start as blocking threads and become task threads the
//! first time they wake to run tasks (`on_thread_unpark`), which only the
//! scheduler's threads do: a runtime's blocking pool keeps the role it
//! started with.

use std::cell::Cell;
use std::panic::Location;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// A thread no runtime set up: a command's main thread, or one a
    /// component started for itself.
    Other,
    /// Keeps network lines alive: frames in and out, heartbeats.
    Link,
    /// Carries the bytes of data connections.
    Data,
    /// Runs asynchronous tasks, a slice at a time.
    Work,
    /// Meant to block: a database's threads, a runtime's blocking pool, a
    /// pool of its own.
    Blocking,
}

impl Role {
    pub fn word(self) -> &'static str {
        match self {
            Role::Other => "other",
            Role::Link => "link",
            Role::Data => "data",
            Role::Work => "task",
            Role::Blocking => "blocking",
        }
    }

    /// Whether a thread of this role may block.
    pub fn may_block(self) -> bool {
        matches!(self, Role::Other | Role::Blocking)
    }
}

thread_local! {
    static ROLE: Cell<Role> = const { Cell::new(Role::Other) };
    /// How long this thread has blocked in place, so that the slices of
    /// tasks are timed without it (`crate::task`).
    static IN_PLACE: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

static IN_PLACE_TOOK: LazyLock<&'static crate::readings::Timing> =
    LazyLock::new(|| crate::readings::timing("tasks.in_place"));

pub fn set(role: Role) {
    ROLE.with(|current| current.set(role));
}

pub fn current() -> Role {
    ROLE.with(Cell::get)
}

/// Gives this thread `role` until the result is dropped, then the one it had.
pub fn scoped(role: Role) -> Scoped {
    let previous = current();
    set(role);
    Scoped(previous)
}

pub struct Scoped(Role);

impl Drop for Scoped {
    fn drop(&mut self) {
        set(self.0);
    }
}

/// Notes `what`, about to block this thread, when this thread must not
/// block; the place noted is the caller's.
#[track_caller]
pub fn blocking(what: &'static str) {
    let role = current();
    if role.may_block() {
        return;
    }
    crate::conduct::blocked(role, what, Location::caller());
}

/// Runs `work`, which blocks its thread, on this thread. A task thread of a
/// runtime with several first hands its other tasks to another thread, so
/// only the task that waits waits (`tokio::task::block_in_place`); a thread
/// that may block runs it as it is. On any other thread it is noted as
/// `blocking` notes it, and run. The place noted is the caller's.
#[track_caller]
pub fn in_place<R>(what: &'static str, work: impl FnOnce() -> R) -> R {
    let role = current();
    if role.may_block() {
        return work();
    }
    if role == Role::Work
        && tokio::runtime::Handle::try_current()
            .is_ok_and(|runtime| runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        let began = Instant::now();
        let done = tokio::task::block_in_place(|| {
            let _role = scoped(Role::Blocking);
            work()
        });
        let took = began.elapsed();
        IN_PLACE.with(|so_far| so_far.set(so_far.get() + took));
        IN_PLACE_TOOK.record(took);
        return done;
    }
    crate::conduct::blocked(role, what, Location::caller());
    work()
}

/// Marks this thread, about to run a slice of a task, a task thread when it
/// runs the tasks of a runtime with several: a thread of the blocking pool
/// can be handed a worker's tasks (`in_place`) and run them before it first
/// waits for more, which is when a runtime marks its task threads.
pub(crate) fn running_tasks() {
    if current() == Role::Blocking
        && tokio::runtime::Handle::try_current()
            .is_ok_and(|runtime| runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        set(Role::Work);
    }
}

/// How long this thread has blocked in place so far.
pub(crate) fn in_place_so_far() -> Duration {
    IN_PLACE.with(Cell::get)
}

/// Notes `what`, a lock that guards only memory, about to be taken on this
/// thread, when this thread keeps network lines alive: such a thread takes
/// no lock of the work it carries. The place noted is the caller's.
#[track_caller]
pub fn locking(what: &'static str) {
    let role = current();
    if matches!(role, Role::Link | Role::Data) {
        crate::conduct::blocked(role, what, Location::caller());
    }
}
