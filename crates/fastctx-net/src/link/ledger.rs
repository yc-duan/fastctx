//! The towns' ledgers at the hub, as this machine reads and writes them.
//!
//! Items come back as their signers signed them, each checked against the
//! members; opening a sealed entry and sealing an entry's key again happen
//! here, where this machine's sealing key is.

use fastctx_types::MachineName;

use super::{Inner, Link};
use crate::ledger::sealed::{self, EntryKey, Reader};
use crate::ledger::{Item, NamingKey, SealedEntry, Share, TownId};
use crate::sign::Signed;
use crate::wire::{Question, Told};

/// Items of a town, each with its number, and whether more follow.
pub type Page = (Vec<PageItem>, bool);

/// One item of a page, in its place. One whose signer's signature does not
/// check out is kept in its place with why, so what follows it can still
/// be taken in order; it is never opened.
pub struct PageItem {
    pub seq: u64,
    pub item: Item,
    pub signed: Signed,
    pub refused: Option<String>,
}

impl Link {
    /// Keeps an item in its town's ledger at the hub. Returns its number.
    pub async fn ledger_write(&self, item: Signed) -> Result<u64, String> {
        match self.inner.ask(Question::LedgerWrite { item }).await? {
            Told::Written { seq } => Ok(seq),
            _ => Err("the hub answered something else".to_string()),
        }
    }

    /// Seals `inner`, an entry this machine signed, for the readers of
    /// `town`, as an item signed by this machine.
    pub fn seal_entry(
        &self,
        town: &TownId,
        naming: Option<&NamingKey>,
        paths: &[String],
        machines: Option<Vec<MachineName>>,
        inner: &Signed,
    ) -> Result<Signed, String> {
        let readers = sealed::readers(&self.inner.members().table, town);
        let membership = &self.inner.membership;
        sealed::seal_entry(
            &self.inner.key,
            &membership.world,
            &membership.name,
            town,
            naming,
            paths,
            machines,
            inner,
            &readers,
        )
    }

    /// Signs a statement in the ledger domain, as this machine.
    pub fn sign_entry(&self, entry: &crate::ledger::Entry) -> Signed {
        crate::ledger::sign(&self.inner.key, entry)
    }

    /// Opens `entry` with a copy of its key for this machine, in the entry
    /// or in `shares` (`sealed::open_sealed`).
    pub fn open_sealed<'a>(
        &self,
        entry: &SealedEntry,
        shares: impl IntoIterator<Item = &'a Share>,
    ) -> Result<Option<(EntryKey, Signed)>, String> {
        sealed::open_sealed(entry, shares, &self.inner.membership.name, &self.inner.seal)
    }

    /// Seals the key of the entry numbered `seq` once more, for `to`.
    pub fn share(&self, entry: &SealedEntry, seq: u64, key: &EntryKey, to: &Reader) -> Result<Signed, String> {
        sealed::share(&self.inner.key, &self.inner.membership.name, entry, seq, key, to)
    }

    /// The towns this machine spans, with the number of each one's latest
    /// item at the hub.
    pub async fn ledger_towns(&self) -> Result<Vec<(TownId, u64)>, String> {
        match self.inner.ask(Question::LedgerTowns).await? {
            Told::Towns { towns } => Ok(towns),
            _ => Err("the hub answered something else".to_string()),
        }
    }

    /// Every town of the World, without subscribing to their ledgers.
    pub async fn ledger_catalog(&self) -> Result<Vec<(TownId, u64)>, String> {
        match self.inner.ask(Question::LedgerCatalog).await? {
            Told::Towns { towns } => Ok(towns),
            _ => Err("the hub answered something other than the project directory".to_string()),
        }
    }

    /// The items of a town this machine spans after `after`.
    pub async fn ledger_read(&self, town: &TownId, after: u64) -> Result<Page, String> {
        let answer = self.inner.ask(Question::LedgerRead { town: town.clone(), after }).await?;
        self.inner.items(town, answer).await
    }

    /// The items of any town after `after`, for an explicit query.
    pub async fn ledger_query(&self, town: &TownId, after: u64) -> Result<Page, String> {
        let answer = self.inner.ask(Question::LedgerQuery { town: town.clone(), after }).await?;
        self.inner.items(town, answer).await
    }

    /// None means no control connection, or that it ended before answering.
    /// Refusals and invalid answers remain errors, not offline cache hits.
    pub async fn try_ledger_catalog(&self) -> Result<Option<Vec<(TownId, u64)>>, String> {
        let Some(connection) = self.inner.connection() else { return Ok(None) };
        match self.inner.ask_on(connection, Question::LedgerCatalog).await? {
            Some(Told::Towns { towns }) => Ok(Some(towns)),
            None => Ok(None),
            _ => Err("the hub answered something other than the project directory".to_string()),
        }
    }

    /// Reads and checks a page of a town without waiting for reconnection.
    /// None has the same connection meaning as `try_ledger_catalog`.
    pub async fn try_ledger_query(&self, town: &TownId, after: u64) -> Result<Option<Page>, String> {
        let Some(connection) = self.inner.connection() else { return Ok(None) };
        let Some(answer) = self.inner.ask_on(connection, Question::LedgerQuery { town: town.clone(), after }).await?
        else {
            return Ok(None);
        };
        self.inner.items(town, answer).await.map(Some)
    }
}

impl Inner {
    /// The items an answer holds, each checked against the members. One
    /// that cannot even be read, or is of another town, is an error: the
    /// hub that gave it is not keeping the ledger.
    async fn items(&self, town: &TownId, answer: Told) -> Result<Page, String> {
        let (entries, more) = match answer {
            Told::Entries { entries, more } => (entries, more),
            _ => return Err("the hub answered something else".to_string()),
        };
        let mut table = self.members().table.clone();
        // An item by a member that joined after the members were last asked
        // for: ask again, as for a call from it.
        let unknown = entries
            .iter()
            .any(|(_, signed)| signed.claim::<Item>().is_ok_and(|item| table.key_ever(item.signer()).is_none()));
        if unknown {
            self.refresh().await?;
            table = self.members().table.clone();
        }
        let mut read = Vec::new();
        for (seq, signed) in entries {
            let claimed: Item = signed.claim().map_err(|error| format!("item {seq} of {town}: {error}"))?;
            if claimed.town() != town {
                return Err(format!("item {seq} of {town} is for the town {}", claimed.town()));
            }
            let (item, refused) = match sealed::open_item(&signed, &self.membership.world, &table) {
                Ok(item) => (item, None),
                Err(error) => (claimed, Some(error)),
            };
            read.push(PageItem { seq, item, signed, refused });
        }
        Ok((read, more))
    }
}
