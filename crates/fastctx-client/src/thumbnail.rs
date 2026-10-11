use fastctx_types::thumbnail::{Thumbnail, ThumbnailRequest};
use fastctx_types::{Call, MachineName, Reply};

use crate::{Error, World};

impl World {
    /// Read an upload by its server receipt, including an agent host lent
    /// without permission to read arbitrary files on that host.
    pub async fn thumbnail_attachment(
        &self,
        attachment: impl Into<fastctx_types::attachments::AttachmentSource>,
        max: u32,
    ) -> Result<Thumbnail, Error> {
        match self.call(Call::AttachmentThumbnail { attachment: attachment.into(), max }).await? {
            (Reply::Thumbnail(thumbnail), _) => Ok(thumbnail),
            _ => Err(Error::Unexpected("attachment thumbnail".into())),
        }
    }
    pub async fn thumbnail(&self, machine: Option<MachineName>, request: ThumbnailRequest) -> Result<Thumbnail, Error> {
        match self.call(Call::Thumbnail { machine, request }).await? {
            (Reply::Thumbnail(thumbnail), _) => Ok(thumbnail),
            _ => Err(Error::Unexpected("thumbnail".into())),
        }
    }
}
