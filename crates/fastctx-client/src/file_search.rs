use fastctx_types::file_search::{FileSearchReply, FileSearchRequest};
use fastctx_types::{Call, MachineName, Reply};

use crate::{Error, World};

impl World {
    /// Start for the conversation's composer on `machine`, before its first
    /// `@`. Query on edits and while indexing, retain across popovers, and
    /// close when leaving the composer. Use the returned search
    /// id only with the same machine and World identity. Devices must name
    /// a machine; that machine checks their remote permission.
    pub async fn find_files(
        &self,
        machine: Option<MachineName>,
        request: FileSearchRequest,
    ) -> Result<FileSearchReply, Error> {
        match self.call(Call::FindFiles { machine, request }).await?.0 {
            Reply::FilesFound(reply) => Ok(reply),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
