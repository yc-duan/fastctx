//! The user's configuration through a node or a device connection.

use crate::{Error, World};
use fastctx_types::mcp::{McpCall, McpReply, ProfileView};
use fastctx_types::profile::Profile;
use fastctx_types::{AccountName, Call, HubName, Reply};

impl World {
    pub async fn profile_subagents(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        operation: fastctx_types::profile::SubagentOperation,
    ) -> Result<fastctx_types::profile::SubagentView, Error> {
        match self.ask(Call::ProfileSubagents { hub, account, operation }).await? {
            Reply::ProfileSubagents(view) => Ok(*view),
            _ => Err(Error::Unexpected("the account's Claude subagents".into())),
        }
    }
    pub async fn profile_read(&self, hub: Option<HubName>, account: Option<AccountName>) -> Result<ProfileView, Error> {
        match self.ask(Call::ProfileRead { hub, account }).await? {
            Reply::Profile(view) => Ok(*view),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
    pub async fn profile_write(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        profile: Profile,
    ) -> Result<u64, Error> {
        match self.ask(Call::ProfileWrite { hub, account, profile }).await? {
            Reply::ProfileWritten { version } => Ok(version),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
    pub async fn mcp(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        mcp: McpCall,
    ) -> Result<McpReply, Error> {
        match self.ask(Call::Mcp { hub, account, mcp }).await? {
            Reply::Mcp { result: reply } => Ok(reply),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
