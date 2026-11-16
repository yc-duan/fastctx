//! When this machine first saw each account as it is now. An account deleted
//! and opened again under the same name is another: what this machine did for
//! the one before goes into none of the new one's records. The hub's clock
//! tells when an account was opened, and a margin covers how far it and this
//! machine's may differ (`assets::activity::of_this`); within that margin an
//! account made again soon after the one before it worked here would take
//! what that one did (codex-357de52a, third round). What this machine did is
//! timed by its own clock, and so is the moment it first saw the account made
//! again: compared, they need no margin.
//!
//! The moment is noted as the directory comes (`world::directory_changed`),
//! before anything can be asked here for the account made again: work for an
//! account comes through a hub whose directory holds it. An account this
//! machine never saw another of, or saw first when it began noting — an
//! install from before this, a network joined — has no moment: the margin
//! alone tells.

use std::collections::BTreeMap;
use std::sync::Mutex;

use fastctx_net::directory::Directory;
use fastctx_net::ids::HubId;
use fastctx_types::AccountName;

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
    book: Mutex<Option<Book>>,
}

impl Incarnations {
    /// Notes the accounts `directory` of `hub` holds: one opened otherwise
    /// than it was seen before, or not seen before on a network seen before,
    /// is seen so from now on. Call as the directory comes.
    pub(crate) fn saw(&self, node: &Node, hub: &HubId, directory: &Directory) {
        let now = jiff::Timestamp::now().as_millisecond();
        self.with(node, |book| {
            let watched = book.contains_key(hub.as_str());
            let accounts = book.entry(hub.to_string()).or_default();
            let mut changed = !watched;
            for account in &directory.accounts {
                let opened = account.opened.as_millisecond();
                match accounts.get_mut(account.name.as_str()) {
                    Some(seen) if seen.opened == opened => {}
                    Some(seen) => {
                        *seen = Seen { opened, since: Some(now) };
                        changed = true;
                    }
                    None => {
                        accounts.insert(account.name.to_string(), Seen { opened, since: watched.then_some(now) });
                        changed = true;
                    }
                }
            }
            changed
        });
    }

    /// Since when, by this machine's clock, `account` of `hub` is the one
    /// opened at `opened` (milliseconds): none where this machine saw no
    /// other of its name before it. One not noted so yet is noted now.
    pub(crate) fn since(&self, node: &Node, hub: &HubId, account: &AccountName, opened: i64) -> Option<i64> {
        let now = jiff::Timestamp::now().as_millisecond();
        let mut found = None;
        self.with(node, |book| {
            let watched = book.contains_key(hub.as_str());
            let accounts = book.entry(hub.to_string()).or_default();
            match accounts.get(account.as_str()) {
                Some(seen) if seen.opened == opened => {
                    found = seen.since;
                    false
                }
                Some(_) => {
                    found = Some(now);
                    accounts.insert(account.to_string(), Seen { opened, since: found });
                    true
                }
                None => {
                    found = watched.then_some(now);
                    accounts.insert(account.to_string(), Seen { opened, since: found });
                    true
                }
            }
        });
        found
    }

    /// Runs `change` on the book, read first where it is not yet, and keeps
    /// it where `change` says it changed. A book that cannot be read is begun
    /// again, as from an install from before it.
    fn with(&self, node: &Node, change: impl FnOnce(&mut Book) -> bool) {
        let path = node.world.path().join(FILE);
        let mut held = self.book.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let book = held.get_or_insert_with(|| {
            fastctx_net::link::state::read_or(&path, Book::new()).unwrap_or_else(|error| {
                node.log.line(format_args!("reading when accounts were first seen here: {error}"));
                Book::new()
            })
        });
        if change(book)
            && let Err(error) = fastctx_net::link::state::write(&path, &*book)
        {
            node.log.line(format_args!("keeping when accounts were first seen here: {error}"));
        }
    }
}
