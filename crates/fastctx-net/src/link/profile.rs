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

    pub async fn profile_write(&self, account: &AccountName, mut profile: Profile) -> Result<u64, String> {
        let current = self.profile_read(account)?;
        if !current.kept {
            return Err(format!("upgrade hub {} to use user-level configuration", self.hub_name()));
        }
        profile.subagents = current.profile.subagents;
        profile.memory = current.profile.memory;
        profile.layer = current.profile.layer;
        profile.validate()?;
        let confirmed = profile.clone();
        let told =
            self.inner.ask(Question::Manage { account: account.clone(), change: Change::Profile { profile } }).await?;
        let Told::ProfileWritten { version } = told else {
            return Err("the hub answered the profile write with something else".into());
        };
        self.confirm_profile(account, confirmed, version).await?;
        Ok(version)
    }

    pub(super) async fn confirm_profile(
        &self,
        account: &AccountName,
        mut confirmed: Profile,
        version: u64,
    ) -> Result<(), String> {
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
                std::sync::Arc::make_mut(found).profile = Some(confirmed);
                super::state::write(&self.inner.dir.view(), &view).map_err(|error| {
                    format!("the hub committed profile version {version}, but keeping its view failed: {error}")
                })?;
                *self.inner.view.lock().expect("never poisoned") = std::sync::Arc::new(view);
            }
        }
        self.inner.node.directory_changed();
        Ok(())
    }
    pub async fn profile_subagents(
        &self,
        account: &AccountName,
        operation: fastctx_types::profile::SubagentOperation,
    ) -> Result<fastctx_types::profile::SubagentView, String> {
        use fastctx_types::profile::{SubagentOperation, SubagentView};
        let current = self.profile_read(account)?;
        let mut configuration = current.profile.subagents.clone().unwrap_or_default();
        if let Some(version) = operation.version() {
            if !self.view().subagents {
                return Err(format!("upgrade hub {} to configure Claude subagents", self.hub_name()));
            }
            if version != current.profile.version {
                return Err(format!(
                    "the profile changed: version {version} was read, current version is {}",
                    current.profile.version
                ));
            }
            configuration.change(&operation)?;
            let subagents = (!matches!(operation, SubagentOperation::Reset { .. })).then_some(configuration);
            let told = self
                .inner
                .ask(Question::Manage {
                    account: account.clone(),
                    change: Change::Subagents { version, subagents: subagents.clone() },
                })
                .await?;
            let Told::ProfileWritten { version } = told else {
                return Err("the hub did not confirm the subagent change".into());
            };
            let mut profile = current.profile;
            profile.subagents = subagents;
            self.confirm_profile(account, profile, version).await?;
        }
        let current = self.profile_read(account)?;
        Ok(SubagentView {
            hub: current.hub,
            account: current.account,
            supported: self.view().subagents,
            version: current.profile.version,
            configured: current.profile.subagents.is_some(),
            configuration: current.profile.subagents.unwrap_or_default(),
        })
    }
}
