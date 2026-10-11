//! Reads the hub's history on computers and devices.

use fastctx_types::AccountName;
use fastctx_types::machine_log::Page;

use super::Link;
use crate::wire::{Question, Told};

impl Link {
    pub async fn machine_log(&self, account: &AccountName, before: Option<u64>) -> Result<Page, String> {
        // Refresh before checking support: a cached view may predate a hub upgrade.
        self.refresh().await?;
        if !self.view().machine_log {
            return Err("this hub does not support the machine change log".to_string());
        }
        match self.inner.ask(Question::MachineLog { account: account.clone(), before }).await? {
            Told::MachineLog { page } => Ok(page),
            _ => Err("the hub answered the machine log with something else".to_string()),
        }
    }
}
