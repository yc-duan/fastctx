//! One history operation contract for computers and devices without a node.

use super::Link;
use fastctx_types::{
    AccountName, Address,
    access::TownId,
    history::{Author, Catalog, Document, HistoryCall, HistoryReply, HistoryRevision, Scope},
};

struct Target<'a> {
    link: &'a Link,
    account: &'a AccountName,
    scope: Source,
}

enum Source {
    Project(TownId),
    Conversation(Address),
}

impl Link {
    pub(crate) fn require_history_write(&self, account: &AccountName) -> Result<(), String> {
        let view = self.view();
        if view.history_switch && view.account(account).is_some_and(|account| account.history && !account.locked) {
            Ok(())
        } else {
            Err(fastctx_types::history::words::DISABLED.into())
        }
    }

    pub async fn history_call(
        &self,
        account: &AccountName,
        request: HistoryCall,
        author: Option<Author>,
    ) -> Result<HistoryReply, String> {
        if !fastctx_platform::history() {
            return Err("history is not enabled in this release".into());
        }
        if request.writes() {
            self.require_history_write(account)?;
        }
        let scope = match request.scope() {
            Scope::Project { project } => Source::Project(
                self.view().project_named(account, project).ok_or("this account cannot use that project")?.town.clone(),
            ),
            Scope::Conversation { resident, machine } => {
                Source::Conversation(Address { resident: resident.clone(), machine: machine.clone() })
            }
        };
        let target = Target { link: self, account, scope };
        match request {
            HistoryCall::List { .. } => Ok(HistoryReply::Catalog { catalog: target.catalog().await? }),
            HistoryCall::Read { name, version, .. } => {
                Ok(HistoryReply::Document { document: Box::new(target.read(&name, version).await?) })
            }
            HistoryCall::Save { name, text, expected, .. } => {
                let revision = target
                    .save(&name, text, expected, false, author.ok_or("history writes require attribution")?)
                    .await?;
                Ok(HistoryReply::Saved { revision: Box::new(revision) })
            }
            HistoryCall::Retire { name, expected, .. } => {
                let document = target.read(&name, None).await?;
                if document.revision.version != expected {
                    return Err(fastctx_types::history::conflict(Some(document.revision.version)));
                }
                let revision = target
                    .save(
                        &name,
                        document.text,
                        Some(expected),
                        true,
                        author.ok_or("history writes require attribution")?,
                    )
                    .await?;
                Ok(HistoryReply::Saved { revision: Box::new(revision) })
            }
            HistoryCall::Restore { name, version, .. } => {
                // Only a document that stands is restored, over the version it
                // stands at: one deleted for good meanwhile is not brought back
                // by a version read before.
                let expected = target
                    .catalog()
                    .await?
                    .documents
                    .into_iter()
                    .find(|revision| revision.name == name)
                    .map(|revision| revision.version)
                    .ok_or_else(|| fastctx_types::history::gone(&name))?;
                let document = target.read(&name, Some(version)).await?;
                let revision = target
                    .save(
                        &name,
                        document.text,
                        Some(expected),
                        false,
                        author.ok_or("history writes require attribution")?,
                    )
                    .await?;
                Ok(HistoryReply::Saved { revision: Box::new(revision) })
            }
            HistoryCall::Delete { name, expected, .. } => {
                // A hub before deletion would keep it as taken down, its
                // versions still read: it is not told as deleted.
                if !self.view().history_deletion {
                    return Err(fastctx_types::history::words::DELETION_UNSUPPORTED.into());
                }
                let current = target
                    .catalog()
                    .await?
                    .documents
                    .into_iter()
                    .find(|revision| revision.name == name)
                    .ok_or("there is no such history document to delete")?;
                if current.version != expected {
                    return Err(fastctx_types::history::conflict(Some(current.version)));
                }
                let revision = target.delete(&current, author.ok_or("history writes require attribution")?).await?;
                Ok(HistoryReply::Saved { revision: Box::new(revision) })
            }
        }
    }
}

impl Target<'_> {
    /// Deletes for good the document `current` is the latest revision of: a
    /// project's by a revision that deletes it, its ledger keeping what was
    /// written before; a conversation's by deleting its record with every
    /// version of it.
    async fn delete(&self, current: &HistoryRevision, author: Author) -> Result<HistoryRevision, String> {
        match &self.scope {
            Source::Project(town) => {
                let (revision, files) =
                    crate::topic_history::deletion_files(&current.name, &current.description, current.version, author)?;
                let entry = self.link.entry(town, None, files);
                self.link.history_write(self.account, town, self.link.sign_entry(&entry)).await?;
                Ok(revision)
            }
            Source::Conversation(address) => {
                self.link.conversation_history_delete(self.account, address, current, author).await
            }
        }
    }

    async fn catalog(&self) -> Result<Catalog, String> {
        match &self.scope {
            Source::Project(town) => {
                self.link.shared_history_catalog(self.account, town).await.map(|catalog| (*catalog).clone())
            }
            Source::Conversation(address) => self
                .link
                .shared_conversation_history_catalog(self.account, address)
                .await
                .map(|catalog| (*catalog).clone()),
        }
    }

    async fn read(&self, name: &str, version: Option<u64>) -> Result<Document, String> {
        match &self.scope {
            Source::Project(town) => self.link.history_read(self.account, town, name, version).await,
            Source::Conversation(address) => {
                self.link.conversation_history_read(self.account, address, name, version).await
            }
        }
    }

    async fn save(
        &self,
        name: &str,
        text: String,
        expected: Option<u64>,
        retired: bool,
        author: Author,
    ) -> Result<HistoryRevision, String> {
        match &self.scope {
            Source::Project(town) => {
                // A name deleted for good is written again from its deletion.
                let expected = match expected {
                    Some(version) => Some(version),
                    None => self.catalog().await?.deleted.get(name).copied(),
                };
                let (revision, files) =
                    crate::topic_history::revision_files(name, &text, expected, retired, author, None)?;
                let entry = self.link.entry(town, None, files);
                self.link.history_write(self.account, town, self.link.sign_entry(&entry)).await?;
                Ok(revision)
            }
            Source::Conversation(address) => {
                self.link
                    .conversation_history_save(self.account, address, name, text, expected, retired, author, None)
                    .await
            }
        }
    }
}
