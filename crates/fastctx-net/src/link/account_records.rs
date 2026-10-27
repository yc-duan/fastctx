//! Authoritative account records: no bodies or versions are cached by the link.

use fastctx_types::{
    AccountName,
    account_records::{Changed, Request, Response, words},
};

use super::Link;
use crate::wire::{Question, Told};

impl Link {
    /// Queries the hub at the time of this call. A disconnected or older hub
    /// returns an error; a compare-and-swap conflict is a structured response.
    pub async fn account_records(&self, account: &AccountName, request: Request) -> Result<Response, String> {
        fastctx_platform::doing::outside(
            "the hub's account records",
            std::time::Duration::from_secs(60),
            Box::pin(self.account_records_now(account, request)),
        )
        .await
        .map_err(|late| late.to_string())?
    }

    async fn account_records_now(&self, account: &AccountName, request: Request) -> Result<Response, String> {
        request.validate()?;
        let connection = self.inner.connected().await.ok_or_else(|| words::disconnected(self.inner.not_connected()))?;
        if !self.view().account_records {
            return Err(words::old_hub(self.hub_name()));
        }
        let response = self
            .inner
            .ask_on(connection, Question::AccountRecords { account: account.clone(), request })
            .await?
            .ok_or(words::HUB_CLOSED)?;
        match response {
            Told::AccountRecords { response } => Ok(response),
            _ => Err(words::HUB_REPLY.into()),
        }
    }

    /// Invalidations contain metadata only. Subscribe before listing; list again
    /// after lag or reconnection. Authorization is checked by the hub on every hint.
    pub fn record_changes(&self) -> tokio::sync::broadcast::Receiver<Option<Changed>> {
        self.inner.record_changes.subscribe()
    }
}

/// A metadata-only watch. A connection failure ends the watch explicitly; opening
/// another watch obtains a fresh listing and recovers any missed invalidations.
pub struct RecordWatch {
    link: Link,
    account: AccountName,
    kind: String,
    changes: tokio::sync::broadcast::Receiver<Option<Changed>>,
    status: tokio::sync::watch::Receiver<super::Status>,
    first: bool,
}

impl Link {
    pub fn watch_account_records(&self, account: AccountName, kind: String) -> Result<RecordWatch, String> {
        fastctx_types::account_records::validate_kind(&kind)?;
        Ok(RecordWatch {
            link: self.clone(),
            account,
            kind,
            changes: self.record_changes(),
            status: self.subscribe(),
            first: true,
        })
    }
}

impl RecordWatch {
    /// Receives the initial list, later invalidations, or a fresh list after lag.
    pub async fn next(&mut self) -> Result<fastctx_types::account_records::Watched, String> {
        use fastctx_types::account_records::Watched;
        if !std::mem::take(&mut self.first) {
            loop {
                tokio::select! {
                    changed = self.changes.recv() => match changed {
                        Ok(Some(change)) if change.account == self.account && change.metadata.kind == self.kind => {
                            let view = self.link.view();
                            view.authorize(&self.account, crate::authorization::Action::ActFor(&view.me)).map_err(|error| error.to_string())?;
                            return Ok(Watched::Changed { metadata: change.metadata });
                        }
                        Ok(Some(_)) => continue,
                        Ok(None) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return Err(words::WATCH_CLOSED.into()),
                    },
                    changed = self.status.changed() => {
                        changed.map_err(|_| words::HUB_CLOSED)?;
                        self.status.borrow_and_update();
                        break;
                    }
                }
            }
        }
        match self.link.account_records(&self.account, Request::List { kind: self.kind.clone() }).await? {
            Response::Listed { entries } => Ok(Watched::Current { entries }),
            _ => Err(words::HUB_LIST.into()),
        }
    }
}
