//! Pictures this device was shown, and files it opened, kept by their bytes
//! where those are known: never fetched twice, even when another
//! conversation shows the same picture. A picture's bytes are known from the
//! history it came in (`UserImage::content`, `Content::ImageRef`, an upload's
//! own digest); one named by nothing else is kept by its source.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, publish};
use fastctx_types::attachments::{AttachmentFile, AttachmentSource};
use fastctx_types::session::{Content, Happened, SessionEvent, What};
use fastctx_types::thumbnail::Thumbnail;
use serde::{Deserialize, Serialize};

/// The bytes of the pictures the histories handed out show, by source.
#[derive(Default)]
pub(crate) struct Pictures {
    prints: HashMap<String, String>,
}

impl Pictures {
    pub fn clear(&mut self) {
        self.prints.clear();
    }

    /// Lets go of what it knows of the pictures `gone`, no longer kept.
    pub fn forget(&mut self, gone: &HashSet<String>) {
        self.prints.retain(|_, print| !gone.contains(print));
    }

    /// Learns what the pictures `events` show are; returns their keys.
    pub fn learn(&mut self, events: &[SessionEvent]) -> Vec<String> {
        let mut keys = Vec::new();
        for event in events {
            match &event.happened {
                Happened::Heard { message: Some(message), .. } => {
                    for attachment in &message.attachments {
                        keys.push(source_key(&AttachmentSource::Upload(attachment.clone())));
                        keys.push(self.know(&AttachmentSource::Upload(attachment.clone()), &attachment.file.sha256));
                    }
                    for image in &message.images {
                        keys.push(source_key(&image.source));
                        keys.push(match &image.content {
                            Some(print) => self.know(&image.source, &print.sha256),
                            None => source_key(&image.source),
                        });
                    }
                }
                Happened::Began { item, .. } | Happened::Finished { item, .. } | Happened::Updated { item, .. } => {
                    if let What::Tool(tool) = &item.what {
                        for content in &tool.output {
                            if let Content::ImageRef { content, source, .. } = content {
                                keys.push(source_key(source));
                                keys.push(self.know(source, &content.sha256));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    fn know(&mut self, source: &AttachmentSource, sha256: &str) -> String {
        if !is_digest(sha256) {
            return source_key(source);
        }
        self.prints.insert(source_text(source), sha256.to_string());
        sha256.to_string()
    }

    /// What `source` is kept by: its bytes, where they are known.
    pub fn key(&self, source: &AttachmentSource) -> String {
        if let AttachmentSource::Upload(attachment) = source
            && is_digest(&attachment.file.sha256)
        {
            return attachment.file.sha256.clone();
        }
        self.prints.get(&source_text(source)).cloned().unwrap_or_else(|| source_key(source))
    }
}

fn source_text(source: &AttachmentSource) -> String {
    serde_json::to_string(source).expect("sources serialize")
}

pub(crate) fn source_key(source: &AttachmentSource) -> String {
    format!("s{}", super::key(&source_text(source)))
}

fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Serialize, Deserialize)]
struct Shape {
    #[serde(default)]
    content: Option<fastctx_types::session::Fingerprint>,
    width: u32,
    height: u32,
    mime: String,
}

fn thumbnail_path(root: &Path, key: &str, max: u32) -> PathBuf {
    root.join("pictures").join(format!("{key}-{max}.thumb"))
}

/// A picture kept at `max` pixels on its longer side.
pub(crate) fn thumbnail(root: &Path, key: &str, max: u32) -> io::Result<Option<Thumbnail>> {
    let bytes = match std::fs::read(thumbnail_path(root, key, max)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let damaged = || io::Error::new(io::ErrorKind::InvalidData, format!("the kept picture {key}-{max} is damaged"));
    let split = bytes.iter().position(|byte| *byte == b'\n').ok_or_else(damaged)?;
    let shape: Shape = serde_json::from_slice(&bytes[..split]).map_err(|_| damaged())?;
    use base64::Engine as _;
    Ok(Some(Thumbnail {
        content: shape.content,
        width: shape.width,
        height: shape.height,
        mime: shape.mime,
        base64: base64::engine::general_purpose::STANDARD.encode(&bytes[split + 1..]),
    }))
}

/// Keeps a picture made at `max` pixels.
pub(crate) fn keep_thumbnail(root: &Path, key: &str, max: u32, thumbnail: &Thumbnail) -> io::Result<()> {
    use base64::Engine as _;
    let picture = base64::engine::general_purpose::STANDARD
        .decode(&thumbnail.base64)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("the picture sent: {error}")))?;
    let shape = Shape {
        content: thumbnail.content.clone(),
        width: thumbnail.width,
        height: thumbnail.height,
        mime: thumbnail.mime.clone(),
    };
    let mut bytes = serde_json::to_vec(&shape)?;
    bytes.push(b'\n');
    bytes.write_all(&picture)?;
    let path = thumbnail_path(root, key, max);
    fastctx_platform::ensure_private_dir(path.parent().expect("pictures have a folder"))?;
    publish(&path, &bytes, Permissions::Private)
}

/// A file kept as it was sent: where it is and what it is.
pub(crate) fn original(root: &Path, key: &str) -> io::Result<Option<(PathBuf, AttachmentFile)>> {
    let folder = root.join("originals").join(key);
    let file: AttachmentFile = match std::fs::read(folder.join(".file.json")) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let path = place(&folder, &file.name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the kept original's name is not a file name"))?;
    Ok(path.try_exists()?.then_some((path, file)))
}

/// Where a file named `name` goes in `folder`; `None` when the name cannot
/// name a file there on its own.
pub(crate) fn place(folder: &Path, name: &str) -> Option<PathBuf> {
    let path = folder.join(name);
    let plain = path.parent() == Some(folder) && !name.starts_with('.') && !(cfg!(windows) && name.contains(':'));
    plain.then_some(path)
}

/// Records what the file just written at `path` is, so it is found again.
pub(crate) fn kept_original(path: &Path, file: &AttachmentFile) -> io::Result<()> {
    publish(
        &path.parent().expect("an original has a folder").join(".file.json"),
        &serde_json::to_vec(file)?,
        Permissions::Private,
    )
}

/// Lets go of the pictures and files kept under `keys`.
pub(crate) fn forget(root: &Path, keys: &HashSet<String>) -> io::Result<()> {
    if keys.iter().any(|key| {
        !(is_digest(key)
            || key.strip_prefix('s').is_some_and(|key| {
                key.len() == 32 && key.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            }))
    }) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the cache index contains an invalid picture key"));
    }
    if keys.is_empty() {
        return Ok(());
    }
    match std::fs::read_dir(root.join("pictures")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.rsplit_once('-').is_some_and(|(key, _)| keys.contains(key)) {
                    remove(&entry.path(), false)?;
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for key in keys {
        remove(&root.join("originals").join(key), true)?;
    }
    Ok(())
}

fn remove(path: &Path, folder: bool) -> io::Result<()> {
    let removed = if folder { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    match removed {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
