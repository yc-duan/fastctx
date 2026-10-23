//! Project memory at the hub, usable without an online computer.

use fastctx_types::AccountName;
use fastctx_types::access::TownId;
use fastctx_types::memory::{Document, ProjectIndex};

use crate::sign::Signed;
use crate::wire::{Question, Told};

use super::Link;

impl Link {
    fn memory_supported(&self) -> Result<(), String> {
        if self.view().project_memory {
            Ok(())
        } else {
            Err("this hub is too old for project memory; update it".into())
        }
    }

    /// Reads attributes and the catalog; never downloads journal or spec bodies.
    pub async fn project_index(&self, account: &AccountName, town: &TownId) -> Result<ProjectIndex, String> {
        self.memory_supported()?;
        match self.inner.ask(Question::ProjectIndex { town: town.clone(), account: account.clone() }).await? {
            Told::ProjectIndex { index } => Ok(*index),
            _ => Err("the hub answered something other than the project index".into()),
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
            return Err("the hub answered something other than a memory document".into());
        };
        let entry = crate::ledger::open(&entry, &self.inner.hub, town)?;
        let Some(crate::memory::Mutation::Revision(revision)) = crate::memory::decode(&entry.files)? else {
            return Err("the hub returned no signed memory revision".into());
        };
        if revision.name != name
            || version.is_some_and(|version| version != revision.version)
            || !versions.contains(&revision)
        {
            return Err("the signed memory revision does not match the request or history".into());
        }
        let text = entry
            .files
            .iter()
            .find(|file| file.path == revision.path("md"))
            .ok_or("the signed Markdown is missing")?
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
            _ => Err("the hub answered something other than a committed memory append".into()),
        }
    }
}
