use fastctx_types::access::Written;
use fastctx_types::attachments::AttachmentSource;
use fastctx_types::thumbnail::{Thumbnail, ThumbnailRequest};
use fastctx_types::{Call, Reply};

use crate::cache::pictures;
use crate::{Error, World};

impl World {
    /// Read an upload by its server receipt, including one of an agent
    /// assisting from a host the account may not read arbitrary files on. A device
    /// that keeps what it views asks for each size of a picture once.
    pub async fn thumbnail_attachment(
        &self,
        attachment: impl Into<AttachmentSource>,
        max: u32,
    ) -> Result<Thumbnail, Error> {
        let attachment = attachment.into();
        let uploaded = matches!(attachment, AttachmentSource::Upload(_));
        let cache = self.cache().cloned();
        let kept = cache.as_ref().map(|cache| (cache.root().to_path_buf(), cache.picture_key(&attachment)));
        let mut requested = None;
        let mut generation = 0;
        let fallback = cache.as_ref().map(|cache| cache.source_key(&attachment));
        if let (Some(cache), Some((root, key))) = (cache.clone(), kept.clone()) {
            let fallback = fallback.clone();
            let (request, current, found) = tokio::task::spawn_blocking(move || {
                let request = cache.request(&format!("thumbnail:{key}:{max}"))?;
                let turn = cache.turn()?;
                let found = pictures::thumbnail(&root, &key, max)?;
                let found = if found.is_none() && uploaded {
                    pictures::thumbnail(&root, fallback.as_deref().expect("a source key"), max)?
                } else {
                    found
                };
                Ok::<_, std::io::Error>((request, turn.generation(), found))
            })
            .await
            .map_err(|error| Error::Kept(std::io::Error::other(error)))?
            .map_err(Error::Kept)?;
            requested = Some(request);
            generation = current;
            if let Some(thumbnail) = found {
                return Ok(thumbnail);
            }
        }
        let thumbnail = match self.call(Call::AttachmentThumbnail { attachment, max }).await? {
            (Reply::Thumbnail(thumbnail), _) => thumbnail,
            _ => return Err(Error::Unexpected("attachment thumbnail".into())),
        };
        if let (Some(cache), Some((root, mut key))) = (cache, kept) {
            if key.len() == 64 {
                match &thumbnail.content {
                    Some(content) if content.sha256 != key && uploaded => key = fallback.expect("a source key"),
                    Some(content) if content.sha256 != key => {
                        return Err(Error::Other("the image changed since its history was read".into()));
                    }
                    None if uploaded => key = fallback.expect("a source key"),
                    None if !uploaded => {
                        return Err(Error::Other(
                            "this machine does not identify the thumbnail's original content".into(),
                        ));
                    }
                    _ => {}
                }
            }
            let keeping = thumbnail.clone();
            tokio::task::spawn_blocking(move || {
                let _request = requested;
                let turn = cache.turn()?;
                if turn.generation() != generation {
                    return Ok(());
                }
                pictures::keep_thumbnail(&root, &key, max, &keeping)
            })
            .await
            .map_err(|error| Error::Kept(std::io::Error::other(error)))?
            .map_err(Error::Kept)?;
        }
        Ok(thumbnail)
    }

    pub async fn thumbnail(&self, machine: Option<Written>, request: ThumbnailRequest) -> Result<Thumbnail, Error> {
        match self.call(Call::Thumbnail { machine, request }).await? {
            (Reply::Thumbnail(thumbnail), _) => Ok(thumbnail),
            _ => Err(Error::Unexpected("thumbnail".into())),
        }
    }
}
