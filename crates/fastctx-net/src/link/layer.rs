//! The account's layer at the hub (`world-layers.md`).

use fastctx_types::AccountName;
use fastctx_types::layers::{Chosen, HUB_TOO_OLD, Layer};

use crate::wire::{Change, Question, Told};

use super::Link;

impl Link {
    /// The layer `account` chose, as this member's view has it.
    pub fn layer(&self, account: &AccountName) -> Result<Chosen, String> {
        Ok(Chosen { layer: self.profile_read(account)?.profile.layer })
    }

    /// Changes only the account's layer, over the profile version read.
    pub async fn set_layer(&self, account: &AccountName, layer: Layer) -> Result<Chosen, String> {
        if !self.view().layers {
            return Err(HUB_TOO_OLD.into());
        }
        self.refresh().await?;
        let mut profile = self.profile_read(account)?.profile;
        let answer = self
            .inner
            .ask(Question::Manage {
                account: account.clone(),
                change: Change::Layer { version: profile.version, layer },
            })
            .await?;
        let Told::ProfileWritten { version } = answer else {
            return Err("the hub did not confirm the layer".into());
        };
        profile.layer = Some(layer);
        self.confirm_profile(account, profile, version).await?;
        self.layer(account)
    }
}
