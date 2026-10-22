//! The projects' ledgers at the hub, as this machine reads and writes them.
//! Entries come back as their authors signed them, each checked here.

use fastctx_types::MachineName;
use fastctx_types::access::TownId;
use jiff::Timestamp;

use super::Link;
use crate::ledger::{Entry, File};
use crate::sign::Signed;
use crate::wire::{Question, Told};

/// Entries of a project, each with its number, and whether more follow.
pub type Page = (Vec<PageItem>, bool);

/// One entry of a page, in its place. One whose signature does not check
/// out is kept in its place with why, so what follows it can still be
/// taken in order; it is never used.
pub struct PageItem {
    pub seq: u64,
    pub entry: Option<Entry>,
    pub signed: Signed,
    pub refused: Option<String>,
}

impl Link {
    /// An entry of `town` by this machine, written now.
    pub fn entry(&self, town: &TownId, machines: Option<Vec<MachineName>>, files: Vec<File>) -> Entry {
        Entry {
            hub: self.inner.hub.clone(),
            town: town.clone(),
            author: self.inner.me(),
            key: self.inner.key.public(),
            at: Timestamp::now(),
            machines,
            files,
        }
    }

    /// Signs an entry as this machine.
    pub fn sign_entry(&self, entry: &Entry) -> Signed {
        crate::ledger::sign(&self.inner.key, entry)
    }

    /// Keeps an entry in its project's ledger at the hub. Returns its number.
    pub async fn ledger_write(&self, town: &TownId, entry: Signed) -> Result<u64, String> {
        match self.inner.ask(Question::LedgerWrite { town: town.clone(), entry }).await? {
            Told::Written { seq } => Ok(seq),
            _ => Err("the hub answered something else".to_string()),
        }
    }

    /// The legacy catalog, with each project's latest entry. Current hubs
    /// list projects available to this machine's accounts.
    pub async fn ledger_towns(&self) -> Result<Vec<(TownId, u64)>, String> {
        match self.inner.ask(Question::LedgerTowns).await? {
            Told::Towns { towns } => Ok(towns),
            _ => Err("the hub answered something else".to_string()),
        }
    }

    /// Every project this machine may use, without subscribing to their
    /// ledgers.
    pub async fn ledger_catalog(&self) -> Result<Vec<(TownId, u64)>, String> {
        match self.inner.ask(Question::LedgerCatalog).await? {
            Told::Towns { towns } => Ok(towns),
            _ => Err("the hub answered something other than the projects".to_string()),
        }
    }

    /// The entries of a project after `after`.
    pub async fn ledger_read(&self, town: &TownId, after: u64) -> Result<Page, String> {
        let answer = self.inner.ask(Question::LedgerRead { town: town.clone(), after }).await?;
        self.page(town, answer)
    }

    /// None means no control connection, or that it ended before answering.
    /// Refusals and invalid answers remain errors, not offline cache hits.
    pub async fn try_ledger_catalog(&self) -> Result<Option<Vec<(TownId, u64)>>, String> {
        let Some(connection) = self.inner.connection() else { return Ok(None) };
        match self.inner.ask_on(connection, Question::LedgerCatalog).await? {
            Some(Told::Towns { towns }) => Ok(Some(towns)),
            None => Ok(None),
            _ => Err("the hub answered something other than the projects".to_string()),
        }
    }

    /// Reads and checks a page of a project without waiting for
    /// reconnection. None has the same connection meaning as
    /// `try_ledger_catalog`.
    pub async fn try_ledger_read(&self, town: &TownId, after: u64) -> Result<Option<Page>, String> {
        let Some(connection) = self.inner.connection() else { return Ok(None) };
        let Some(answer) = self.inner.ask_on(connection, Question::LedgerRead { town: town.clone(), after }).await?
        else {
            return Ok(None);
        };
        self.page(town, answer).map(Some)
    }

    /// The entries an answer holds, each checked.
    fn page(&self, town: &TownId, answer: Told) -> Result<Page, String> {
        let (entries, more) = match answer {
            Told::Entries { entries, more } => (entries, more),
            _ => return Err("the hub answered something else".to_string()),
        };
        let hub = &self.inner.hub;
        let read = entries
            .into_iter()
            .map(|(seq, signed)| match crate::ledger::open(&signed, hub, town) {
                Ok(entry) => PageItem { seq, entry: Some(entry), signed, refused: None },
                Err(error) => PageItem { seq, entry: None, signed, refused: Some(error) },
            })
            .collect();
        Ok((read, more))
    }
}
