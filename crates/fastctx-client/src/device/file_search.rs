use super::{Device, failed};
use crate::Error;
use fastctx_types::access::Written;
use fastctx_types::file_search::{FileSearchReply, FileSearchScope};

impl Device {
    pub(crate) async fn open_file_search(
        &self,
        machine: Option<Written>,
        scope: FileSearchScope,
    ) -> Result<(FileSearchReply, fastctx_net::file_search::Lease), Error> {
        let machine = machine.ok_or_else(|| failed("choose a machine for file search; a device has no local files"))?;
        let session = self.reaching(&machine)?;
        let machine = machine.name;
        let account = session.account_for(&machine, fastctx_types::access::Purpose::Hands)?;
        fastctx_net::file_search::open(session.link.clone(), machine, account, scope)
            .await
            .map_err(|error| crate::CallError::Failed(error).into())
    }
}
