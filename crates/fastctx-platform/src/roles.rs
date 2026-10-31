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
}

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
