//! Queries for callers that can read an existing local copy while offline.

use fastctx_types::TownName;

use super::Link;
use crate::ledger::Entry;
use crate::sign::Signed;
use crate::wire::{Question, Told};

impl Link {
    /// None means no control connection, or that it ended before answering.
    /// Refusals and invalid answers remain errors, not offline cache hits.
    pub async fn try_ledger_catalog(&self) -> Result<Option<Vec<(TownName, u64)>>, String> {
        let Some(connection) = self.inner.connection() else { return Ok(None) };
        match self.inner.ask_on(connection, Question::LedgerCatalog).await? {
            Some(Told::Towns { towns }) => Ok(Some(towns)),
            None => Ok(None),
            _ => Err("the hub answered something other than the project directory".to_string()),
        }
    }

    /// Reads and verifies a ledger page without waiting for reconnection.
    /// None has the same connection meaning as `try_ledger_catalog`.
    pub async fn try_ledger_query(
        &self,
        town: &TownName,
        after: u64,
    ) -> Result<Option<(Vec<(u64, Entry, Signed)>, bool)>, String> {
        let Some(connection) = self.inner.connection() else { return Ok(None) };
        let Some(answer) = self.inner.ask_on(connection, Question::LedgerQuery { town: town.clone(), after }).await?
        else {
            return Ok(None);
        };
        self.read_ledger_answer(town, answer).await.map(Some)
    }
}
