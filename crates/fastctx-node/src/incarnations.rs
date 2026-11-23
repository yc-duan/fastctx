//! When this machine first saw each account as it is now. An account deleted
//! and opened again under the same name is another: what this machine did for
//! the one before goes into none of the new one's records. The hub's clock
//! tells when an account was opened, and a margin covers how far it and this
//! machine's may differ (what earlier releases checked before writing an account's records); within that margin an
//! account made again soon after the one before it worked here would take
//! what that one did (codex-357de52a, third round). What this machine did is
//! timed by its own clock, and so is the moment it first saw the account made
//! again: compared, they need no margin.
//!
//! The moment is noted as the directory comes (`world::directory_changed`),
//! before anything can be asked here for the account made again: work for an
//! account comes through a hub whose directory holds it. An account this
//! machine never saw another of its name — one seen here first, as an
//! install from before this began noting, a network joined, an account new
//! or shown to this machine anew — has no moment: nothing done here was
//! another's of its name that this machine knew, and the margin alone tells
//! (codex-357de52a, fourth round #7). A clock set back past the moment can
//! let what was done before it seem done after: not guarded.

use std::collections::BTreeMap;
use std::sync::Mutex;

use fastctx_net::directory::Directory;
use fastctx_net::ids::HubId;

use crate::Node;

/// Where it is kept, in the World's directory.
const FILE: &str = "accounts-seen.json";

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Seen {
    /// When the account was opened, as the hub tells it, in milliseconds.
    opened: i64,
    /// When this machine first saw it so, by its own clock, in milliseconds:
    /// none for one it saw first as it began noting.
    since: Option<i64>,
}

/// Each network's accounts, by name.
type Book = BTreeMap<String, BTreeMap<String, Seen>>;

#[derive(Default)]
pub(crate) struct Incarnations {
    book: Mutex<Option<Held>>,
}

/// The book, and whether it changed since it was last kept.
struct Held {
    book: Book,
    unkept: bool,
}

impl Incarnations {
    /// Notes the accounts `directory` of `hub` holds: one opened otherwise
    /// than it was seen before is seen so from now on; one not seen before
    /// has no moment. Call as the directory comes.
    pub(crate) fn saw(&self, node: &Node, hub: &HubId, directory: &Directory) {
        let now = jiff::Timestamp::now().as_millisecond();
        self.with(node, |book| {
            let accounts = book.entry(hub.to_string()).or_default();
            let mut changed = false;
            for account in &directory.accounts {
                let opened = account.opened.as_millisecond();
                match accounts.get_mut(account.name.as_str()) {
                    Some(seen) if seen.opened == opened => {}
                    Some(seen) => {
                        *seen = Seen { opened, since: Some(now) };
                        changed = true;
                    }
                    None => {
                        accounts.insert(account.name.to_string(), Seen { opened, since: None });
                        changed = true;
                    }
                }
            }
            changed
        });
    }

    /// Runs `change` on the book, read first where it is not yet, and keeps
    /// it where `change` says it changed, or an earlier change could not be
    /// kept (fourth round #8). A book that cannot be read is begun again, as
    /// from an install from before it.
    fn with(&self, node: &Node, change: impl FnOnce(&mut Book) -> bool) {
        let path = node.world.path().join(FILE);
        let mut held = self.book.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let held = held.get_or_insert_with(|| Held {
            book: fastctx_net::link::state::read_or(&path, Book::new()).unwrap_or_else(|error| {
                node.log.line(format_args!("reading when accounts were first seen here: {error}"));
                Book::new()
            }),
            unkept: false,
        });
        held.unkept |= change(&mut held.book);
        if held.unkept {
            match fastctx_net::link::state::write(&path, &held.book) {
                Ok(()) => held.unkept = false,
                Err(error) => node.log.line(format_args!("keeping when accounts were first seen here: {error}")),
            }
        }
    }
}
