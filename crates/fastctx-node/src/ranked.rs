//! The node's ranked locks (`fastctx_platform::doing`): what each guards and
//! where it comes in the order locks are taken in
//! (`specs/world-concurrency.md`, 锁只管落账).

use std::fmt::Display;

use fastctx_platform::doing::{Mutex, Rank, RwLock};

/// A conversation's management gate.
pub(crate) fn management(id: &impl Display) -> RwLock<()> {
    RwLock::new(Rank::LIFECYCLE, "management", id.to_string(), ())
}

/// The reports of one Claude session, applied in order.
pub(crate) fn reading(session: &str) -> Mutex<()> {
    Mutex::new(Rank::SESSION, "reading", session, ())
}

/// The reports of one Codex thread, applied in order.
pub(crate) fn working(thread: &str) -> Mutex<()> {
    Mutex::new(Rank::THREAD, "working", thread, ())
}

/// Handing a conversation's words to its Claude session.
pub(crate) fn delivering(id: &impl Display) -> Mutex<()> {
    Mutex::new(Rank::DELIVERING, "delivering", id.to_string(), ())
}

/// A conversation's words and turns.
pub(crate) fn speaking(id: &impl Display) -> Mutex<()> {
    Mutex::new(Rank::SPEAKING, "speaking", id.to_string(), ())
}

/// Paying out a conversation's answers.
pub(crate) fn paying(id: &impl Display) -> Mutex<()> {
    Mutex::new(Rank::PAYING, "paying", id.to_string(), ())
}

/// A conversation's shared input.
pub(crate) fn composer<T>(id: &impl Display) -> Mutex<Option<T>> {
    Mutex::new(Rank::COMPOSER, "composer", id.to_string(), None)
}

/// One login's listing of Claude Code's background sessions.
pub(crate) fn listing<T>(login: &impl Display) -> Mutex<Option<T>> {
    Mutex::new(Rank::LISTING, "listing", login.to_string(), None)
}

/// A lock over many conversations, held by a struct that is made by
/// `Default`: a named field type that makes the lock in its place.
macro_rules! machine_lock {
    ($(#[$doc:meta])* $name:ident: $lock:ident = $rank:ident, $label:literal) => {
        $(#[$doc])*
        pub(crate) struct $name(fastctx_platform::doing::$lock<()>);

        impl Default for $name {
            fn default() -> Self {
                Self(fastctx_platform::doing::$lock::new(
                    fastctx_platform::doing::Rank::$rank,
                    $label,
                    "this machine",
                    (),
                ))
            }
        }

        impl std::ops::Deref for $name {
            type Target = fastctx_platform::doing::$lock<()>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

machine_lock!(
    /// Claude sessions being born and claimed.
    Births: RwLock = BIRTHS, "births"
);
machine_lock!(
    /// The kept deletions to ask of hubs, read and written.
    Forgetting: Mutex = FORGETTING, "forgetting"
);
machine_lock!(
    /// Changes of the agents' logins, one at a time.
    LoginChanges: Mutex = LOGINS, "login changes"
);
machine_lock!(
    /// Asking again the logins whose sign-in changed, one round at a time.
    LoginAsking: Mutex = LOGINS, "asking logins again"
);
machine_lock!(
    /// Carrying an account's collaboration files.
    Carrying: Mutex = CARRYING, "carrying"
);
machine_lock!(
    /// Syncing the ledger with the network.
    Syncing: Mutex = SYNCING, "syncing"
);
machine_lock!(
    /// Writing a project's or a page's records.
    Writing: Mutex = WRITING, "writing"
);
machine_lock!(
    /// Decoding a picture into a thumbnail.
    Decoding: Mutex = DECODING, "decoding"
);
machine_lock!(
    /// Reading an attached picture's record.
    Pictures: Mutex = PICTURES, "pictures"
);
machine_lock!(
    /// Jobs' records kept from collection while they are made, read or
    /// changed.
    Disk: SyncMutex = DISK, "disk"
);
machine_lock!(
    /// A command's call facts, emitted in order.
    Emitting: SyncMutex = EMITTING, "call facts"
);
machine_lock!(
    /// Conversations' records, each read and written as one transaction.
    Records: SyncMutex = RECORDS, "records"
);
machine_lock!(
    /// A command's write facts.
    WriteFacts: SyncMutex = WRITE_FACTS, "write facts"
);
