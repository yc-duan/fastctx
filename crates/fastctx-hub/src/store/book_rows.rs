//! The last committed rows of the book, sharing unchanged entries with it.
use super::{
    Book,
    rows::{Changes, Listed, Mapped},
    shelf::Shelf,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, io, sync::Arc};

pub(super) fn list<T: DeserializeOwned + Serialize>(
    shelf: &Shelf,
    kind: &'static str,
) -> io::Result<(Vec<Arc<T>>, Listed<T>)> {
    let rows: Vec<(String, i64, Arc<T>)> = shelf.rows(kind)?;
    let mut positions = Vec::with_capacity(rows.len());
    let mut entries = Vec::with_capacity(rows.len());
    for (key, position, entry) in rows {
        positions.push((key, position));
        entries.push(entry);
    }
    let listed = Listed::read(kind, positions, &entries);
    Ok((entries, listed))
}
pub(super) fn key(kind: &str, entry: &impl Serialize) -> String {
    super::migrate::key(kind, &serde_json::to_value(entry).expect("record identity serializes"))
}
pub(super) struct BookRows {
    accounts: Listed<fastctx_net::directory::Account>,
    members: Listed<fastctx_net::directory::Member>,
    shares: Listed<fastctx_net::directory::Share>,
    assists: Listed<fastctx_net::directory::Assist>,
    plans: Listed<fastctx_net::directory::LoginPlan>,
    acting: Listed<fastctx_net::directory::ActsFor>,
    projects: Listed<fastctx_net::directory::Project>,
    tokens: Listed<super::TokenRecord>,
    removed: Listed<super::Removed>,
    archived: Listed<fastctx_net::directory::Project>,
    project_deletions: Listed<crate::project_deletions::Pending>,
    forgetting: Listed<crate::forget::Pending>,
    passwords: Mapped<fastctx_types::AccountName, fastctx_net::sign::Bytes>,
    log_next: u64,
    uncertain: bool,
    forgetting_last: u64,
}
impl BookRows {
    pub(super) fn load(shelf: &Shelf) -> io::Result<(Book, Self)> {
        let mut book = Book::default();
        let (entries, accounts) = list(shelf, "accounts")?;
        book.directory.accounts = entries;
        let (entries, members) = list(shelf, "members")?;
        book.directory.members = entries;
        let (entries, shares) = list(shelf, "shares")?;
        book.directory.shares = entries;
        let (entries, assists) = list(shelf, "lends")?;
        book.directory.assists = entries;
        let (entries, plans) = list(shelf, "plans")?;
        book.directory.plans = entries;
        let (entries, acting) = list(shelf, "acting")?;
        book.directory.acting = entries;
        let (entries, projects) = list(shelf, "projects")?;
        book.directory.projects = entries;
        let (entries, tokens) = list(shelf, "tokens")?;
        book.tokens = entries;
        let (entries, removed) = list(shelf, "removed")?;
        book.removed = entries;
        let (entries, archived) = list(shelf, "archived")?;
        book.archived = entries;
        let (entries, project_deletions) = list(shelf, "project_deletions")?;
        book.project_deletions = entries;
        let (entries, forgetting) = list(shelf, "forgetting")?;
        book.forgetting.pending = entries;
        let mut passwords = BTreeMap::new();
        for (key, _, value) in shelf.rows("passwords")? {
            passwords.insert(key.parse().map_err(io::Error::other)?, value);
        }
        book.passwords = Arc::new(passwords.clone());
        book.machine_log = crate::machine_log::History::from_database(shelf)?;
        book.forgetting.last = shelf.meta("forgetting.last")?.unwrap_or(0);
        let state = Self {
            passwords: Mapped::read("passwords", passwords),
            log_next: book.machine_log.next,
            uncertain: false,
            forgetting_last: book.forgetting.last,
            accounts,
            members,
            shares,
            assists,
            plans,
            acting,
            projects,
            tokens,
            removed,
            archived,
            project_deletions,
            forgetting,
        };
        Ok((book, state))
    }
    pub(super) fn save(&mut self, shelf: &Shelf, book: &mut Book) -> io::Result<()> {
        self.save_with(shelf, book, Changes::default())
    }

    pub(super) fn save_with(&mut self, shelf: &Shelf, book: &mut Book, mut changes: Changes) -> io::Result<()> {
        if self.uncertain {
            changes.log_after = Some(self.log_next);
        }
        let accounts = self.accounts.changes(&book.directory.accounts, |row| key("accounts", row), &mut changes)?;
        let members = self.members.changes(&book.directory.members, |row| key("members", row), &mut changes)?;
        let shares = self.shares.changes(&book.directory.shares, |row| key("shares", row), &mut changes)?;
        let assists = self.assists.changes(&book.directory.assists, |row| key("lends", row), &mut changes)?;
        let plans = self.plans.changes(&book.directory.plans, |row| key("plans", row), &mut changes)?;
        let acting = self.acting.changes(&book.directory.acting, |row| key("acting", row), &mut changes)?;
        let projects = self.projects.changes(&book.directory.projects, |row| key("projects", row), &mut changes)?;
        let tokens = self.tokens.changes(&book.tokens, |row| key("tokens", row), &mut changes)?;
        let removed = self.removed.changes(&book.removed, |row| key("removed", row), &mut changes)?;
        let archived = self.archived.changes(&book.archived, |row| key("archived", row), &mut changes)?;
        let project_deletions = self.project_deletions.changes(
            &book.project_deletions,
            |row| key("project_deletions", row),
            &mut changes,
        )?;
        let forgetting =
            self.forgetting.changes(&book.forgetting.pending, |row| key("forgetting", row), &mut changes)?;
        let passwords = self.passwords.changes(&book.passwords, |key| key.to_string(), &mut changes)?;
        if self.uncertain || self.log_next != book.machine_log.next {
            changes.meta("machine_log.next", super::rows::body(&book.machine_log.next)?);
        }
        if self.uncertain || self.forgetting_last != book.forgetting.last {
            changes.meta("forgetting.last", super::rows::body(&book.forgetting.last)?);
        }
        book.machine_log.changes(&mut changes)?;
        if let Err(error) = shelf.write(changes) {
            self.uncertain = true;
            self.accounts.lost();
            self.members.lost();
            self.shares.lost();
            self.assists.lost();
            self.plans.lost();
            self.acting.lost();
            self.projects.lost();
            self.tokens.lost();
            self.removed.lost();
            self.archived.lost();
            self.project_deletions.lost();
            self.forgetting.lost();
            self.passwords.lost();
            return Err(error);
        }
        self.uncertain = false;
        self.accounts.kept(accounts);
        self.members.kept(members);
        self.shares.kept(shares);
        self.assists.kept(assists);
        self.plans.kept(plans);
        self.acting.kept(acting);
        self.projects.kept(projects);
        self.tokens.kept(tokens);
        self.removed.kept(removed);
        self.archived.kept(archived);
        self.project_deletions.kept(project_deletions);
        self.forgetting.kept(forgetting);
        self.passwords.kept(passwords);
        self.log_next = book.machine_log.next;
        self.forgetting_last = book.forgetting.last;
        book.machine_log.committed();
        Ok(())
    }
}
