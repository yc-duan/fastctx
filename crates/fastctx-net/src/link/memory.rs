//! Project memory at the hub, usable without an online computer.

use fastctx_types::AccountName;
use fastctx_types::access::TownId;
use fastctx_types::memory::{Document, ProjectIndex};

use crate::sign::Signed;
use crate::wire::{Question, Told};

use super::Link;

impl Link {
    /// Reads the account's map switch, or changes just that field with profile CAS.
    pub async fn memory_settings(
        &self,
        account: &AccountName,
        enabled: Option<bool>,
    ) -> Result<fastctx_types::memory::Settings, String> {
        self.memory_supported()?;
        self.refresh().await?;
        let mut profile = self.profile_read(account)?.profile;
        if let Some(enabled) = enabled {
            let answer = self
                .inner
                .ask(Question::Manage {
                    account: account.clone(),
                    change: crate::wire::Change::MemorySettings { version: profile.version, enabled },
                })
                .await?;
            let Told::ProfileWritten { version } = answer else {
                return Err(fastctx_types::memory::words::SETTING_UNCONFIRMED.into());
            };
            profile.memory = enabled;
            self.confirm_profile(account, profile, version).await?;
        }
        Ok(fastctx_types::memory::Settings { enabled: self.profile_read(account)?.profile.memory })
    }

    fn memory_supported(&self) -> Result<(), String> {
        if self.view().project_memory { Ok(()) } else { Err(fastctx_types::memory::words::HUB_TOO_OLD.into()) }
    }

    /// Reads attributes and the catalog; never downloads journal or spec bodies.
    pub async fn project_index(&self, account: &AccountName, town: &TownId) -> Result<ProjectIndex, String> {
        self.memory_supported()?;
        match self.inner.ask(Question::ProjectIndex { town: town.clone(), account: account.clone() }).await? {
            Told::ProjectIndex { index } => Ok(*index),
            _ => Err(fastctx_types::memory::words::INDEX_UNANSWERED.into()),
        }
    }

    /// The selected revision is checked against its author's ledger signature.
    pub async fn memory_read(
        &self,
        account: &AccountName,
        town: &TownId,
        name: &str,
        version: Option<u32>,
    ) -> Result<Document, String> {
        self.memory_supported()?;
        let answer = self
            .inner
            .ask(Question::MemoryRead { town: town.clone(), account: account.clone(), name: name.into(), version })
            .await?;
        let Told::MemoryDocument { entry, versions, evidence } = answer else {
            return Err(fastctx_types::memory::words::DOCUMENT_UNANSWERED.into());
        };
        let entry = crate::ledger::open(&entry, &self.inner.hub, town)?;
        let Some(crate::memory::Mutation::Revision(revision)) = crate::memory::decode(&entry.files)? else {
            return Err(fastctx_types::memory::words::MISSING_SIGNED_REVISION.into());
        };
        if revision.name != name
            || version.is_some_and(|version| version != revision.version)
            || !versions.contains(&revision)
        {
            return Err(fastctx_types::memory::words::REVISION_MISMATCH.into());
        }
        let text = entry
            .files
            .iter()
            .find(|file| file.path == revision.path("md"))
            .ok_or(fastctx_types::memory::words::MISSING_SIGNED_MARKDOWN)?
            .bytes
            .0
            .clone();
        let text = String::from_utf8(text).map_err(|error| error.to_string())?;
        Ok(Document { revision, text, versions, evidence })
    }

    /// Publishes an already signed append for exactly this account.
    pub async fn memory_write(&self, account: &AccountName, town: &TownId, entry: Signed) -> Result<u64, String> {
        self.memory_supported()?;
        match self.inner.ask(Question::MemoryWrite { town: town.clone(), account: account.clone(), entry }).await? {
            Told::Written { seq } => Ok(seq),
            _ => Err(fastctx_types::memory::words::APPEND_UNCONFIRMED.into()),
        }
    }
}
