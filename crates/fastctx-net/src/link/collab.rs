//! Account collaboration uses the persisted directory view on computers and phones.

use fastctx_types::AccountName;
use fastctx_types::collab::{Collab, CollabView};
use fastctx_types::collab_prompt::DEFAULT_PROMPT;

use super::Link;
use crate::wire::{Change, Question, Told};

impl Link {
    pub fn collab_read(&self, account: &AccountName) -> Result<CollabView, String> {
        let view = self.view();
        if !view.logged_in(&view.me, account) {
            return Err(format!("this device is not logged in to the account {account}"));
        }
        let found = view.account(account).ok_or_else(|| format!("there is no account {account}"))?;
        Ok(CollabView { collab: found.collab.clone().unwrap_or_default(), default_prompt: DEFAULT_PROMPT.to_string() })
    }

    pub async fn collab_write(&self, account: &AccountName, collab: Collab) -> Result<u64, String> {
        collab.validate()?;
        let mut confirmed = collab.clone();
        let told =
            self.inner.ask(Question::Manage { account: account.clone(), change: Change::Collab { collab } }).await?;
        let Told::CollabWritten { version } = told else {
            return Err("the hub answered model collaboration with something else".to_string());
        };
        // Keep the hub's acknowledged setting before returning it. A concurrent
        // view refresh may already hold a newer version, which must win.
        confirmed.version = version;
        {
            let _one = self.inner.refreshing.lock().await;
            let mut view = (*self.view()).clone();
            let found = view.directory.accounts.iter_mut().find(|found| found.name == *account).ok_or_else(|| {
                format!("the hub committed collaboration version {version}, but this account left the view")
            })?;
            if found.collab.as_ref().is_none_or(|current| current.version < version) {
                std::sync::Arc::make_mut(found).collab = Some(confirmed);
                super::state::write(&self.inner.dir.view(), &view).map_err(|error| {
                    format!("the hub committed collaboration version {version}, but keeping its view failed: {error}")
                })?;
                *self.inner.view.lock().expect("never poisoned") = std::sync::Arc::new(view);
            }
        }
        self.inner.node.directory_changed();
        Ok(version)
    }
}
