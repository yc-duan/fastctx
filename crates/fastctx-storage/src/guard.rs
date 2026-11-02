//! Rules every row the writer changes must keep, checked as each row is about
//! to change. Triggers can keep such rules too, but any trigger on a table has
//! each statement on it journal the pages it touches and run a program for
//! every row: on the node's records that made a row's write four times as
//! dear. A guard costs a lookup.
//!
//! The check is SQLite's pre-update hook, built into the bundled library by
//! the workspace's `LIBSQLITE3_FLAGS` (`.cargo/config.toml`); a build without
//! it fails to link rather than run unguarded.

use crate::Error;
use rusqlite::ffi;
use rusqlite::types::ValueRef;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};

/// A row of the main database about to be inserted, updated or deleted.
pub struct Change<'a> {
    table: &'a str,
    action: Action,
    db: *mut ffi::sqlite3,
    /// Its values live as long as the hook's call.
    call: PhantomData<&'a ()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Insert,
    Update,
    Delete,
}

impl Change<'_> {
    pub fn table(&self) -> &str {
        self.table
    }

    pub fn action(&self) -> Action {
        self.action
    }

    /// A column's value before the change, by its place in the table's
    /// definition; a row being inserted has none.
    pub fn before(&self, column: usize) -> Result<ValueRef<'_>, Error> {
        if self.action == Action::Insert {
            return Err(Error::new("a row being inserted has no value before"));
        }
        self.value(column, true)
    }

    /// A column's value after the change; a row being deleted has none.
    pub fn after(&self, column: usize) -> Result<ValueRef<'_>, Error> {
        if self.action == Action::Delete {
            return Err(Error::new("a row being deleted has no value after"));
        }
        self.value(column, false)
    }

    fn value(&self, column: usize, old: bool) -> Result<ValueRef<'_>, Error> {
        let unread = || Error::new(format!("the guard could not read column {column} of {}", self.table));
        let index = c_int::try_from(column).map_err(|_| unread())?;
        let mut value = std::ptr::null_mut();
        // SAFETY: only made inside the hook's call, on the connection that
        // made it; SQLite keeps the value until the call returns, and the
        // returned reference does not outlive this change.
        unsafe {
            let code = if old {
                ffi::sqlite3_preupdate_old(self.db, index, &mut value)
            } else {
                ffi::sqlite3_preupdate_new(self.db, index, &mut value)
            };
            if code != ffi::SQLITE_OK || value.is_null() {
                return Err(unread());
            }
            Ok(value_ref(value))
        }
    }
}

/// # Safety
/// `value` is a live SQLite value that outlives `'a`.
unsafe fn value_ref<'a>(value: *mut ffi::sqlite3_value) -> ValueRef<'a> {
    // SAFETY: as the caller promises; text is asked for before its length.
    unsafe {
        let bytes = |start: *const u8| {
            let length = usize::try_from(ffi::sqlite3_value_bytes(value)).unwrap_or(0);
            if start.is_null() || length == 0 { &[][..] } else { std::slice::from_raw_parts(start, length) }
        };
        match ffi::sqlite3_value_type(value) {
            ffi::SQLITE_NULL => ValueRef::Null,
            ffi::SQLITE_INTEGER => ValueRef::Integer(ffi::sqlite3_value_int64(value)),
            ffi::SQLITE_FLOAT => ValueRef::Real(ffi::sqlite3_value_double(value)),
            ffi::SQLITE_TEXT => ValueRef::Text(bytes(ffi::sqlite3_value_text(value))),
            _ => ValueRef::Blob(bytes(ffi::sqlite3_value_blob(value).cast())),
        }
    }
}

/// What a database's rows must keep. It is told every change the writer
/// makes, and how each operation and each commit ended, so what it knows
/// of the database is what the database holds.
pub trait Guard: Send {
    /// Refuses the change with why, or lets it be. A refused change fails
    /// its whole operation, which is undone with its savepoint.
    fn check(&mut self, change: &Change<'_>) -> Result<(), Error>;
    /// The operation's changes stand in its transaction, or were undone.
    fn ended(&mut self, kept: bool);
    /// The transaction committed, or failed and undid all it held.
    fn committed(&mut self, stood: bool);
}

pub(crate) type Shared = Arc<Mutex<Box<dyn Guard>>>;

thread_local! {
    /// Why the writer's operation in progress was refused, first reason only.
    static REFUSED: RefCell<Option<Error>> = const { RefCell::new(None) };
    /// Whether the guard is set aside on this writer.
    static SUSPENDED: Cell<bool> = const { Cell::new(false) };
}

/// Sets the guard aside on this writer while held: its changes are still
/// followed, none is refused. For an operation that writes rows the rules
/// fence on purpose, as an import into a conversation fenced for it.
pub struct Suspended {
    was: bool,
}

/// Sets the guard aside until the returned value is dropped; called inside a
/// write on the writer.
pub fn suspend() -> Suspended {
    Suspended { was: SUSPENDED.replace(true) }
}

impl Drop for Suspended {
    fn drop(&mut self) {
        SUSPENDED.set(self.was);
    }
}

/// The hook on the writer's connection; dropped before the connection, it
/// takes itself off.
pub(crate) struct Hooked {
    db: *mut ffi::sqlite3,
    guard: *mut Shared,
}

/// Has the writer's connection tell `guard` each change.
pub(crate) fn install(connection: &rusqlite::Connection, guard: Shared) -> Hooked {
    let guard = Box::into_raw(Box::new(guard));
    // SAFETY: the handle is the open connection's, and the guard it is given
    // lives until `Hooked` takes the hook off again.
    unsafe {
        let db = connection.handle();
        ffi::sqlite3_preupdate_hook(db, Some(changing), guard.cast());
        Hooked { db, guard }
    }
}

impl Drop for Hooked {
    fn drop(&mut self) {
        // SAFETY: the connection is still open; once the hook is off, no
        // call can reach the guard.
        unsafe {
            ffi::sqlite3_preupdate_hook(self.db, None, std::ptr::null_mut());
            drop(Box::from_raw(self.guard));
        }
    }
}

unsafe extern "C" fn changing(
    guard: *mut c_void,
    db: *mut ffi::sqlite3,
    op: c_int,
    database: *const c_char,
    table: *const c_char,
    _: i64,
    _: i64,
) {
    // Nothing unwinds into SQLite.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: SQLite calls with the argument `install` gave and the
        // names of the change's database and table.
        let (guard, database, table) =
            unsafe { (&*guard.cast::<Shared>(), CStr::from_ptr(database), CStr::from_ptr(table)) };
        let Ok(table) = table.to_str() else { return };
        if database.to_bytes() != b"main" {
            return;
        }
        let action = match op {
            ffi::SQLITE_INSERT => Action::Insert,
            ffi::SQLITE_DELETE => Action::Delete,
            // One SQLite does not name has both values to be asked for and
            // neither to be had: it is refused, never let by.
            _ => Action::Update,
        };
        let checked = lock(guard).check(&Change { table, action, db, call: PhantomData });
        if let Err(why) = checked
            && !SUSPENDED.get()
        {
            REFUSED.with(|refused| {
                refused.borrow_mut().get_or_insert(why);
            });
        }
    }));
}

/// Begins an operation with nothing refused.
pub(crate) fn begin() {
    REFUSED.with(|refused| refused.borrow_mut().take());
}

/// Why the operation that just ran was refused, if it was.
pub(crate) fn refused() -> Option<Error> {
    REFUSED.with(|refused| refused.borrow_mut().take())
}

pub(crate) fn lock(guard: &Shared) -> std::sync::MutexGuard<'_, Box<dyn Guard>> {
    guard.lock().unwrap_or_else(PoisonError::into_inner)
}
