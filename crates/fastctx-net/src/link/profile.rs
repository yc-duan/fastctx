//! An account's configuration, available only where work for it is admitted.

use fastctx_types::AccountName;
use fastctx_types::mcp::ProfileView;
use fastctx_types::profile::Profile;

use super::Link;
use crate::wire::{Change, Question, Told};

impl Link {
    pub fn profile_read(&self, account: &AccountName) -> Result<ProfileView, String> {
        let view = self.view();
        if !view.may_act_for(account, &view.me) {
            return Err(format!("this device does not work for account {account}"));
        }
        let found = view.account(account).ok_or_else(|| format!("there is no account {account}"))?;
        Ok(ProfileView {
            hub: self.hub_name(),
            account: account.clone(),
            profile: found.profile.clone().unwrap_or_default(),
            kept: view.profiles,
        })
    }

    pub async fn profile_write(&self, account: &AccountName, profile: Profile) -> Result<u64, String> {
        let current = self.profile_read(account)?;
        if !current.kept {
            return Err(format!("upgrade hub {} to use user-level configuration", self.hub_name()));
        }
        profile.validate()?;
        let mut confirmed = profile.clone();
        let told =
            self.inner.ask(Question::Manage { account: account.clone(), change: Change::Profile { profile } }).await?;
        let Told::ProfileWritten { version } = told else {
            return Err("the hub answered the profile write with something else".into());
        };
        confirmed.version = version;
        {
            let _one = self.inner.refreshing.lock().await;
            let mut view = (*self.view()).clone();
            if !view.may_act_for(account, &view.me) {
                return Err(format!(
                    "the hub committed profile version {version}, but this device no longer works for {account}"
                ));
            }
            let found =
                view.directory.accounts.iter_mut().find(|found| found.name == *account).ok_or_else(|| {
                    format!("the hub committed profile version {version}, but the account left the view")
                })?;
            if found.profile.as_ref().is_none_or(|current| current.version < version) {
                found.profile = Some(confirmed);
                super::state::write(&self.inner.dir.view(), &view).map_err(|error| {
                    format!("the hub committed profile version {version}, but keeping its view failed: {error}")
                })?;
                *self.inner.view.lock().expect("never poisoned") = std::sync::Arc::new(view);
            }
        }
        self.inner.node.directory_changed();
        Ok(version)
    }
}
