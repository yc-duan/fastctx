//! Sentinel controls through the same World interface on desktops and devices.
use crate::{Error, World};
use fastctx_types::{
    AccountName, Call, HubName, Reply,
    sentinel_settings::{SettingsReply, WriteSettings},
};
impl World {
    pub async fn sentinel_settings(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        change: Option<WriteSettings>,
    ) -> Result<SettingsReply, Error> {
        match self.ask(Call::SentinelSettings { hub, account, change }).await? {
            Reply::SentinelSettings(reply) => Ok(*reply),
            _ => Err(Error::Unexpected("sentinel settings".into())),
        }
    }
}

impl World {
    pub async fn sentinel(&self,hub:Option<HubName>,account:Option<AccountName>,request:fastctx_types::sentinel_control::Request)->Result<fastctx_types::sentinel_control::Reply,Error> {
        match self.ask(Call::Sentinel{hub,account,request}).await? {
            Reply::Sentinel(reply)=>Ok(*reply),_=>Err(Error::Unexpected("sentinel reply".into())),
        }
    }
}
