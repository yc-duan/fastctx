//! The account's notes setting, shared by computers and devices.

use super::Link;
use crate::wire::{Change, Question, Told};
use fastctx_types::{
    AccountName,
    history_switch::{HUB_TOO_OLD, HistorySwitch},
};

impl Link {
    pub fn history_switch_read(&self, account: &AccountName) -> Result<HistorySwitch, String> {
        let view = self.view();
        if !view.logged_in(&view.me, account) {
            return Err(format!("this device is not logged in to the account {account}"));
        }
        let found = view.account(account).ok_or_else(|| format!("there is no account {account}"))?;
        Ok(HistorySwitch { on: view.history_switch && found.history })
    }

    /// Writes one boolean, without replacing any other account settings.
    pub async fn history_switch_write(&self, account: &AccountName, on: bool) -> Result<(), String> {
        if !self.view().history_switch {
            return Err(HUB_TOO_OLD.into());
        }
        self.history_switch_read(account)?;
        let told =
            self.inner.ask(Question::Manage { account: account.clone(), change: Change::History { on } }).await?;
        if !matches!(told, Told::Done) {
            return Err("the hub did not confirm the notes setting".into());
        }
        // Read the authoritative view: another writer's later choice wins.
        self.refresh().await?;
        self.inner.node.directory_changed();
        Ok(())
    }
}
