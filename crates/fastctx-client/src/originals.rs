//! A file the user sent, or a picture a session showed, brought to this
//! device as it was, and kept there by its bytes when the device keeps what
//! it views: opened or saved a second time, it is not fetched again.

use std::path::PathBuf;

use fastctx_types::attachments::{AttachmentFile, AttachmentSource};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::cache::pictures;
use crate::{Error, World};

impl World {
    /// The original of `attachment` on this device: where it is kept and
    /// what it is. `None` when this World keeps nothing it views.
    pub async fn kept_original(
        &self,
        attachment: AttachmentSource,
    ) -> Result<Option<(PathBuf, AttachmentFile)>, Error> {
        let Some(cache) = self.cache().cloned() else { return Ok(None) };
        let (root, key) = (cache.root().to_path_buf(), cache.picture_key(&attachment));
        let (looking, named) = (root.clone(), key.clone());
        if let Some(found) = blocking(move || pictures::original(&looking, &named)).await? {
            return Ok(Some(found));
        }
        let mut reading = self.read_attachment(attachment.clone()).await?;
        let file = reading
            .info()
            .attachment
            .clone()
            .ok_or_else(|| Error::Unexpected("the node supplied no attachment metadata".into()))?;
        let random = fastctx_net::sign::random::<8>().map_err(Error::Kept)?;
        let incoming = root.join("originals").join(format!(".incoming-{:016x}", u64::from_le_bytes(random)));
        let Some(path) = pictures::place(&incoming, &file.name) else {
            return Err(Error::Kept(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("the name {:?} cannot name a file on this device", file.name),
            )));
        };
        let folder = path.parent().expect("an original has a folder").to_path_buf();
        let making = folder.clone();
        blocking(move || fastctx_platform::ensure_private_dir(&making)).await?;
        let mut digest = Sha256::new();
        let written = async {
            let mut out = tokio::fs::File::create(&path).await?;
            while let Some(piece) = reading.next().await.map_err(std::io::Error::other)? {
                digest.update(&piece);
                out.write_all(&piece).await?;
            }
            out.sync_all().await
        }
        .await;
        if let Err(error) = written {
            let _ = std::fs::remove_dir_all(&incoming);
            return Err(Error::Kept(error));
        }
        let sha256 = format!("{:x}", digest.finalize());
        // Bytes named by a digest that are not those bytes, such as a host's
        // own copy of a picture whose original is gone, are kept by their
        // source instead.
        let key = if key.len() == 64 && key != sha256 { cache.source_key(&attachment) } else { key };
        blocking(move || {
            let target = root.join("originals").join(&key);
            match std::fs::rename(&folder, &target) {
                Ok(()) => {}
                // Fetched meanwhile by another viewer on this device.
                Err(_) if target.join(".file.json").try_exists()? => {
                    std::fs::remove_dir_all(&incoming)?;
                    return pictures::original(&root, &key)?
                        .ok_or_else(|| std::io::Error::other("a kept original went away while it was kept again"));
                }
                Err(error) => return Err(error),
            }
            let path = target.join(&file.name);
            pictures::kept_original(&path, &file)?;
            Ok((path, file))
        })
        .await
        .map(Some)
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Kept(std::io::Error::other(error)))?
        .map_err(Error::Kept)
}
