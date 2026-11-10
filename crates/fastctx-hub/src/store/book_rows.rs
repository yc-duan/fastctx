//! The last committed rows of the book, sharing unchanged entries with it.
use super::{
    Book, BookEdit,
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
    account_forgetting: Listed<crate::forget::accounts::AccountPending>,
    passwords: Mapped<fastctx_types::AccountName, fastctx_net::sign::Bytes>,
    log_next: u64,
    uncertain: bool,
    forgetting_last: u64,
    account_forgetting_last: u64,
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
        let (entries, account_forgetting) = list(shelf, "account_forgetting")?;
        book.account_forgetting.pending = entries;
        let mut passwords = BTreeMap::new();
        for (key, _, value) in shelf.rows("passwords")? {
            passwords.insert(key.parse().map_err(io::Error::other)?, value);
        }
        book.passwords = Arc::new(passwords.clone());
        book.machine_log = crate::machine_log::History::from_database(shelf)?;
        book.forgetting.last = shelf.meta("forgetting.last")?.unwrap_or(0);
        book.account_forgetting.last = shelf.meta("account_forgetting.last")?.unwrap_or(0);
        let state = Self {
            passwords: Mapped::read("passwords", passwords),
            log_next: book.machine_log.next,
            uncertain: false,
            forgetting_last: book.forgetting.last,
            account_forgetting_last: book.account_forgetting.last,
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
            account_forgetting,
        };
        Ok((book, state))
    }
    pub(super) fn save(&mut self, shelf: &Shelf, book: &mut Book, edit: &BookEdit) -> io::Result<BookEdit> {
        self.save_with(shelf, book, Changes::default(), edit)
    }

    pub(super) fn save_with(
        &mut self,
        shelf: &Shelf,
        book: &mut Book,
        mut changes: Changes,
        edit: &BookEdit,
    ) -> io::Result<BookEdit> {
        let edit = if self.uncertain { BookEdit::Full } else { edit.clone() };
        if self.uncertain {
            changes.log_after = Some(self.log_next);
        }
        macro_rules! stage {
            ($field:ident, $entries:expr, $kind:literal) => {{
                let (changed, positions) = edit.list($kind);
                self.$field.stage($entries, changed, positions, |row| key($kind, row), &mut changes)?
            }};
        }
        let accounts = stage!(accounts, &book.directory.accounts, "accounts");
        let members = stage!(members, &book.directory.members, "members");
        let shares = stage!(shares, &book.directory.shares, "shares");
        let assists = stage!(assists, &book.directory.assists, "lends");
        let plans = stage!(plans, &book.directory.plans, "plans");
        let acting = stage!(acting, &book.directory.acting, "acting");
        let projects = stage!(projects, &book.directory.projects, "projects");
        let tokens = stage!(tokens, &book.tokens, "tokens");
        let removed = stage!(removed, &book.removed, "removed");
        let archived = stage!(archived, &book.archived, "archived");
        let project_deletions = stage!(project_deletions, &book.project_deletions, "project_deletions");
        let forgetting = stage!(forgetting, &book.forgetting.pending, "forgetting");
        let account_forgetting = stage!(account_forgetting, &book.account_forgetting.pending, "account_forgetting");
        let passwords = if edit.is_full() {
            Some(self.passwords.changes(&book.passwords, |key| key.to_string(), &mut changes)?)
        } else {
            None
        };
        if self.uncertain || self.log_next != book.machine_log.next {
            changes.meta("machine_log.next", super::rows::body(&book.machine_log.next)?);
        }
        if self.uncertain || self.forgetting_last != book.forgetting.last {
            changes.meta("forgetting.last", super::rows::body(&book.forgetting.last)?);
        }
        if self.uncertain || self.account_forgetting_last != book.account_forgetting.last {
            changes.meta("account_forgetting.last", super::rows::body(&book.account_forgetting.last)?);
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
            self.account_forgetting.lost();
            self.passwords.lost();
            return Err(error);
        }
        self.uncertain = false;
        self.accounts.keep_stage(accounts);
        self.members.keep_stage(members);
        self.shares.keep_stage(shares);
        self.assists.keep_stage(assists);
        self.plans.keep_stage(plans);
        self.acting.keep_stage(acting);
        self.projects.keep_stage(projects);
        self.tokens.keep_stage(tokens);
        self.removed.keep_stage(removed);
        self.archived.keep_stage(archived);
        self.project_deletions.keep_stage(project_deletions);
        self.forgetting.keep_stage(forgetting);
        self.account_forgetting.keep_stage(account_forgetting);
        if let Some(passwords) = passwords {
            self.passwords.kept(passwords);
        }
        self.log_next = book.machine_log.next;
        self.forgetting_last = book.forgetting.last;
        self.account_forgetting_last = book.account_forgetting.last;
        book.machine_log.committed();
        Ok(edit)
    }
}
