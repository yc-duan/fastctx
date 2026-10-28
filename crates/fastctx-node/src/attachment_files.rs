//! Resolve a receipt first, then the image its own resident's host kept.

use fastctx_types::attachments::{AttachmentFile, AttachmentFileSource, AttachmentSource, HostImage, ToolImage};
use fastctx_types::ipc::{Failure, FailureCode};
use fastctx_types::session::attachments::Envelope;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::io::{self, BufReader, Read as _};
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

mod record_read;

static READING: std::sync::LazyLock<crate::ranked::Pictures> = std::sync::LazyLock::new(Default::default);

pub(crate) struct File {
    pub path: PathBuf,
    pub snapshot: fastctx_platform::file_read::Snapshot,
    pub description: AttachmentFile,
    /// The SHA-256 of a picture decoded anew for this read, which names
    /// its bytes where the file it was written to cannot (`FileReadInfo`).
    pub version: Option<String>,
    _temporary: Option<tempfile::TempDir>,
}

fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) async fn resolve(
    node: &Arc<crate::Node>,
    source: AttachmentSource,
    cancel: Arc<AtomicBool>,
) -> Result<File, Failure> {
    if !node.is_me(&source.owner().machine) {
        return Err(Failure::not_run("the attachment names another holding machine"));
    }
    if let AttachmentSource::Upload(attachment) = &source {
        let is_image = attachment.is_image();
        let (node, attachment, cancelled) = (node.clone(), attachment.clone(), cancel.clone());
        match tokio::task::spawn_blocking(move || crate::attachments::verified_original(&node, &attachment, &cancelled))
            .await
            .map_err(failed)?
        {
            Ok((path, snapshot)) => {
                return Ok(File {
                    path: PathBuf::from(path),
                    snapshot,
                    description: AttachmentFile {
                        name: source_name(&source),
                        media_type: attachment_media(&source),
                        source: AttachmentFileSource::Original,
                    },
                    version: None,
                    _temporary: None,
                });
            }
            Err(error) if error.code == Some(FailureCode::OriginalGone) => {}
            Err(error) => return Err(error),
        }
        if !is_image {
            return Err(Failure::original_gone());
        }
    }
    let (about, _) = node.residents.about(&source.owner().resident).ok_or_else(|| match source {
        AttachmentSource::Upload(_) => Failure::original_gone(),
        _ => Failure::not_run("the attachment's resident is unknown"),
    })?;
    if let AttachmentSource::HostImage { host_image, .. } = &source {
        let host = match host_image {
            HostImage::Codex { .. } => "codex",
            HostImage::Claude { .. } => "claude",
        };
        if about.host.agent.as_str() != host {
            return Err(Failure::new("the image selector names a different host"));
        }
    }
    // Codex lists a turn's items: the tool's own is read from there.
    if let AttachmentSource::ToolImage { tool_image, .. } = &source
        && about.host.agent.as_str() == "codex"
    {
        let thread = about.session.as_deref().ok_or_else(Failure::original_gone)?;
        let turn = tool_image.turn.as_deref().ok_or_else(Failure::original_gone)?;
        let (mime, base64) = node
            .codex
            .of(node, &about)?
            .tool_image(thread, turn, &tool_image.item, tool_image.image)
            .await
            .map_err(failed)?
            .ok_or_else(Failure::original_gone)?;
        let temporary = node.world.path().join("image-reading");
        return tokio::task::spawn_blocking(move || decoded(&temporary, &mime, &base64).map_err(failed))
            .await
            .map_err(failed)?;
    }
    if let AttachmentSource::HostImage { host_image: HostImage::Codex { turn, message, image }, .. } = &source {
        let thread = about.session.as_deref().ok_or_else(Failure::original_gone)?;
        if let Some(picture) =
            node.codex.of(node, &about)?.user_picture(thread, turn, *message, *image).await.map_err(failed)?
        {
            let picture = picture.ok_or_else(Failure::original_gone)?;
            let temporary = node.world.path().join("image-reading");
            let cancel = cancel.clone();
            let resolved = tokio::task::spawn_blocking(move || match picture {
                fastctx_hosts::codex::UserPicture::Local(path) => match local_image(path, &cancel) {
                    Ok(file) => Ok(Some(file)),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(failed(error)),
                },
                fastctx_hosts::codex::UserPicture::Inline { mime, base64 } => {
                    decoded(&temporary, &mime, &base64).map(Some).map_err(failed)
                }
            })
            .await
            .map_err(failed)??;
            if let Some(file) = resolved {
                return Ok(file);
            }
        }
    }
    let paths = match about.host.agent.as_str() {
        "codex" => {
            let thread = about.session.as_deref().ok_or_else(Failure::original_gone)?;
            let host = node.codex.of(node, &about)?;
            let path = host.rollout(thread).await.map_err(failed)?.ok_or_else(Failure::original_gone)?;
            vec![path]
        }
        "claude" => crate::claude::image_records(node, &about.id)?,
        _ => return Err(Failure::new("the resident's host has no image-record reader")),
    };
    let temporary = node.world.image_reading();
    let agent = about.host.agent.clone();
    let serial = READING.lock_owned().await;
    tokio::task::spawn_blocking(move || {
        let _serial = serial;
        fastctx_platform::ensure_private_dir(&temporary).map_err(failed)?;
        for path in paths {
            match recorded(&path, &temporary, &source, agent.as_str() == "codex", &cancel) {
                Ok(Some(file)) => return Ok(file),
                Ok(None) => {}
                Err(error) => return Err(failed(format!("reading the host's image record: {error}"))),
            }
        }
        Err(Failure::original_gone())
    })
    .await
    .map_err(failed)?
}

fn source_name(source: &AttachmentSource) -> String {
    match source {
        AttachmentSource::Upload(a) => a.file.name.clone(),
        _ => "image".into(),
    }
}
fn attachment_media(source: &AttachmentSource) -> String {
    match source {
        AttachmentSource::Upload(a) => a.file.media_type.clone(),
        _ => String::new(),
    }
}

fn recorded(
    path: &std::path::Path,
    temporary: &std::path::Path,
    source: &AttachmentSource,
    codex: bool,
    cancel: &AtomicBool,
) -> io::Result<Option<File>> {
    let input = match fastctx_platform::file_read::open(path) {
        Ok(input) => input,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let before = fastctx_platform::file_read::snapshot(&input, &input.metadata()?)?;
    let check = input.try_clone()?;
    let scope = tempfile::tempdir_in(temporary)?;
    // The observed record has a finite end, even while its host appends.
    let mut input = BufReader::new(record_read::Prefix::new(input.take(before.size)));
    let mut selected = None;
    let mut current_turn = String::new();
    let mut message_ordinal = 0u32;
    let mut paginated = false;
    let mut completed_messages = std::collections::HashSet::new();
    let mut pending = None;
    while let Some(mut record) = fastctx_hosts::image_record::next(&mut input, scope.path(), cancel)? {
        if codex && record.value["type"] == "session_meta" {
            paginated = match record.value["payload"]["history_mode"].as_str() {
                None | Some("legacy") => false,
                Some("paginated") => true,
                Some(mode) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unsupported Codex history mode {mode}"),
                    ));
                }
            };
        }
        if codex
            && record.value["type"] == "event_msg"
            && record.value["payload"]["type"] == "task_started"
            && let Some(turn) = record.value["payload"]["turn_id"].as_str()
        {
            current_turn = turn.into();
        }
        let candidate = if codex
            && let AttachmentSource::HostImage { host_image: HostImage::Codex { turn, message, image }, .. } = source
        {
            if record.value["type"] == "response_item"
                && record.value["payload"]["type"] == "message"
                && record.value["payload"]["role"] == "user"
            {
                if paginated {
                    let metadata = &record.value["payload"]["internal_chat_message_metadata_passthrough"];
                    let is_user = metadata["content_item_kinds"].as_array().is_some_and(|kinds| {
                        kinds.iter().any(|kind| kind.as_str().is_some_and(|kind| kind.starts_with("user.")))
                    });
                    if metadata["turn_id"] != *turn || !is_user {
                        continue;
                    }
                }
                pending = Some(record);
                continue;
            }
            let payload = &record.value["payload"];
            let user_event = if paginated {
                payload["type"] == "item_completed" && payload["item"]["type"] == "UserMessage"
            } else {
                payload["type"] == "user_message"
            };
            if record.value["type"] != "event_msg" || !user_event {
                continue;
            }
            let recorded_turn = if paginated { payload["turn_id"].as_str().unwrap_or_default() } else { &current_turn };
            if turn != recorded_turn {
                pending = None;
                continue;
            }
            if paginated {
                let id = payload["item"]["id"].as_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "the Codex user message has no identity")
                })?;
                if !completed_messages.insert(id.to_owned()) {
                    continue;
                }
            }
            let matches = *message == message_ordinal;
            message_ordinal = message_ordinal.checked_add(1).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the host turn has more messages than its image selector can address",
                )
            })?;
            if !matches {
                pending = None;
                continue;
            }
            let (inline, local_path) = if paginated {
                let block = payload["item"]["content"].as_array().and_then(|blocks| {
                    blocks
                        .iter()
                        .filter(|block| matches!(block["type"].as_str(), Some("image" | "local_image")))
                        .nth(*image as usize)
                });
                match block {
                    Some(block) if block["type"] == "image" => (Some(image_value(block).clone()), None),
                    Some(block) => (None, block["path"].as_str().map(str::to_owned)),
                    None => (None, None),
                }
            } else {
                let images = payload["images"].as_array().map_or(&[][..], Vec::as_slice);
                let inline = images
                    .get(*image as usize)
                    .map(|encoded| encoded.get("url").filter(|value| !value.is_null()).unwrap_or(encoded).clone());
                let local = payload["local_images"]
                    .as_array()
                    .and_then(|paths| paths.get((*image as usize).saturating_sub(images.len())))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                (inline, local)
            };
            if let Some(encoded) = inline {
                Some((encoded, true, String::new()))
            } else {
                match (local_path, pending.take()) {
                    (Some(path), Some(previous)) => {
                        if paginated {
                            let metadata = &previous.value["payload"]["internal_chat_message_metadata_passthrough"];
                            let is_user = metadata["content_item_kinds"].as_array().is_some_and(|kinds| {
                                kinds.iter().any(|kind| kind.as_str().is_some_and(|kind| kind.starts_with("user.")))
                            });
                            if metadata["turn_id"] != *turn || !is_user {
                                continue;
                            }
                        }
                        record = previous;
                        record.value["payload"]["content"]
                            .as_array()
                            .and_then(|blocks| labelled_image(blocks, &path))
                            .map(|encoded| (encoded.clone(), true, String::new()))
                    }
                    _ => None,
                }
            }
        } else if codex {
            if record.value["type"] != "response_item"
                || record.value["payload"]["type"] != "message"
                || record.value["payload"]["role"] != "user"
            {
                continue;
            }
            record.value["payload"]["content"]
                .as_array()
                .and_then(|blocks| codex_image(blocks, source))
                .map(|(encoded, url, mime)| (encoded.clone(), url, mime))
        } else {
            if record.value["type"] != "user" {
                continue;
            }
            if let AttachmentSource::Upload(wanted) = source {
                let blocks = record.value["message"]["content"].as_array().map_or(&[][..], Vec::as_slice);
                if !fastctx_types::session::images::kept(blocks).contains(wanted) {
                    continue;
                }
                let encoded: Vec<_> = blocks
                    .iter()
                    .filter(|block| block["type"] == "image")
                    .map(|block| block["source"]["data"].clone())
                    .collect();
                let mut matching = None;
                for value in encoded {
                    let mut digest = fastctx_hosts::image_digest::Digest::default();
                    record.decode(&value, false, &mut digest, cancel)?;
                    let (size, sha256) = digest.finish();
                    if size == wanted.file.size && sha256 == wanted.file.sha256 {
                        matching = Some(value);
                        break;
                    }
                }
                matching.and_then(|encoded| {
                    record.value["message"]["content"]
                        .as_array()?
                        .iter()
                        .find(|block| block["type"] == "image" && block["source"]["data"] == encoded)
                        .map(|block| {
                            (encoded, false, block["source"]["media_type"].as_str().unwrap_or_default().to_owned())
                        })
                })
            } else if let AttachmentSource::ToolImage { tool_image, .. } = source {
                record.value["message"]["content"]
                    .as_array()
                    .and_then(|blocks| claude_tool_image(blocks, tool_image))
                    .map(|(encoded, url, mime)| (encoded.clone(), url, mime))
            } else {
                record.value["message"]["content"]
                    .as_array()
                    .and_then(|blocks| claude_image(blocks, &record.value, source))
                    .map(|(encoded, url, mime)| (encoded.clone(), url, mime))
            }
        };
        let Some((encoded, data_url, media)) = candidate else { continue };
        if selected.is_some() {
            continue;
        }
        let mut output = tempfile::NamedTempFile::new_in(scope.path())?;
        let mut hashed = Hashed { file: output.as_file_mut(), sha256: Sha256::new() };
        let decoded_mime = record.decode(&encoded, data_url, &mut hashed, cancel)?;
        let version = format!("{:x}", hashed.sha256.finalize());
        let mime = if data_url { decoded_mime } else { media };
        let extension = match mime.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsupported host image media type {mime}"),
                ));
            }
        };
        let original_name = source_name(source);
        let stem = std::path::Path::new(&original_name).file_stem().and_then(|s| s.to_str()).unwrap_or("image");
        let name = format!("{stem}.{extension}");
        let decoded = scope.path().join("decoded-image").with_extension(extension);
        output.persist(&decoded).map_err(|error| error.error)?;
        let file = fastctx_platform::file_read::open(&decoded)?;
        let snapshot = fastctx_platform::file_read::snapshot(&file, &file.metadata()?)?;
        selected = Some((
            decoded,
            snapshot,
            AttachmentFile { name, media_type: mime, source: AttachmentFileSource::HostRecord },
            version,
        ));
    }
    record_read::check(path, check, &before, input.into_inner().digest(), cancel)?;
    Ok(selected.map(|(path, snapshot, description, version)| File {
        path,
        snapshot,
        description,
        version: Some(version),
        _temporary: Some(scope),
    }))
}

/// A picture written into its file and its digest at once.
struct Hashed<'a> {
    file: &'a mut std::fs::File,
    sha256: Sha256,
}

impl io::Write for Hashed<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.file.write(bytes)?;
        self.sha256.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn claude_image<'a>(blocks: &'a [Value], row: &Value, source: &AttachmentSource) -> Option<(&'a Value, bool, String)> {
    let index = match source {
        AttachmentSource::HostImage { host_image: HostImage::Claude { message, image }, .. }
            if row["uuid"] == *message =>
        {
            *image as usize
        }
        _ => return None,
    };
    let image = blocks.iter().filter(|b| b["type"] == "image").nth(index)?;
    Some((&image["source"]["data"], false, image["source"]["media_type"].as_str().unwrap_or_default().into()))
}

/// The `image`th image a Claude tool result gave back, counting those of
/// the shape the session's history reads as images
/// (`fastctx_hosts::claude::content`).
fn claude_tool_image<'a>(blocks: &'a [Value], wanted: &ToolImage) -> Option<(&'a Value, bool, String)> {
    let result =
        blocks.iter().find(|block| block["type"] == "tool_result" && block["tool_use_id"] == wanted.item.as_str())?;
    let image = result["content"]
        .as_array()?
        .iter()
        .filter(|block| {
            let source = &block["source"];
            block["type"] == "image"
                && block.as_object().is_some_and(|object| object.len() == 2)
                && source.as_object().is_some_and(|object| object.len() == 3)
                && source["type"] == "base64"
                && source["media_type"].is_string()
                && !source["data"].is_null()
        })
        .nth(wanted.image as usize)?;
    Some((&image["source"]["data"], false, image["source"]["media_type"].as_str().unwrap_or_default().into()))
}

/// A local picture named by the host's authoritative user item.
fn local_image(path: PathBuf, cancel: &AtomicBool) -> io::Result<File> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "the picture read was cancelled"));
    }
    let file = fastctx_platform::file_read::open(&path)?;
    let snapshot = fastctx_platform::file_read::snapshot(&file, &file.metadata()?)?;
    let format = image::ImageReader::new(BufReader::new(file)).with_guessed_format()?.format().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "the host's local picture has no recognized image format")
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the host's local picture has no UTF-8 file name"))?
        .to_string();
    Ok(File {
        path,
        snapshot,
        description: AttachmentFile {
            name,
            media_type: format.to_mime_type().into(),
            source: AttachmentFileSource::Original,
        },
        version: None,
        _temporary: None,
    })
}

/// An image a host gave as base64, written where it can be read as a file.
fn decoded(temporary: &std::path::Path, mime: &str, base64: &str) -> io::Result<File> {
    use base64::Engine as _;
    let extension = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported host image media type {mime}"),
            ));
        }
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("the host's image: {error}")))?;
    fastctx_platform::ensure_private_dir(temporary)?;
    let scope = tempfile::tempdir_in(temporary)?;
    let path = scope.path().join("decoded-image").with_extension(extension);
    std::fs::write(&path, &bytes)?;
    let file = fastctx_platform::file_read::open(&path)?;
    let snapshot = fastctx_platform::file_read::snapshot(&file, &file.metadata()?)?;
    Ok(File {
        path,
        snapshot,
        description: AttachmentFile {
            name: format!("image.{extension}"),
            media_type: mime.into(),
            source: AttachmentFileSource::HostRecord,
        },
        version: Some(format!("{:x}", Sha256::digest(&bytes))),
        _temporary: Some(scope),
    })
}

fn codex_image<'a>(blocks: &'a [Value], source: &AttachmentSource) -> Option<(&'a Value, bool, String)> {
    match source {
        AttachmentSource::Upload(wanted) => {
            // Each World envelope precedes its own images. A local-image
            // label binds a transformed picture to the original's path.
            let mut matched = false;
            let mut labelled = false;
            for block in blocks {
                if let Some(text) = block["text"].as_str() {
                    if let Some((envelope, _)) = Envelope::read(text) {
                        matched = envelope.tells(&wanted.id);
                    }
                    labelled = image_label(text, &wanted.path);
                }
                if block["type"] == "input_image" && matched && labelled {
                    return Some((image_value(block), true, String::new()));
                }
            }
            None
        }
        _ => None,
    }
}

fn labelled_image<'a>(blocks: &'a [Value], path: &str) -> Option<&'a Value> {
    let mut labelled = false;
    for block in blocks {
        if let Some(text) = block["text"].as_str() {
            labelled = image_label(text, path);
        }
        if labelled && block["type"] == "input_image" {
            return Some(image_value(block));
        }
    }
    None
}

fn image_value(block: &Value) -> &Value {
    block.get("image_url").filter(|value| !value.is_null()).unwrap_or(block)
}

fn image_label(text: &str, path: &str) -> bool {
    text.starts_with("<image ")
        && text
            .strip_suffix("\">")
            .and_then(|text| text.split_once(" path=\""))
            .is_some_and(|(_, stored)| stored == path)
}
