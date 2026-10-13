//! A file the user sent, or a picture a session showed, brought to this
//! device as it was, and kept there by its bytes when the device keeps what
//! it views: opened or saved a second time, it is not fetched again.

use std::path::PathBuf;

use fastctx_types::attachments::{AttachmentFile, AttachmentSource};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::cache::pictures;
use crate::file_read::Read;
use crate::{Error, World};

impl World {
    /// The original of `attachment` on this device: where it is kept and
    /// what it is. `None` when this World keeps nothing it views. When it
    /// is read now, `told` is given how the read goes.
    pub async fn kept_original(
        &self,
        attachment: AttachmentSource,
        told: impl FnOnce(tokio::sync::watch::Receiver<crate::file_read::Progress>),
    ) -> Result<Option<(PathBuf, AttachmentFile)>, Error> {
        let Some(cache) = self.cache().cloned() else { return Ok(None) };
        let (root, key) = (cache.root().to_path_buf(), cache.picture_key(&attachment));
        let (looking, named) = (root.clone(), key.clone());
        let fallback = cache.source_key(&attachment);
        let locked = cache.clone();
        let (request, generation, found) = blocking(move || {
            let request = locked.request(&format!("original:{named}"))?;
            let turn = locked.turn()?;
            let found = pictures::original(&looking, &named)?;
            let found = if found.is_none() { pictures::original(&looking, &fallback)? } else { found };
            Ok((request, turn.generation(), found))
        })
        .await?;
        if let Some(found) = found {
            return Ok(Some(found));
        }
        let mut reading = self.read_attachment(attachment.clone());
        told(reading.progress());
        let random = fastctx_net::sign::random::<8>().map_err(Error::Kept)?;
        let incoming = root.join(".incoming").join(format!("original-{:016x}", u64::from_le_bytes(random)));
        let making = incoming.clone();
        blocking(move || fastctx_platform::ensure_private_dir(&making)).await?;
        let pending = Pending(incoming.clone());
        // The file being written, named as the read opened it. A read that
        // starts over may open another picture (its original went away
        // meanwhile): what was written goes, and its name with it.
        let mut kept: Option<(PathBuf, AttachmentFile, tokio::fs::File)> = None;
        let mut digest = Sha256::new();
        while let Some(read) = reading.next().await? {
            match read {
                Read::Opened(info) => {
                    let file = info
                        .attachment
                        .ok_or_else(|| Error::Unexpected("the node supplied no attachment metadata".into()))?;
                    let Some(path) = pictures::place(&incoming, &file.name) else {
                        return Err(Error::Kept(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("the name {:?} cannot name a file on this device", file.name),
                        )));
                    };
                    if let Some((before, _, out)) = kept.take() {
                        drop(out);
                        tokio::fs::remove_file(&before).await.map_err(Error::Kept)?;
                    }
                    let out = tokio::fs::File::create(&path).await.map_err(Error::Kept)?;
                    digest = Sha256::new();
                    kept = Some((path, file, out));
                }
                Read::Bytes(piece) => {
                    let (_, _, out) = kept.as_mut().expect("a read hands out bytes only once it opened");
                    digest.update(&piece);
                    out.write_all(&piece).await.map_err(Error::Kept)?;
                }
            }
        }
        let (path, file, out) = kept.expect("a read that ended well opened");
        out.sync_all().await.map_err(Error::Kept)?;
        drop(out);
        let folder = incoming.clone();
        let sha256 = format!("{:x}", digest.finalize());
        // Bytes named by a digest that are not those bytes, such as a host's
        // own copy of a picture whose original is gone, are kept by their
        // source instead.
        let key = if key.len() == 64 && key != sha256 {
            if file.source == fastctx_types::attachments::AttachmentFileSource::Original {
                return Err(Error::Other("the original changed since its history was read".into()));
            }
            cache.source_key(&attachment)
        } else {
            key
        };
        blocking(move || {
            let _pending = pending;
            let _request = request;
            let turn = cache.turn()?;
            if turn.generation() != generation {
                std::fs::remove_dir_all(&incoming)?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "the viewed cache was cleared during the original read",
                ));
            }
            let target = root.join("originals").join(&key);
            fastctx_platform::ensure_private_dir(&root.join("originals"))?;
            pictures::kept_original(&path, &file)?;
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
            Ok((path, file))
        })
        .await
        .map(Some)
    }
}

struct Pending(PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("discarding an unfinished original cache: {error}");
        }
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Kept(std::io::Error::other(error)))?
        .map_err(Error::Kept)
}
