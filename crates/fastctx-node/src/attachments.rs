//! Verified user uploads, kept with the resident on its host's machine.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_net::data::Stream;
use fastctx_net::link::Feed;
use fastctx_platform::FileLock;
use fastctx_platform::ipc::FrameReader;
use fastctx_types::attachments::{Attachment, UPLOAD_CHUNK, UploadFile, UploadTarget, Uploaded};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::Node;

pub fn delivery(from: &MachineName, text: String, attachments: Vec<Attachment>) -> fastctx_types::residents::Delivery {
    use fastctx_types::residents::{Delivery, Notice};
    use fastctx_types::session::{Speaker, attachments::AttachedMessage};
    let id = crate::speech::new_id("talk");
    let notice = if attachments.is_empty() {
        Notice::Words { from: Speaker::User { machine: from.clone() }, text }
    } else {
        Notice::Attached { message: AttachedMessage { delivery: id.clone(), from: from.clone(), attachments }, text }
    };
    Delivery { id, at: jiff::Timestamp::now(), notice }
}

#[derive(Serialize, Deserialize)]
struct Draft {
    agent: AgentName,
    town: Option<TownName>,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("upload: {error}"))
}

/// Receives from a dedicated user IPC connection. A remote destination is
/// fed directly from that connection, with no intermediate file.
pub async fn upload(
    node: &Arc<Node>,
    target: UploadTarget,
    machine: Option<MachineName>,
    file: UploadFile,
    composer: Option<fastctx_types::composer::UploadScope>,
    mut source: FrameReader,
) -> Result<Uploaded, Failure> {
    validate_file(&file)?;
    let machine = match machine {
        Some(machine) => machine,
        None if composer.is_some() => match &target {
            UploadTarget::Resident { resident } if node.residents.about(resident).is_some() => node.machine(),
            UploadTarget::Resident { resident } => crate::colleagues::find(node, resident).await?,
            _ => return Err(failure("a shared composer upload needs an existing resident")),
        },
        None => node.machine(),
    };
    if machine == node.machine() {
        if let Some(cached) = cached(node, &node.machine(), &target, &file, composer.as_ref()).await? {
            return Ok(cached);
        }
        let mut arriving = prepare(node, target, file).await?;
        let mut guard = shared(node, &node.machine(), &arriving.attachment, composer.as_ref()).await?;
        let result = async {
            loop {
                let piece = source
                    .recv_up_to(UPLOAD_CHUNK)
                    .await
                    .map_err(failure)?
                    .ok_or_else(|| failure("the uploader disconnected before ending the file"))?;
                if piece.is_empty() {
                    return finish(arriving, guard.as_mut()).await;
                }
                arriving.write(&piece)?;
            }
        }
        .await;
        if let Err(error) = &result
            && let Some(guard) = guard.as_mut()
        {
            guard.fail(error).await;
        }
        return result;
    }
    let link = node.link().ok_or_else(|| failure("this machine is not connected to a World"))?;
    let size = file.size;
    let source_error = Arc::new(Mutex::new(None));
    let feeding_error = source_error.clone();
    let feed: Feed = Box::new(move |stream| {
        Box::pin(async move {
            if let Err(reason) = forward(source, stream, size).await {
                *feeding_error.lock().expect("never poisoned") = Some(reason);
            }
            // Keep the destination's verified receipt or explicit failure authoritative.
            Ok(())
        })
    });
    let answer = link.call_with_stream(&machine, RemoteCall::Upload { target, file, composer }, feed).await;
    let result = match answer {
        Ok(Ok(RemoteReply::Uploaded(attachment))) => return Ok(Uploaded { attachment, composer: None }),
        Ok(Ok(RemoteReply::ComposerUploaded(result))) => {
            return Ok(Uploaded { attachment: result.attachment, composer: Some(result.updated) });
        }
        Ok(Ok(_)) => failure(format!("{machine} answered something other than an upload")),
        Ok(Err(error)) => Failure::new(format!("on {machine}: {}", error.message)),
        Err(error) => failure(error),
    };
    match source_error.lock().expect("never poisoned").take() {
        Some(reason) => Err(Failure::new(format!("{}; source stream: {reason}", result.message))),
        None => Err(result),
    }
}

async fn cached(
    node: &Arc<Node>,
    from: &MachineName,
    target: &UploadTarget,
    file: &UploadFile,
    scope: Option<&fastctx_types::composer::UploadScope>,
) -> Result<Option<Uploaded>, Failure> {
    let Some(scope) = scope else { return Ok(None) };
    let UploadTarget::Resident { resident } = target else {
        return Err(failure("a shared composer upload needs an existing resident"));
    };
    Ok(crate::composer::uploads::cached(node, resident, from, scope, file)
        .await?
        .map(|result| Uploaded { attachment: result.attachment, composer: Some(result.updated) }))
}

async fn shared(
    node: &Arc<Node>,
    from: &MachineName,
    attachment: &Attachment,
    scope: Option<&fastctx_types::composer::UploadScope>,
) -> Result<Option<crate::composer::uploads::Guard>, Failure> {
    match scope {
        Some(scope) => {
            crate::composer::uploads::begin(node, &attachment.owner.resident, from, scope, attachment).await.map(Some)
        }
        None => Ok(None),
    }
}

async fn finish(arriving: Arriving, guard: Option<&mut crate::composer::uploads::Guard>) -> Result<Uploaded, Failure> {
    let attachment = arriving.finish()?;
    let composer = match guard {
        Some(guard) => Some(guard.finish(&attachment).await),
        None => None,
    };
    Ok(Uploaded { attachment, composer })
}

pub fn reply(uploaded: Uploaded) -> fastctx_types::Reply {
    match uploaded.composer {
        Some(updated) => fastctx_types::Reply::ComposerUploaded(fastctx_types::composer::UploadResult {
            attachment: uploaded.attachment,
            updated,
        }),
        None => fastctx_types::Reply::Uploaded(uploaded.attachment),
    }
}

pub fn remote_reply(uploaded: Uploaded) -> RemoteReply {
    match uploaded.composer {
        Some(updated) => RemoteReply::ComposerUploaded(fastctx_types::composer::UploadResult {
            attachment: uploaded.attachment,
            updated,
        }),
        None => RemoteReply::Uploaded(uploaded.attachment),
    }
}

async fn forward(mut source: FrameReader, mut stream: Stream, size: u64) -> Result<(), String> {
    let mut received = 0u64;
    loop {
        let piece = source
            .recv_up_to(UPLOAD_CHUNK)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the uploader disconnected before ending the file".to_string())?;
        if piece.is_empty() {
            if received != size {
                return Err(format!("the uploader sent {received} bytes; expected {size}"));
            }
            stream.say(&"uploaded").await?;
            return stream.finish().await;
        }
        if piece.len() as u64 > size - received {
            return Err(format!("the uploader sent more than the declared {size} bytes"));
        }
        received += piece.len() as u64;
        stream.send(&piece).await?;
    }
}

/// The destination of an authenticated upload call and its data stream.
pub async fn receive(
    node: &Arc<Node>,
    from: &MachineName,
    target: UploadTarget,
    file: UploadFile,
    composer: Option<fastctx_types::composer::UploadScope>,
    mut stream: Stream,
) -> Result<Uploaded, Failure> {
    validate_file(&file)?;
    if let Some(cached) = cached(node, from, &target, &file, composer.as_ref()).await? {
        return Ok(cached);
    }
    let size = file.size;
    let mut arriving = prepare(node, target, file).await?;
    let mut guard = shared(node, from, &arriving.attachment, composer.as_ref()).await?;
    let result = async {
        stream.take(size, |piece| arriving.write(piece).map_err(|error| error.message)).await.map_err(Failure::new)?;
        let end: String = stream.hear().await.map_err(failure)?;
        if end != "uploaded" {
            return Err(failure("the uploader did not end the file"));
        }
        let uploaded = finish(arriving, guard.as_mut()).await?;
        stream.done().await.map_err(failure)?;
        Ok(uploaded)
    }
    .await;
    if let Err(error) = &result
        && let Some(guard) = guard.as_mut()
    {
        guard.fail(error).await;
    }
    result
}

fn validate_file(file: &UploadFile) -> Result<(), Failure> {
    if file.name.is_empty() || file.media_type.is_empty() {
        return Err(failure("name and media_type must be supplied"));
    }
    if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(failure("sha256 must be 64 lowercase hexadecimal digits"));
    }
    Ok(())
}

async fn prepare(node: &Arc<Node>, target: UploadTarget, file: UploadFile) -> Result<Arriving, Failure> {
    let (resident, town) = match target {
        UploadTarget::Resident { resident } => {
            let (_, town) = node
                .residents
                .about(&resident)
                .ok_or_else(|| failure(format!("no resident {resident} lives on {}", node.machine())))?;
            (resident, town)
        }
        UploadTarget::New { agent, town } => {
            if !matches!(agent.as_str(), "codex" | "claude") {
                return Err(failure(format!("the World cannot open {agent} sessions")));
            }
            if let Some(town) = &town {
                crate::town::project::spans_here(node, town).await?;
            }
            // Drafts are not in the live resident index. Their 128-bit ids
            // also occupy a different namespace from the ordinary four-hex
            // allocator, which must never assign an unopened draft's id.
            let mut identity = [0u8; 16];
            getrandom::fill(&mut identity).map_err(failure)?;
            let resident: ResidentId =
                format!("{agent}-{:032x}", u128::from_be_bytes(identity)).parse().map_err(failure)?;
            let directory = node.world.resident(town.as_ref(), &resident);
            std::fs::create_dir_all(directory.path()).map_err(failure)?;
            write_new(&directory.path().join("attachment-draft.json"), &Draft { agent, town: town.clone() })?;
            (resident, town)
        }
        UploadTarget::Draft { resident, town } => {
            let directory = node.world.resident(town.as_ref(), &resident);
            let draft: Draft = read_json(&directory.path().join("attachment-draft.json"))?;
            if draft.town != town || node.residents.about(&resident).is_some() {
                return Err(failure("that upload draft is already open or belongs to another town"));
            }
            (resident, town)
        }
    };
    // Uploads stay where they occurred when a resident changes towns. No
    // resident-transition lock may be held while network bytes arrive.
    let directory = node.world.resident(town.as_ref(), &resident).path().join("attachments");
    std::fs::create_dir_all(&directory).map_err(failure)?;
    let id = crate::speech::new_id("attachment");
    let directory = directory.join(id.as_str());
    std::fs::create_dir(&directory).map_err(failure)?;
    let directory = std::fs::canonicalize(directory).map_err(failure)?;
    let temporary = directory.join("upload.part");
    let output = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(failure)?;
    // The original name is metadata, never a path component. The image
    // extension also lets native hosts that dispatch by extension open it.
    let name = match file.media_type.as_str() {
        "image/png" => "content.png",
        "image/jpeg" => "content.jpg",
        "image/webp" => "content.webp",
        "image/gif" => "content.gif",
        _ => "content",
    };
    let path = directory.join(name);
    let attachment = Attachment {
        id,
        owner: Address { machine: node.machine(), resident },
        town,
        path: crate::world::native(&path),
        file,
    };
    Ok(Arriving { attachment, temporary, path, output: Some(output), received: 0, hash: Sha256::new() })
}

struct Arriving {
    attachment: Attachment,
    temporary: PathBuf,
    path: PathBuf,
    output: Option<std::fs::File>,
    received: u64,
    hash: Sha256,
}

impl Arriving {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        if bytes.len() as u64 > self.attachment.file.size - self.received {
            return Err(failure("the uploader sent more than the declared size"));
        }
        self.output.as_mut().expect("upload is open").write_all(bytes).map_err(failure)?;
        self.hash.update(bytes);
        self.received += bytes.len() as u64;
        Ok(())
    }

    fn finish(mut self) -> Result<Attachment, Failure> {
        if self.received != self.attachment.file.size {
            return Err(failure(format!("received {} bytes; expected {}", self.received, self.attachment.file.size)));
        }
        if format!("{:x}", self.hash.clone().finalize()) != self.attachment.file.sha256 {
            return Err(failure("the file's SHA-256 does not match the uploaded bytes"));
        }
        self.output.take().expect("upload is open").sync_all().map_err(failure)?;
        std::fs::rename(&self.temporary, &self.path).map_err(failure)?;
        write_new(&self.path.parent().expect("attachment directory").join("receipt.json"), &self.attachment)?;
        Ok(self.attachment.clone())
    }
}

impl Drop for Arriving {
    fn drop(&mut self) {
        drop(self.output.take());
        if let Err(error) = std::fs::remove_file(&self.temporary)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("removing incomplete upload {}: {error}", self.temporary.display());
        }
    }
}

fn write_new(path: &std::path::Path, value: &impl Serialize) -> Result<(), Failure> {
    let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(path).map_err(failure)?;
    serde_json::to_writer(&mut file, value).map_err(failure)?;
    file.sync_all().map_err(failure)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T, Failure> {
    serde_json::from_slice(&std::fs::read(path).map_err(failure)?).map_err(failure)
}

/// Resolves only server-written receipts and checks the bytes again before
/// handing their paths to a host. Receipts remain valid across town moves.
pub async fn resolve(node: &Arc<Node>, resident: &ResidentId, attachments: &[Attachment]) -> Result<(), Failure> {
    if attachments.is_empty() {
        return Ok(());
    }
    let (node, resident, attachments) = (node.clone(), resident.clone(), attachments.to_vec());
    tokio::task::spawn_blocking(move || {
        let mut buffer = vec![0u8; UPLOAD_CHUNK];
        for attachment in attachments {
            if attachment.owner.machine != node.machine() || attachment.owner.resident != resident {
                return Err(failure("an attachment belongs to another resident or machine"));
            }
            let directory = node
                .world
                .resident(attachment.town.as_ref(), &resident)
                .path()
                .join("attachments")
                .join(attachment.id.as_str());
            let kept: Attachment = read_json(&directory.join("receipt.json"))?;
            if kept != attachment {
                return Err(failure("the attachment receipt differs from the stored upload"));
            }
            let mut file = std::fs::File::open(&kept.path).map_err(failure)?;
            let mut hash = Sha256::new();
            let mut size = 0u64;
            loop {
                let read = file.read(&mut buffer).map_err(failure)?;
                if read == 0 {
                    break;
                }
                size += read as u64;
                hash.update(&buffer[..read]);
            }
            if size != kept.file.size || format!("{:x}", hash.finalize()) != kept.file.sha256 {
                return Err(failure(format!("{} on {} changed after upload", kept.path, kept.owner.machine)));
            }
        }
        Ok(())
    })
    .await
    .map_err(failure)?
}

/// Reserves a draft for precisely one Open. The lock spans host creation;
/// the ordinary resident record prevents reusing the draft after success.
pub async fn opening(
    node: &Arc<Node>,
    agent: &AgentName,
    town: Option<&TownName>,
    attachments: &[Attachment],
) -> Result<(ResidentId, Option<FileLock>), Failure> {
    let Some(first) = attachments.first() else {
        return Ok((node.residents.new_id(agent)?, None));
    };
    let id = first.owner.resident.clone();
    if first.owner.machine != node.machine() || first.town.as_ref() != town {
        return Err(failure("the upload draft belongs to another machine or town"));
    }
    let directory = node.world.resident(town, &id);
    let draft: Draft = read_json(&directory.path().join("attachment-draft.json"))?;
    if draft.agent != *agent || draft.town.as_ref() != town {
        return Err(failure("the upload draft is for another agent or town"));
    }
    let lock = FileLock::try_acquire(&directory.path().join("attachment-open.lock"))
        .map_err(failure)?
        .ok_or_else(|| failure("this upload draft is already being opened"))?;
    if node.residents.about(&id).is_some() {
        return Err(failure("this upload draft has already been opened"));
    }
    resolve(node, &id, attachments).await?;
    Ok((id, Some(lock)))
}
