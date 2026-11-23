//! Account collaboration uses the persisted directory view on computers and phones.

use fastctx_types::AccountName;
use fastctx_types::collab::{Collab, CollabView};

use super::Link;
use crate::wire::{Change, Question, Told};

impl Link {
    pub fn collab_read(&self, account: &AccountName) -> Result<CollabView, String> {
        let view = self.view();
        if !view.logged_in(&view.me, account) {
            return Err(format!("this device is not logged in to the account {account}"));
        }
        let found = view.account(account).ok_or_else(|| format!("there is no account {account}"))?;
        Ok(CollabView::new(found.collab.clone().unwrap_or_default(), view.collab_presets))
    }

    pub async fn collab_write(&self, account: &AccountName, collab: Collab) -> Result<u64, String> {
        collab.validate()?;
        // A hub from before presets would keep a scout as a reviewer, offer
        // a preset turned off and forget what a conversation's colleagues
        // work within together: nothing of them is sent to it.
        let presets = collab.shelved.is_some() || collab.at_once.is_some() || collab.callees.iter().any(|callee| callee.preset.is_some());
        if presets && !self.view().collab_presets {
            return Err(format!("upgrade hub {} to keep the presets of model collaboration", self.hub_name()));
        }
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
            let _publishing = self.inner.publishing_view.lock().expect("publishing view");
            let mut view = (*self.view()).clone();
            let found = view.directory.accounts.iter_mut().find(|found| found.name == *account).ok_or_else(|| {
                format!("the hub committed collaboration version {version}, but this account left the view")
            })?;
            if found.collab.as_ref().is_none_or(|current| current.version < version) {
                std::sync::Arc::make_mut(found).collab = Some(confirmed);
                // Put in the view held now under the view's publishing, so a
                // change the hub told meanwhile is not lost to this copy.
                *self.inner.view.lock().expect("never poisoned") = std::sync::Arc::new(view);
                self.inner.keep_view();
            }
        }
        self.inner.node.directory_changed();
        Ok(version)
    }
}
