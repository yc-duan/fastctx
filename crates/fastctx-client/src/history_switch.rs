//! History opt-in through a local node or a direct device connection.

use crate::{Error, World};
use fastctx_types::{AccountName, Call, HubName, Reply, history_switch::HistorySwitch};

impl World {
    pub async fn history_switch_read(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
    ) -> Result<HistorySwitch, Error> {
        match self.ask(Call::HistorySwitchRead { hub, account }).await? {
            Reply::HistorySwitch(setting) => Ok(setting),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    pub async fn history_switch_write(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        on: bool,
    ) -> Result<(), Error> {
        match self.ask(Call::HistorySwitchWrite { hub, account, on }).await? {
            Reply::Done => Ok(()),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
