//! Conversation history uses its opener's account, with resource-level authorization.

use super::Link;
use fastctx_types::{
    AccountName, Address,
    account_records::{Request as RecordRequest, Response as RecordResponse},
    history::{self, Author, Catalog, Document, HistoryRevision, stored::Stored},
    owned_records::{Request, Response},
};

fn key(conversation: &Address, name: &str) -> Result<String, String> {
    history::markdown::check_name(name)?;
    Ok(format!("{}/{name}", conversation.resident))
}

impl Link {
    pub async fn conversation_history_catalog(
        &self,
        account: &AccountName,
        conversation: &Address,
    ) -> Result<Catalog, String> {
        match self.owned_records(account, Request::Catalog { conversation: conversation.clone() }).await? {
            Response::Catalog { catalog } => Ok(catalog),
            _ => Err("the hub did not return the conversation history catalog".into()),
        }
    }

    pub async fn conversation_history_stored(
        &self,
        account: &AccountName,
        conversation: &Address,
        name: &str,
        version: Option<u64>,
    ) -> Result<Option<Stored>, String> {
        let key = key(conversation, name)?;
        let request = match version {
            Some(version) => Request::Revision {
                conversation: conversation.clone(),
                kind: history::RECORD_KIND.into(),
                key,
                version,
            },
            None => Request::Record {
                conversation: conversation.clone(),
                request: RecordRequest::Read { kind: history::RECORD_KIND.into(), key },
            },
        };
        let record = match self.owned_records(account, request).await? {
            Response::Revision { record } => Some(record),
            Response::Record { response: RecordResponse::Read { record } } => record,
            _ => return Err("the hub did not return the history record".into()),
        };
        let Some(record) = record.filter(|record| !record.metadata.deleted) else { return Ok(None) };
        let stored: Stored = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
        stored.validate()?;
        if stored.revision.name != name || stored.revision.version != record.metadata.version {
            return Err("the conversation history revision differs from its record metadata".into());
        }
        Ok(Some(stored))
    }

    pub async fn conversation_history_read(
        &self,
        account: &AccountName,
        conversation: &Address,
        name: &str,
        version: Option<u64>,
    ) -> Result<Document, String> {
        let selected = self
            .conversation_history_stored(account, conversation, name, version)
            .await?
            .ok_or("the history document or revision does not exist")?;
        let response = self
            .owned_records(
                account,
                Request::Versions {
                    conversation: conversation.clone(),
                    kind: history::RECORD_KIND.into(),
                    key: key(conversation, name)?,
                },
            )
            .await?;
        let Response::Versions { versions } = response else {
            return Err("the hub did not return the history versions".into());
        };
        let versions = versions.into_iter().filter_map(|version| version.history).collect();
        Ok(Document { revision: selected.revision, text: selected.text, versions })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn conversation_history_save(
        &self,
        account: &AccountName,
        conversation: &Address,
        name: &str,
        text: String,
        expected: Option<u64>,
        retired: bool,
        author: Author,
        submission: Option<String>,
    ) -> Result<HistoryRevision, String> {
        self.require_history_write(account)?;
        let description = history::markdown::parse_written(name, &text)?.description;
        let mut expected = expected;
        let mut deletion_seen = false;
        loop {
            let revision = HistoryRevision {
                name: name.into(),
                description: description.clone(),
                version: expected.unwrap_or(0).checked_add(1).ok_or("history revision numbers are exhausted")?,
                retired,
                deleted: false,
                author: author.clone(),
            };
            let stored = Stored { revision: revision.clone(), text: text.clone(), submission: submission.clone() };
            let response = self
                .owned_records(
                    account,
                    Request::Record {
                        conversation: conversation.clone(),
                        request: RecordRequest::Write {
                            kind: history::RECORD_KIND.into(),
                            key: key(conversation, name)?,
                            expected,
                            body: serde_json::to_value(stored).map_err(|error| error.to_string())?,
                        },
                    },
                )
                .await?;
            return match response {
                // A hub put back since numbers the revision above every one
                // written before (`number_floor`): the revision is the number
                // it gave.
                Response::Record { response: RecordResponse::Written { metadata } }
                    if metadata.version >= revision.version =>
                {
                    self.inner.history_catalogs.invalidate(|(_, source)| {
                        matches!(source, super::history_catalogs::Source::Conversation(address) if address.resident == conversation.resident)
                    });
                    Ok(HistoryRevision { version: metadata.version, ..revision })
                }
                // A name deleted for good is written again from its deletion,
                // as a new document.
                Response::Record { response: RecordResponse::Conflict { current: Some(current) } }
                    if current.deleted && expected.is_none() && !deletion_seen =>
                {
                    expected = Some(current.version);
                    deletion_seen = true;
                    continue;
                }
                Response::Record { response: RecordResponse::Conflict { current: Some(current) } }
                    if current.deleted =>
                {
                    Err(history::gone(name))
                }
                Response::Record { response: RecordResponse::Conflict { current } } => {
                    Err(history::conflict(current.map(|metadata| metadata.version)))
                }
                _ => Err("the hub did not confirm the history revision".into()),
            };
        }
    }

    /// Deletes for good the conversation's history document `current` is
    /// the latest revision of: its record and every version of it. A hub of
    /// a release before deletion refuses, the document only retired there.
    pub async fn conversation_history_delete(
        &self,
        account: &AccountName,
        conversation: &Address,
        current: &HistoryRevision,
        author: Author,
    ) -> Result<HistoryRevision, String> {
        self.require_history_write(account)?;
        let response = self
            .owned_records(
                account,
                Request::Record {
                    conversation: conversation.clone(),
                    request: RecordRequest::Delete {
                        kind: history::RECORD_KIND.into(),
                        key: key(conversation, &current.name)?,
                        expected: Some(current.version),
                    },
                },
            )
            .await?;
        match response {
            Response::Record { response: RecordResponse::Written { metadata } } => {
                self.inner.history_catalogs.invalidate(|(_, source)| {
                    matches!(source, super::history_catalogs::Source::Conversation(address) if address.resident == conversation.resident)
                });
                Ok(HistoryRevision {
                    name: current.name.clone(),
                    description: current.description.clone(),
                    version: metadata.version,
                    retired: true,
                    deleted: true,
                    author,
                })
            }
            Response::Record { response: RecordResponse::Conflict { current } } => {
                Err(history::conflict(current.map(|metadata| metadata.version)))
            }
            _ => Err("the hub did not confirm the deletion".into()),
        }
    }
}
