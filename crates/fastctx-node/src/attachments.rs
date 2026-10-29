//! User originals remain on the computer that attached them.

pub(crate) mod delivered;
pub(crate) mod destinations;
pub(crate) mod follow;
pub(crate) mod forwarded;
pub(crate) mod lifetime;
mod references;
pub(crate) mod sent;
pub(crate) use references::{inherit, inherit_texts, preview};

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_net::data::Stream;
use fastctx_net::link::Feed;
use fastctx_platform::FileLock;
use fastctx_platform::ipc::FrameReader;
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::attachments::{Attachment, UPLOAD_CHUNK, UploadFile, UploadStart, UploadTarget, Uploaded};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::{ConversationDir, Kind};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::Node;

/// The user's words typed on `from` as a delivery; named by the device's
/// own `id` for them when it gave one, so the same words said again under
/// it are known as the same (`world-network.md`, 改变什么的动作都带编号).
pub fn delivery(
    from: &MachineName,
    text: String,
    attachments: Vec<Attachment>,
    id: Option<&str>,
) -> fastctx_types::residents::Delivery {
    use fastctx_types::residents::{Delivery, Notice};
    use fastctx_types::session::{Speaker, attachments::AttachedMessage};
    let id = match id {
        Some(id) => crate::speech::fixed_id("talk", &[from.as_str(), id]),
        None => crate::speech::new_id("talk"),
    };
    let notice = if attachments.is_empty() {
        Notice::Words { from: Speaker::User { machine: from.clone() }, text, goal_report: None, goal_question: None }
    } else {
        Notice::Attached { message: AttachedMessage { delivery: id.clone(), from: from.clone(), attachments }, text }
    };
    Delivery { id, at: jiff::Timestamp::now(), notice }
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
struct Draft {
    agent: AgentName,
    town: Option<TownName>,
    #[serde(default)]
    from: Option<MachineName>,
    #[serde(default)]
    acting: Option<Acting>,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("upload: {error}"))
}

/// A transport failure of an upload: no answer about the file came, and
/// the same upload goes again (`world-network.md`, 断开不是结束).
fn broke(why: impl std::fmt::Display) -> Failure {
    Failure::unanswered(format!("upload: the connection broke: {why}"))
}

/// Receives from a dedicated user IPC connection. A remote destination is
/// fed directly from that connection, with no intermediate file. A shared
/// input's upload that broke goes on from what its destination holds of it:
/// the uploader sends the file whole again, and what is held is skipped.
pub async fn upload(
    node: &Arc<Node>,
    target: UploadTarget,
    machine: Option<Written>,
    file: UploadFile,
    composer: Option<fastctx_types::composer::UploadScope>,
    mut source: FrameReader,
) -> Result<Uploaded, Failure> {
    if let UploadTarget::Sent { machine, target, operation, account, files_on } = target {
        let uploader = destinations::sender(node, &machine, &target, account.as_ref())?;
        if node.is_here(&machine) {
            let acting = destinations::acting(node, &target, &uploader)?;
            if !file.media_type.starts_with("image/") && destinations::assisted(node, &target, &acting)? {
                return forwarded::upload(
                    node,
                    &node.machine(),
                    acting,
                    *target,
                    operation,
                    file,
                    composer,
                    files_on,
                    forwarded::Input::Local(source),
                )
                .await;
            }
        }
        if !node.is_here(&machine) {
            let (link, account_name) = node.speaking(&uploader, &machine, Purpose::Agents).map_err(failure)?;
            let supported = match link
                .call(&machine.name, &account_name, Purpose::Look, RemoteCall::Facts)
                .await
                .map_err(broke)??
            {
                RemoteReply::Facts(facts) => facts.attachment_places,
                _ => false,
            };
            if supported {
                let sent = UploadTarget::Sent { machine: machine.clone(), target, operation, account, files_on };
                return relay(node, &uploader, &machine, sent, file, composer, source).await;
            }
            if files_on.is_some() {
                return Err(Failure::not_run("upgrade the conversation machine to select attachment storage"));
            }
            destinations::owns_here(node, &uploader)?;
        }
        return sent::upload(node, &uploader, machine, *target, operation, file, composer, source).await;
    }
    validate_file(&file)?;
    let local = machine.is_none()
        && (composer.is_none()
            || match &target {
                UploadTarget::Resident { resident } => node.residents.about(resident).is_some(),
                _ => false,
            });
    let machine = match machine {
        Some(machine) => machine,
        None if composer.is_some() => match &target {
            UploadTarget::Resident { resident } if node.residents.about(resident).is_some() => node.machine().into(),
            UploadTarget::Resident { resident } => crate::colleagues::find_for(node, &Acting::Here, resident).await?,
            _ => return Err(failure("a shared composer upload needs an existing resident")),
        },
        None => node.machine().into(),
    };
    if local || node.is_here(&machine) {
        let uploader = destinations::sender(node, &machine, &target, None)?;
        let acting = destinations::acting(node, &target, &uploader)?;
        // The account the file is for may use the conversation's project,
        // before anything is kept for it.
        check_project(node, &acting, &target).await?;
        if !file.media_type.starts_with("image/") && destinations::assisted(node, &target, &acting)? {
            let operation = composer
                .as_ref()
                .map(|scope| scope.operation.clone())
                .unwrap_or_else(|| crate::speech::new_id("upload").to_string());
            return forwarded::upload(
                node,
                &node.machine(),
                acting,
                target,
                operation,
                file,
                composer,
                None,
                forwarded::Input::Local(source),
            )
            .await;
        }
        let _place = using_target(node, &target).await;
        let (mut arriving, mut guard) =
            match arrival(node, &node.machine(), &acting, target, file, composer.as_ref(), true).await? {
                Arrival::Ended(uploaded) => return Ok(uploaded),
                Arrival::Going(arriving, guard) => (arriving, guard),
            };
        let mut held = Skipping(arriving.received);
        let result = async {
            loop {
                let piece = source
                    .recv_up_to(UPLOAD_CHUNK)
                    .await
                    .map_err(broke)?
                    .ok_or_else(|| broke("the uploader disconnected before ending the file"))?;
                if piece.is_empty() {
                    return finish(arriving, guard.as_mut(), None).await;
                }
                arriving.write(held.past(&piece))?;
                fastctx_platform::doing::progress();
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
    let acting = destinations::sender(node, &machine, &target, None)?;
    relay(node, &acting, &machine, target, file, composer, source).await
}

#[allow(clippy::too_many_arguments)]
async fn relay(
    node: &Arc<Node>,
    acting: &Acting,
    machine: &Written,
    target: UploadTarget,
    file: UploadFile,
    composer: Option<fastctx_types::composer::UploadScope>,
    source: FrameReader,
) -> Result<Uploaded, Failure> {
    let (link, account) = node.speaking(acting, machine, Purpose::Agents).map_err(failure)?;
    let machine = machine.name.clone();
    if !file.media_type.starts_with("image/") {
        match link.call(&machine, &account, Purpose::Look, RemoteCall::Facts).await.map_err(broke)?? {
            RemoteReply::Facts(facts) if facts.attachment_places => {}
            _ => {
                return Err(Failure::not_run(
                    "upgrade the conversation machine to keep assisted files on this account's machines",
                ));
            }
        }
    }
    let resumes = composer.is_some() && resumes_there(&link, &machine, &account).await?;
    let size = file.size;
    let source_error = Arc::new(Mutex::new(None));
    let feeding_error = source_error.clone();
    let feed: Feed = Box::new(move |stream| {
        Box::pin(async move {
            if let Err(reason) = forward(source, stream, size, resumes).await {
                *feeding_error.lock().expect("never poisoned") = Some(reason);
            }
            // Keep the destination's verified receipt or explicit failure authoritative.
            Ok(())
        })
    });
    let upload = RemoteCall::Upload { target, file, composer, resumes };
    let answer = link.call_with_stream(&machine, &account, Purpose::Agents, upload, feed).await;
    let result = match answer {
        Ok(Ok(RemoteReply::Uploaded(attachment))) => return Ok(Uploaded { attachment, composer: None }),
        Ok(Ok(RemoteReply::ComposerUploaded(result))) => {
            return Ok(Uploaded { attachment: result.attachment, composer: Some(result.updated) });
        }
        Ok(Ok(_)) => failure(format!("{machine} answered something other than an upload")),
        Ok(Err(error)) => Failure { message: format!("on {machine}: {}", error.message), ..error },
        Err(error) => broke(error),
    };
    match source_error.lock().expect("never poisoned").take() {
        Some(reason) => Err(Failure { message: format!("{}; source stream: {reason}", result.message), ..result }),
        None => Err(result),
    }
}

/// Whether `machine` keeps what arrived of an upload that broke, and takes
/// it up from there. A machine that cannot be asked cannot take the upload
/// either.
async fn resumes_there(
    link: &fastctx_net::link::Link,
    machine: &MachineName,
    account: &fastctx_types::AccountName,
) -> Result<bool, Failure> {
    match link.call(machine, account, Purpose::Look, RemoteCall::Facts).await {
        Ok(Ok(RemoteReply::Facts(facts))) => Ok(facts.upload_resumes),
        Ok(_) => Ok(false),
        Err(error) => Err(broke(error)),
    }
}

/// Where an upload stands before its bytes come.
enum Arrival {
    /// It went before under its operation and ended, with this answer.
    Ended(Uploaded),
    /// Its bytes go here, from `Arriving::received` on.
    Going(Arriving, Option<crate::composer::uploads::Guard>),
}

/// Readies the arrival of `file` for `target`, from `from`: a shared
/// input's upload is known by its operation, and one that broke goes on
/// from its part when the sender skips what is held (`resume`).
async fn arrival(
    node: &Arc<Node>,
    from: &MachineName,
    acting: &Acting,
    target: UploadTarget,
    file: UploadFile,
    scope: Option<&fastctx_types::composer::UploadScope>,
    resume: bool,
) -> Result<Arrival, Failure> {
    use crate::composer::uploads::Prior;
    let Some(scope) = scope else { return Ok(Arrival::Going(prepare(node, from, acting, target, file).await?, None)) };
    let UploadTarget::Resident { resident } = &target else {
        return Err(failure("a shared composer upload needs an existing resident"));
    };
    let resident = resident.clone();
    match crate::composer::uploads::prior(node, &resident, from, scope, &file).await? {
        Prior::Ended(result) => {
            Ok(Arrival::Ended(Uploaded { attachment: result.attachment, composer: Some(result.updated) }))
        }
        Prior::Again(attachment, mut guard) => {
            let directory = Path::new(&attachment.path).parent().map(Path::to_path_buf);
            let arriving = match directory {
                Some(directory) => tokio::task::spawn_blocking(move || arriving_in(&directory, *attachment, resume))
                    .await
                    .map_err(failure)
                    .and_then(|arriving| arriving),
                None => Err(failure("the upload's receipt names no directory")),
            };
            match arriving {
                Ok(mut arriving) => {
                    arriving.keep = true;
                    Ok(Arrival::Going(arriving, Some(guard)))
                }
                Err(error) => {
                    guard.fail(&error).await;
                    Err(error)
                }
            }
        }
        Prior::Fresh => {
            let mut arriving = prepare(node, from, acting, target, file).await?;
            let guard = crate::composer::uploads::begin(node, &resident, from, scope, &arriving.attachment).await?;
            arriving.keep = true;
            Ok(Arrival::Going(arriving, Some(guard)))
        }
    }
}

async fn finish(
    arriving: Arriving,
    guard: Option<&mut crate::composer::uploads::Guard>,
    work: Option<fastctx_net::data::ReceiptWork>,
) -> Result<Uploaded, Failure> {
    let attachment = if let Some(writing) = work.as_ref().map(|work| work.begin()) {
        tokio::task::spawn_blocking(move || {
            let _settling = writing;
            arriving.finish()
        })
        .await
        .map_err(failure)??
    } else {
        arriving.finish()?
    };
    let _settling = work.as_ref().map(|work| work.begin());
    let composer = match guard {
        Some(guard) => Some(guard.finish(&attachment).await),
        None => None,
    };
    Ok(Uploaded { attachment, composer })
}

/// The bytes of an upload past those its destination already holds.
struct Skipping(u64);

impl Skipping {
    fn past<'a>(&mut self, piece: &'a [u8]) -> &'a [u8] {
        let skipped = self.0.min(piece.len() as u64);
        self.0 -= skipped;
        &piece[skipped as usize..]
    }
}

pub fn reply(uploaded: Uploaded) -> fastctx_types::Reply {
    match uploaded.composer {
        Some(updated) => fastctx_types::Reply::ComposerUploaded(Box::new(fastctx_types::composer::UploadResult {
            attachment: uploaded.attachment,
            updated,
        })),
        None => fastctx_types::Reply::Uploaded(uploaded.attachment),
    }
}

pub fn remote_reply(uploaded: Uploaded) -> RemoteReply {
    match uploaded.composer {
        Some(updated) => RemoteReply::ComposerUploaded(Box::new(fastctx_types::composer::UploadResult {
            attachment: uploaded.attachment,
            updated,
        })),
        None => RemoteReply::Uploaded(uploaded.attachment),
    }
}

async fn forward(mut source: FrameReader, mut stream: Stream, size: u64, resumes: bool) -> Result<(), String> {
    let mut held = Skipping(if resumes { stream.hear_step::<UploadStart>("upload offset").await?.from } else { 0 });
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
        let rest = held.past(&piece);
        if !rest.is_empty() {
            stream.send(rest).await?;
        }
    }
}

/// The destination of an authenticated upload call and its data stream.
/// When the caller `resumes`, it is first told how much of the file is held
/// here from an earlier try, and sends the rest.
#[allow(clippy::too_many_arguments)]
pub async fn receive(
    node: &Arc<Node>,
    from: &MachineName,
    uploader: &Acting,
    target: UploadTarget,
    file: UploadFile,
    composer: Option<fastctx_types::composer::UploadScope>,
    resumes: bool,
    mut stream: Stream,
) -> Result<Uploaded, Failure> {
    let acting = destinations::acting(node, &target, uploader)?;
    let routed = !file.media_type.starts_with("image/") && destinations::assisted(node, &target, &acting)?;
    if let UploadTarget::Sent { machine, target, operation, account, files_on } = target {
        if account
            .as_ref()
            .is_some_and(|account| uploader != &Acting::one(account.hub.clone(), account.account.clone()))
        {
            return Err(Failure::refused("the upload account differs from its authenticated sender"));
        }
        if routed {
            if !node.is_here(&machine) {
                return Err(failure("upload the file through its conversation machine"));
            }
            return forwarded::upload(
                node,
                from,
                acting,
                *target,
                operation,
                file,
                composer,
                files_on,
                forwarded::Input::Remote(stream, resumes),
            )
            .await;
        }
        return sent::receive(node, from, &acting, machine, *target, operation, file, composer, resumes, stream).await;
    }
    if routed {
        let operation = composer
            .as_ref()
            .map(|scope| scope.operation.clone())
            .unwrap_or_else(|| crate::speech::new_id("upload").to_string());
        return forwarded::upload(
            node,
            from,
            acting,
            target,
            operation,
            file,
            composer,
            None,
            forwarded::Input::Remote(stream, resumes),
        )
        .await;
    }
    let _place = using_target(node, &target).await;
    validate_file(&file)?;
    let size = file.size;
    let (mut arriving, mut guard) = match arrival(node, from, &acting, target, file, composer.as_ref(), resumes).await?
    {
        Arrival::Ended(uploaded) => return Ok(uploaded),
        Arrival::Going(arriving, guard) => (arriving, guard),
    };
    let result = async {
        if resumes {
            stream.say(&UploadStart { from: arriving.received }).await.map_err(broke)?;
        }
        let ((back, refused), taken) = stream
            .take_writing(size - arriving.received, (arriving, None), |(arriving, refused), piece| {
                arriving.write(piece).map_err(|error| {
                    let message = error.message.clone();
                    *refused = Some(error);
                    message
                })
            })
            .await
            .map_err(failure)?;
        arriving = back;
        if let Err(why) = taken {
            return Err(refused.unwrap_or_else(|| broke(why)));
        }
        let end: String = stream.hear_step("uploaded").await.map_err(broke)?;
        if end != "uploaded" {
            return Err(failure("the uploader did not end the file"));
        }
        let uploaded = finish(arriving, guard.as_mut(), Some(stream.receipt_work())).await?;
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

/// Shares the place an upload for `target` lands in: the conversation's, or
/// the draft's; a new draft's is its own, which nothing else knows yet.
async fn using_target(node: &Arc<Node>, target: &UploadTarget) -> Option<crate::data::places::Using> {
    match target {
        UploadTarget::Resident { resident } | UploadTarget::Draft { resident, .. } => {
            Some(node.places.using(resident.as_str()).await)
        }
        UploadTarget::New { .. } | UploadTarget::Sent { .. } => None,
    }
}

/// Checks the upload's authenticated account, or the existing conversation's
/// accounts, before reserving any project-owned storage.
pub(crate) async fn check_project(node: &Arc<Node>, acting: &Acting, target: &UploadTarget) -> Result<(), Failure> {
    match target.conversation_target() {
        UploadTarget::Resident { resident } => {
            let (about, town) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
            if let Some(town) = town {
                crate::town::access::ready(node, &about.acting, &town).await?;
            }
        }
        UploadTarget::New { town, .. } | UploadTarget::Draft { town, .. } => {
            if let Some(town) = town {
                crate::town::access::ready(node, acting, town).await?;
            }
        }
        UploadTarget::Sent { .. } => return Err(failure("nested upload policy")),
    }
    Ok(())
}

async fn prepare(
    node: &Arc<Node>,
    from: &MachineName,
    acting: &Acting,
    target: UploadTarget,
    file: UploadFile,
) -> Result<Arriving, Failure> {
    let (resident, town) = match target {
        UploadTarget::Sent { .. } => return Err(failure("nested upload policy")),
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

            // Drafts are not in the live resident index. Their 128-bit ids
            // also occupy a different namespace from the ordinary four-hex
            // allocator, which must never assign an unopened draft's id.
            let mut identity = [0u8; 16];
            getrandom::fill(&mut identity).map_err(failure)?;
            let resident: ResidentId =
                format!("{agent}-{:032x}", u128::from_be_bytes(identity)).parse().map_err(failure)?;
            let directory = node.world.conversation(&resident);
            std::fs::create_dir_all(directory.path()).map_err(failure)?;
            write_new(
                &directory.draft(),
                &Draft { agent, town: town.clone(), from: Some(from.clone()), acting: Some(acting.clone()) },
            )?;
            (resident, town)
        }
        UploadTarget::Draft { resident, town } => {
            let directory = node.world.conversation(&resident);
            let draft: Draft = read_json(&directory.draft())?;
            if draft.acting.as_ref() != Some(acting) || draft.from.as_ref() != Some(from) {
                return Err(Failure::refused(
                    "the upload draft belongs to another account or device; attach the file again",
                ));
            }
            if draft.town != town || node.residents.about(&resident).is_some() {
                return Err(failure("that upload draft is already open or belongs to another town"));
            }
            (resident, town)
        }
    };
    // No resident storage lock may be held while network bytes arrive.
    let directory = node.world.conversation(&resident).attachments();
    std::fs::create_dir_all(&directory).map_err(failure)?;
    let id = crate::speech::new_id("attachment");
    let directory = directory.join(id.as_str());
    std::fs::create_dir(&directory).map_err(failure)?;
    let owner = Address { machine: node.machine(), resident };
    arriving_in(&directory, Attachment { id, owner, conversation: None, town, path: String::new(), file }, false)
}

/// An upload arriving into `directory`, its own, for `attachment`, whose
/// path it gives; going on from the part an earlier try left there when
/// `resume`, else from nothing.
fn arriving_in(directory: &Path, mut attachment: Attachment, resume: bool) -> Result<Arriving, Failure> {
    let directory = std::fs::canonicalize(directory).map_err(failure)?;
    let temporary = directory.join("upload.part");
    let kept = if resume { held(&temporary, attachment.file.size)? } else { None };
    let (output, received, hash) = match kept {
        Some(kept) => kept,
        None => {
            // A try that stopped before its end may have left its part.
            if let Err(error) = std::fs::remove_file(&temporary)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(failure(error));
            }
            let output = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(failure)?;
            (output, 0, Sha256::new())
        }
    };
    // The original name is metadata, never a path component. The image
    // extension also lets native hosts that dispatch by extension open it.
    let name = match attachment.file.media_type.as_str() {
        "image/png" => "content.png",
        "image/jpeg" => "content.jpg",
        "image/webp" => "content.webp",
        "image/gif" => "content.gif",
        "image/svg+xml" => "content.svg",
        _ => "content",
    };
    let name = if name == "content" {
        match Path::new(&attachment.file.name).extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase) {
            Some(ext)
                if matches!(
                    ext.as_str(),
                    "pdf" | "docx" | "pptx" | "xlsx" | "xls" | "xlsm" | "csv" | "tsv" | "md" | "txt" | "json" | "html"
                ) =>
            {
                format!("content.{ext}")
            }
            _ => name.to_owned(),
        }
    } else {
        name.to_owned()
    };
    let path = directory.join(name);
    attachment.path = crate::world::native(&path);
    let image_prefix = if attachment.is_image() && received == 0 { Some(Vec::new()) } else { None };
    if attachment.is_image() && received > 0 {
        let mut header = Vec::new();
        std::fs::File::open(&temporary)
            .map_err(failure)?
            .take(image_header_size(&attachment.file))
            .read_to_end(&mut header)
            .map_err(failure)?;
        check_image(&header, &attachment.file.media_type)?;
    }
    Ok(Arriving {
        attachment,
        temporary,
        path,
        output: Some(output),
        received,
        hash,
        unpublished: false,
        keep: false,
        image_prefix,
    })
}

/// The part at `temporary` an earlier try left, open to go on writing, with
/// its length and the digest of its bytes; none when there is none or it
/// is longer than the file.
fn held(temporary: &Path, size: u64) -> Result<Option<(std::fs::File, u64, Sha256)>, Failure> {
    let mut part = match std::fs::OpenOptions::new().read(true).append(true).open(temporary) {
        Ok(part) => part,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failure(error)),
    };
    let length = part.metadata().map_err(failure)?.len();
    if length > size {
        return Ok(None);
    }
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; UPLOAD_CHUNK];
    let mut read = 0u64;
    loop {
        let bytes = part.read(&mut buffer).map_err(failure)?;
        if bytes == 0 {
            break;
        }
        hash.update(&buffer[..bytes]);
        read += bytes as u64;
    }
    Ok((read == length).then_some((part, length, hash)))
}

struct Arriving {
    attachment: Attachment,
    temporary: PathBuf,
    path: PathBuf,
    output: Option<std::fs::File>,
    received: u64,
    hash: Sha256,
    unpublished: bool,
    /// Whether its part stays when it ends unfinished: a shared input's
    /// upload that broke goes on from it. Bytes that could never make the
    /// file are not kept.
    keep: bool,
    image_prefix: Option<Vec<u8>>,
}

fn image_header_size(file: &UploadFile) -> u64 {
    if file.media_type == "image/svg+xml" { 64 * 1024 } else { 16 }
}

fn check_image(header: &[u8], media_type: &str) -> Result<(), Failure> {
    let matches = if media_type == "image/svg+xml" {
        let mut xml = quick_xml::Reader::from_reader(header);
        loop {
            match xml.read_event() {
                Ok(quick_xml::events::Event::Start(tag) | quick_xml::events::Event::Empty(tag)) => {
                    break tag.local_name().as_ref() == "svg";
                }
                Ok(
                    quick_xml::events::Event::Decl(_)
                    | quick_xml::events::Event::Comment(_)
                    | quick_xml::events::Event::PI(_)
                    | quick_xml::events::Event::DocType(_),
                ) => {}
                Ok(quick_xml::events::Event::Text(text)) if text.as_ref().chars().all(char::is_whitespace) => {}
                _ => break false,
            }
        }
    } else {
        image::guess_format(header).is_ok_and(|format| format.to_mime_type() == media_type)
    };
    if !matches {
        return Err(failure("the attachment bytes do not match the declared image type"));
    }
    Ok(())
}

impl Arriving {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        if let Some(prefix) = self.image_prefix.as_mut() {
            prefix.extend_from_slice(bytes);
            if prefix.len() as u64 > self.attachment.file.size {
                self.keep = false;
                return Err(failure("the image exceeds its declared size"));
            }
            if (prefix.len() as u64) < image_header_size(&self.attachment.file).min(self.attachment.file.size) {
                return Ok(());
            }
            let prefix = self.image_prefix.take().expect("image header");
            if let Err(error) = check_image(&prefix, &self.attachment.file.media_type) {
                self.keep = false;
                return Err(error);
            }
            return self.write(&prefix);
        }
        if bytes.len() as u64 > self.attachment.file.size - self.received {
            self.keep = false;
            return Err(failure("the uploader sent more than the declared size"));
        }
        self.output.as_mut().expect("upload is open").write_all(bytes).map_err(failure)?;
        self.hash.update(bytes);
        self.received += bytes.len() as u64;
        Ok(())
    }

    fn finish(mut self) -> Result<Attachment, Failure> {
        self.keep = false;
        if self.image_prefix.is_some() {
            return Err(failure("the attachment has no complete image header"));
        }
        if self.received != self.attachment.file.size {
            return Err(failure(format!("received {} bytes; expected {}", self.received, self.attachment.file.size)));
        }
        if format!("{:x}", self.hash.clone().finalize()) != self.attachment.file.sha256 {
            return Err(failure("the file's SHA-256 does not match the uploaded bytes"));
        }
        self.output.take().expect("upload is open").sync_all().map_err(failure)?;
        if self.path.exists() {
            // 2026-10-20: a crash can publish the bytes before their receipt.
            // Recover only those exact bytes; never replace a changed original.
            if !holds(&self.attachment, &mut vec![0; UPLOAD_CHUNK])? {
                return Err(failure("the original at this upload's location has changed"));
            }
            std::fs::remove_file(&self.temporary).map_err(failure)?;
        } else {
            std::fs::rename(&self.temporary, &self.path).map_err(failure)?;
            self.unpublished = true;
        }
        write_new(&self.path.parent().expect("attachment directory").join("receipt.json"), &self.attachment)?;
        self.unpublished = false;
        Ok(self.attachment.clone())
    }
}

impl Drop for Arriving {
    fn drop(&mut self) {
        drop(self.output.take());
        if self.unpublished
            && let Err(error) = std::fs::remove_file(&self.path)
        {
            eprintln!("removing unpublished upload {}: {error}", self.path.display());
        }
        if !self.keep
            && let Err(error) = std::fs::remove_file(&self.temporary)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("removing incomplete upload {}: {error}", self.temporary.display());
        }
    }
}

/// Lets go of what arrived of the uploads of `resident` taken out of its
/// words before they finished: their parts, and the directories they leave
/// empty.
pub fn forsake(node: &Node, resident: &ResidentId, uploads: &[fastctx_types::NoticeId]) {
    let attachments = node.world.conversation(resident).attachments();
    for upload in uploads {
        let directory = attachments.join(upload.as_str());
        let removed = match std::fs::remove_file(directory.join("upload.part")) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => match std::fs::read_dir(&directory).map(|mut entries| entries.next().is_none()) {
                Ok(true) => std::fs::remove_dir(&directory),
                _ => Ok(()),
            },
        };
        if let Err(error) = removed {
            node.log.line(format_args!("letting go of the upload {upload} of {resident}: {error}"));
        }
    }
}

fn write_new(path: &std::path::Path, value: &impl Serialize) -> Result<(), Failure> {
    fastctx_platform::publish_new(
        path,
        &serde_json::to_vec(value).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)
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
            if references::reference(&node, &resident, &attachment)?.is_some() {
                continue;
            }
            let kept = kept_here(&node, &resident, &attachment)?;
            if !holds(&kept, &mut buffer)? {
                return Err(failure(format!("{} changed after it was attached", kept.file.name)));
            }
        }
        Ok(())
    })
    .await
    .map_err(failure)?
}

fn slot_name(attachment: &Attachment) -> Option<&str> {
    let name = Path::new(&attachment.path).parent()?.file_name()?.to_str()?;
    let id = attachment.id.as_str();
    (name == id || name.strip_prefix(id)?.strip_prefix('-')?.parse::<u32>().ok()? >= 2).then_some(name)
}

/// The upload kept here for `attachment` of `resident`: the one it names, or
/// the one a copy placed on another machine was made from, known by its id
/// and its file. What it gives says where the file is now.
fn kept_here(node: &Node, resident: &ResidentId, attachment: &Attachment) -> Result<Attachment, Failure> {
    let unknown = || failure(format!("{} was not attached in this conversation", attachment.file.name));
    if attachment.owner.resident != *resident {
        return Err(unknown());
    }
    let placed = !node.is_me(&attachment.owner.machine);
    // A work copy can occupy id-2 after an earlier copy changed. The path
    // supplies only a slot hint under this resident; stored equality below
    // still authenticates the whole receipt before any bytes are opened.
    let slot = if placed { attachment.id.as_str() } else { slot_name(attachment).unwrap_or(attachment.id.as_str()) };
    let load = |room: &ConversationDir| -> Result<Option<Attachment>, Failure> {
        match std::fs::read(room.attachments().join(slot).join("receipt.json")) {
            Ok(bytes) => serde_json::from_slice::<Attachment>(&bytes).map(Some).map_err(failure),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(failure(error)),
        }
    };
    let same = |kept: &Attachment| {
        if placed {
            kept.id == attachment.id && kept.file == attachment.file
        } else {
            kept.id == attachment.id
                && kept.owner == attachment.owner
                && kept.conversation == attachment.conversation
                && kept.file == attachment.file
        }
    };
    // A copy placed here sits in the guest room its path names; an upload
    // made here, or the original a copy elsewhere was made from, in the
    // conversation's home. One moved from an older layout may sit in any
    // room of this resident.
    let hinted = if placed { None } else { room_named(node, attachment) };
    let home = node.world.conversation(resident);
    let mut rooms: Vec<ConversationDir> = hinted.into_iter().chain([home]).collect();
    let mut found = None;
    for pass in 0..2 {
        for room in &rooms {
            if let Some(kept) = load(room)?.filter(&same) {
                found = Some((room.clone(), kept));
                break;
            }
        }
        if found.is_some() || pass == 1 {
            break;
        }
        rooms = node
            .world
            .kept()
            .map_err(failure)?
            .into_iter()
            .filter(|kept| kept.kind.resident() == resident)
            .map(|kept| kept.directory)
            .collect();
    }
    let (room, mut kept) = match found {
        Some(found) => found,
        None if rooms.iter().any(|room| load(room).ok().flatten().is_some()) => {
            return Err(failure("the attachment receipt differs from the stored upload"));
        }
        None => return Err(Failure::original_gone()),
    };
    let leaf = Path::new(&kept.path).file_name().ok_or_else(|| failure("the stored upload has no filename"))?;
    kept.path = crate::world::native(&room.attachments().join(slot).join(leaf));
    Ok(kept)
}

/// The room a copy placed on this machine names in its path: the guest room
/// of the conversation it was placed for, when its name says so.
fn room_named(node: &Node, attachment: &Attachment) -> Option<ConversationDir> {
    let room = Path::new(&attachment.path).parent()?.parent()?.parent()?.file_name()?.to_str()?;
    match Kind::of(room)? {
        Kind::Home(_) => None,
        kind => (*kind.resident() == attachment.owner.resident)
            .then(|| ConversationDir::at(&node.world.conversations().join(room))),
    }
}

/// Locate an immutable original from its recorded receipt, including when
/// the resident's directory was archived. No client path selects a file.
pub(crate) fn verified_original(
    node: &Node,
    attachment: &Attachment,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<(String, fastctx_platform::file_read::Snapshot), Failure> {
    let kept = kept_here(node, &attachment.owner.resident, attachment)?;
    let path = PathBuf::from(&kept.path);
    let file = fastctx_platform::file_read::open(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound { Failure::original_gone() } else { failure(error) }
    })?;
    let metadata = file.metadata().map_err(failure)?;
    if !metadata.is_file() || metadata.len() != kept.file.size {
        return Err(failure("the uploaded original changed"));
    }
    let before = fastctx_platform::file_read::snapshot(&file, &metadata).map_err(failure)?;
    let mut reading = (&file).take(before.size);
    let mut hash = Sha256::new();
    let mut buffer = vec![0; UPLOAD_CHUNK];
    let mut received = 0;
    loop {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(failure("attachment thumbnail was cancelled"));
        }
        let bytes = reading.read(&mut buffer).map_err(failure)?;
        if bytes == 0 {
            break;
        }
        hash.update(&buffer[..bytes]);
        received += bytes as u64;
    }
    let after = fastctx_platform::file_read::snapshot(&file, &file.metadata().map_err(failure)?).map_err(failure)?;
    if before != after || received != kept.file.size || format!("{:x}", hash.finalize()) != kept.file.sha256 {
        return Err(failure("the uploaded original changed"));
    }
    Ok((crate::world::native(&path), before))
}

/// Whether the file `kept` names still holds the bytes it was kept with.
fn holds(kept: &Attachment, buffer: &mut [u8]) -> Result<bool, Failure> {
    let mut file = std::fs::File::open(&kept.path).map_err(failure)?;
    let mut hash = Sha256::new();
    let mut size = 0u64;
    loop {
        let read = file.read(buffer).map_err(failure)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hash.update(&buffer[..read]);
    }
    Ok(size == kept.file.size && format!("{:x}", hash.finalize()) == kept.file.sha256)
}

/// What the machine a file is placed on says first.
#[derive(Serialize, Deserialize)]
struct Need {
    /// Whether it wants the bytes; when not, it holds that very file.
    bytes: bool,
}

/// Keeps original locations when submitting or resubmitting words. A file
/// no computer of the user's holds, sent from a phone, is kept here; for an
/// agent assisting the user, which cannot reach this computer, it goes to
/// the machine the user chose for files, all but images, which go to the
/// model as images (`world-run.md`, 脚本在哪台机器上跑).
pub async fn place(
    node: &Arc<Node>,
    resident: &ResidentId,
    attachments: Vec<Attachment>,
    files_on: Option<&Written>,
) -> Result<Vec<Attachment>, Failure> {
    let acting = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?.0.acting;
    let assisting = !crate::around::works_here(node, &acting);
    let mut given = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        if let Some(kept) = references::reference(node, resident, &attachment)? {
            if assisting && !kept.is_image() {
                destinations::validate_original(node, &acting, &kept)?;
            }
            given.push(kept);
            continue;
        }
        let kept = kept_here(node, resident, &attachment)?;
        given.push(match files_on {
            _ if !assisting || kept.is_image() => kept,
            Some(goes_to) if !node.is_here(goes_to) => send_placed(node, &acting, goes_to, resident, kept).await?,
            _ => {
                return Err(Failure::not_run(format!(
                    "{} needs one of your machines to go to: choose where your files go",
                    kept.file.name
                )));
            }
        });
    }
    Ok(given)
}

/// Places `kept`, an upload of `resident` kept here, on `goes_to`, where
/// the user chose, for whom it works; answers the copy there.
async fn send_placed(
    node: &Arc<Node>,
    acting: &Acting,
    goes_to: &Written,
    resident: &ResidentId,
    kept: Attachment,
) -> Result<Attachment, Failure> {
    let town = match node.speaking(acting, goes_to, Purpose::Hands) {
        Ok((link, _)) => node.town_across(&link, kept.town.as_ref())?,
        Err(_) => kept.town.clone(),
    };
    let call = RemoteCall::Place { resident: resident.clone(), attachment: Attachment { town, ..kept.clone() } };
    let (path, size) = (PathBuf::from(&kept.path), kept.file.size);
    let feed = || -> Feed {
        let path = path.clone();
        Box::new(move |mut stream: Stream| {
            Box::pin(async move {
                let need: Need = stream.hear_step("needed bytes").await?;
                if need.bytes {
                    send_bytes(&mut stream, &path, size).await?;
                    stream.say(&"placed").await?;
                }
                stream.finish().await
            })
        })
    };
    let refused =
        |why: String| Failure::new(format!("{} goes to {goes_to} and did not get there: {why}", kept.file.name));
    let mine = Address { machine: goes_to.name.clone(), resident: resident.clone() };
    match node.call_with_stream(acting, goes_to, Purpose::Hands, call, feed).await {
        Ok(Ok(RemoteReply::Uploaded(placed)))
            if placed.id == kept.id && placed.file == kept.file && placed.owner == mine =>
        {
            Ok(placed)
        }
        Ok(Ok(_)) => Err(refused(format!("{goes_to} answered something other than the file kept there"))),
        Ok(Err(failure)) => Err(refused(failure.message)),
        Err(failure) => Err(refused(failure.to_string())),
    }
}

/// Sends the `size` bytes of the file at `path`, and only those: a file
/// shorter than when it was checked is not sent short.
async fn send_bytes(stream: &mut Stream, path: &Path, size: u64) -> Result<(), String> {
    use tokio::io::AsyncReadExt as _;
    let mut file = tokio::fs::File::open(path).await.map_err(|error| format!("opening {}: {error}", path.display()))?;
    let mut buffer = vec![0u8; UPLOAD_CHUNK];
    let mut left = size;
    while left > 0 {
        let want = (buffer.len() as u64).min(left) as usize;
        let read =
            file.read(&mut buffer[..want]).await.map_err(|error| format!("reading {}: {error}", path.display()))?;
        if read == 0 {
            return Err(format!("{} is shorter than when it was attached", path.display()));
        }
        stream.send(&buffer[..read]).await?;
        left -= read as u64;
    }
    Ok(())
}

/// Keeps on this machine a file the user attached for `resident`, of
/// `from`, which works here, beside its other things here (`world-town.md`),
/// under the same id; asks for the bytes only when it does not hold that
/// very file already. A copy changed since, by whatever works here, is left
/// as it is, and this one is kept beside it.
pub async fn take_placed(
    node: &Arc<Node>,
    from: &MachineName,
    resident: ResidentId,
    attachment: Attachment,
    mut stream: Stream,
) -> Result<Attachment, Failure> {
    validate_file(&attachment.file)?;
    let origin = Address { machine: from.clone(), resident: resident.clone() };
    let _place = match crate::data::places::of_owner(node, &origin) {
        Some(place) => Some(node.places.using(&place).await),
        None => None,
    };
    if attachment.owner != origin {
        return Err(Failure::not_run(format!("{from} places files only of its own sessions")));
    }
    let base = node.world.owner(&node.names(), &origin).attachments();
    let wanted = Attachment { owner: Address { machine: node.machine(), resident }, ..attachment };
    let (directory, held) = {
        let wanted = wanted.clone();
        // Looking for the very file is work toward the answer the sender waits for.
        let looking = stream.settling();
        tokio::task::spawn_blocking(move || {
            let _looking = looking;
            slot(&base, &wanted)
        })
        .await
        .map_err(failure)??
    };
    if let Some(held) = held {
        stream.say(&Need { bytes: false }).await.map_err(failure)?;
        stream.done().await.map_err(failure)?;
        return Ok(held);
    }
    stream.say(&Need { bytes: true }).await.map_err(failure)?;
    let size = wanted.file.size;
    let arriving = arriving_in(&directory, wanted, false)?;
    let (arriving, taken) = stream
        .take_writing(size, arriving, |arriving, piece| arriving.write(piece).map_err(|error| error.message))
        .await
        .map_err(failure)?;
    taken.map_err(failure)?;
    let end: String = stream.hear_step("placed").await.map_err(failure)?;
    if end != "placed" {
        return Err(failure("the sender did not end the file"));
    }
    let settling = stream.settling();
    let placed = tokio::task::spawn_blocking(move || {
        let _settling = settling;
        arriving.finish()
    })
    .await
    .map_err(failure)??;
    stream.done().await.map_err(failure)?;
    Ok(placed)
}

/// Where under `base` the file `wanted` is kept: its id's directory, or the
/// first one after it, `<id>-2` and on, that is free, left from a try that
/// did not finish, or holds that very file, which is then given.
fn slot(base: &Path, wanted: &Attachment) -> Result<(PathBuf, Option<Attachment>), Failure> {
    std::fs::create_dir_all(base).map_err(failure)?;
    let mut buffer = vec![0u8; UPLOAD_CHUNK];
    for n in 1u32.. {
        let name = if n == 1 { wanted.id.as_str().to_string() } else { format!("{}-{n}", wanted.id.as_str()) };
        let directory = base.join(name);
        match std::fs::create_dir(&directory) {
            Ok(()) => return Ok((directory, None)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(failure(error)),
        }
        match read_json::<Attachment>(&directory.join("receipt.json")) {
            Ok(kept) if kept.id == wanted.id && kept.owner == wanted.owner && kept.file == wanted.file => {
                if holds(&kept, &mut buffer).unwrap_or(false) {
                    return Ok((directory, Some(kept)));
                }
            }
            Ok(_) => {}
            Err(_) if !directory.join("receipt.json").exists() => return Ok((directory, None)),
            Err(_) => {}
        }
    }
    unreachable!("some directory is free")
}

/// A draft reserved for one Open: the lock spans host creation, and once the
/// conversation is recorded its lock file goes with it.
pub struct Opening {
    lock: Option<FileLock>,
    directory: Option<ConversationDir>,
}

impl Drop for Opening {
    fn drop(&mut self) {
        drop(self.lock.take());
        if let Some(directory) = &self.directory
            && directory.about().exists()
            && let Err(error) = std::fs::remove_file(directory.draft_lock())
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("removing {}: {error}", directory.draft_lock().display());
        }
    }
}

/// Reserves a draft for precisely one Open. The lock spans host creation;
/// the ordinary resident record prevents reusing the draft after success.
pub async fn opening(
    node: &Arc<Node>,
    agent: &AgentName,
    acting: &Acting,
    town: Option<&TownName>,
    attachments: &[Attachment],
) -> Result<(ResidentId, Opening), Failure> {
    let Some(first) = attachments.first() else {
        return Ok((node.residents.new_id(agent)?, Opening { lock: None, directory: None }));
    };
    let id = first.conversation().resident.clone();
    if !node.is_me(&first.conversation().machine) || first.town.as_ref() != town {
        return Err(failure("the upload draft belongs to another machine or town"));
    }
    let directory = node.world.conversation(&id);
    let draft: Draft = read_json(&directory.draft())?;
    if draft.acting.as_ref() != Some(acting) {
        return Err(Failure::refused("the upload draft belongs to another account; attach the file again"));
    }
    if draft.agent != *agent || draft.town.as_ref() != town {
        return Err(failure("the upload draft is for another agent or town"));
    }
    let lock = FileLock::try_acquire(&directory.draft_lock())
        .map_err(failure)?
        .ok_or_else(|| failure("this upload draft is already being opened"))?;
    if node.residents.about(&id).is_some() {
        return Err(failure("this upload draft has already been opened"));
    }
    resolve(node, &id, attachments).await?;
    Ok((id, Opening { lock: Some(lock), directory: Some(directory) }))
}
