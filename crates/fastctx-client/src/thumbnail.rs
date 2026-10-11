use fastctx_types::attachments::AttachmentSource;
use fastctx_types::thumbnail::{Thumbnail, ThumbnailRequest};
use fastctx_types::{Call, MachineName, Reply};

use crate::cache::pictures;
use crate::{Error, World};

impl World {
    /// Read an upload by its server receipt, including an agent host lent
    /// without permission to read arbitrary files on that host. A device
    /// that keeps what it views asks for each size of a picture once.
    pub async fn thumbnail_attachment(
        &self,
        attachment: impl Into<AttachmentSource>,
        max: u32,
    ) -> Result<Thumbnail, Error> {
        let attachment = attachment.into();
        let kept = self.cache().map(|cache| (cache.root().to_path_buf(), cache.picture_key(&attachment)));
        if let Some((root, key)) = kept.clone() {
            let found = tokio::task::spawn_blocking(move || pictures::thumbnail(&root, &key, max))
                .await
                .map_err(|error| Error::Kept(std::io::Error::other(error)))?
                .map_err(Error::Kept)?;
            if let Some(thumbnail) = found {
                return Ok(thumbnail);
            }
        }
        let thumbnail = match self.call(Call::AttachmentThumbnail { attachment, max }).await? {
            (Reply::Thumbnail(thumbnail), _) => thumbnail,
            _ => return Err(Error::Unexpected("attachment thumbnail".into())),
        };
        if let Some((root, key)) = kept {
            let keeping = thumbnail.clone();
            tokio::task::spawn_blocking(move || pictures::keep_thumbnail(&root, &key, max, &keeping))
                .await
                .map_err(|error| Error::Kept(std::io::Error::other(error)))?
                .map_err(Error::Kept)?;
        }
        Ok(thumbnail)
    }

    pub async fn thumbnail(&self, machine: Option<MachineName>, request: ThumbnailRequest) -> Result<Thumbnail, Error> {
        match self.call(Call::Thumbnail { machine, request }).await? {
            (Reply::Thumbnail(thumbnail), _) => Ok(thumbnail),
            _ => Err(Error::Unexpected("thumbnail".into())),
        }
    }
}
