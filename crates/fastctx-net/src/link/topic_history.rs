//! Account-scoped history over the existing signed project ledger.

use super::Link;
use crate::{
    sign::Signed,
    wire::{Question, Told},
};
use fastctx_types::{
    AccountName,
    access::TownId,
    history::{Catalog, Document},
};

impl Link {
    pub async fn history_submitted(
        &self,
        account: &AccountName,
        town: &TownId,
        name: &str,
        submission: &str,
    ) -> Result<bool, String> {
        self.history_supported()?;
        match self
            .inner
            .ask(Question::HistorySubmitted {
                town: town.clone(),
                account: account.clone(),
                name: name.into(),
                submission: submission.into(),
            })
            .await?
        {
            Told::HistorySubmitted { revision } => Ok(revision.is_some()),
            _ => Err("the hub did not return the history submission receipt".into()),
        }
    }

    pub async fn history_catalog(&self, account: &AccountName, town: &TownId) -> Result<Catalog, String> {
        self.history_supported()?;
        match self.inner.ask(Question::HistoryCatalog { town: town.clone(), account: account.clone() }).await? {
            Told::HistoryCatalog { catalog } => Ok(catalog),
            _ => Err("the hub did not return the history catalog".into()),
        }
    }

    pub async fn history_read(
        &self,
        account: &AccountName,
        town: &TownId,
        name: &str,
        version: Option<u64>,
    ) -> Result<Document, String> {
        self.history_document(account, town, name, version, true).await
    }

    pub async fn history_current(&self, account: &AccountName, town: &TownId, name: &str) -> Result<Document, String> {
        self.history_document(account, town, name, None, false).await
    }

    async fn history_document(
        &self,
        account: &AccountName,
        town: &TownId,
        name: &str,
        version: Option<u64>,
        versions: bool,
    ) -> Result<Document, String> {
        self.history_supported()?;
        let answer = self
            .inner
            .ask(Question::HistoryRead {
                town: town.clone(),
                account: account.clone(),
                name: name.into(),
                version,
                versions,
            })
            .await?;
        let Told::HistoryDocument { document } = answer else {
            return Err("the hub did not return the history document".into());
        };
        fastctx_types::history::markdown::parse(name, &document.text)?;
        if document.revision.name != name || version.is_some_and(|version| version != document.revision.version) {
            return Err("the history revision does not match the requested document".into());
        }
        Ok(*document)
    }

    pub async fn history_write(&self, account: &AccountName, town: &TownId, entry: Signed) -> Result<u64, String> {
        self.require_history_write(account)?;
        self.history_supported()?;
        match self.inner.ask(Question::HistoryWrite { town: town.clone(), account: account.clone(), entry }).await? {
            Told::Written { seq } => {
                self.inner.history_catalogs.project(town);
                Ok(seq)
            }
            _ => Err("the hub did not confirm the history write".into()),
        }
    }

    fn history_supported(&self) -> Result<(), String> {
        if self.view().shared_records { Ok(()) } else { Err("update the hub to use history".into()) }
    }
}
