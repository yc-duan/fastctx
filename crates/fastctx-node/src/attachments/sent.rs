//! Keep upload bytes at their sending node and register only metadata at home.

use super::*;
use fastctx_types::composer::UploadScope;
use fastctx_types::thumbnail::Thumbnail;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Reservation {
    from: MachineName,
    #[serde(default)]
    acting: Option<Acting>,
    target: UploadTarget,
    operation: String,
    composer: Option<UploadScope>,
    attachment: Attachment,
}

fn put(path: &Path, value: &impl Serialize) -> Result<(), Failure> {
    fastctx_platform::publish(
        path,
        &serde_json::to_vec(value).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn reserve(
    node: &Arc<Node>,
    from: &MachineName,
    storage: &MachineName,
    acting: &Acting,
    target: UploadTarget,
    file: UploadFile,
    operation: String,
    composer: Option<UploadScope>,
) -> Result<Attachment, Failure> {
    validate_file(&file)?;
    let actor = destinations::acting(node, &target, acting)?;
    if !file.media_type.starts_with("image/") && !node.is_me(storage) && destinations::assisted(node, &target, &actor)?
    {
        let choices = destinations::places(node, from, &target, &actor, None)?;
        if !choices.machines.iter().any(|machine| machine.name == *storage) {
            return Err(Failure::not_run(
                "upgrade the sender to forward this file directly to the account's own machine",
            ));
        }
    }
    if operation.is_empty() || operation.len() > 256 || matches!(target, UploadTarget::Sent { .. }) {
        return Err(failure("an upload needs one stable operation and a conversation target"));
    }
    let (resident, town) = match &target {
        UploadTarget::Resident { resident } => {
            node.residents.active(resident)?;
            let (_, town) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
            (resident.clone(), town)
        }
        UploadTarget::New { agent, town } => {
            if !matches!(agent.as_str(), "codex" | "claude") {
                return Err(failure("the agent must be codex or claude"));
            }

            let digest = format!("{:x}", Sha256::digest(format!("{}\0{operation}", from.key())));
            let resident: ResidentId = format!("{agent}-{}", &digest[..32]).parse().map_err(failure)?;
            let room = node.world.conversation(&resident);
            fastctx_platform::ensure_private_dir(room.path()).map_err(failure)?;
            let draft = Draft {
                agent: agent.clone(),
                town: town.clone(),
                from: Some(from.clone()),
                acting: Some(acting.clone()),
            };
            match read_json::<Draft>(&room.draft()) {
                Ok(kept) if kept == draft => {}
                Ok(_) => return Err(failure("that upload operation belongs to a different draft")),
                Err(_) if !room.draft().exists() => write_new(&room.draft(), &draft)?,
                Err(error) => return Err(error),
            }
            (resident, town.clone())
        }
        UploadTarget::Draft { resident, town } => {
            let draft: Draft = read_json(&node.world.conversation(resident).draft())?;
            if draft.town != *town || draft.from.as_ref() != Some(from) || draft.acting.as_ref() != Some(acting) {
                return Err(failure("this upload draft belongs to another device or project"));
            }
            (resident.clone(), town.clone())
        }
        UploadTarget::Sent { .. } => unreachable!(),
    };
    let _place = node.places.using(resident.as_str()).await;
    if node.residents.deleted(&resident).is_some() {
        return Err(crate::forget::gone(node, &resident));
    }
    let id = crate::speech::fixed_id("attachment", &[from.as_str(), &operation]);
    let directory = node.world.conversation(&resident).attachments().join(id.as_str());
    fastctx_platform::ensure_private_dir(&directory).map_err(failure)?;
    let _lock = FileLock::try_acquire(&directory.join("reservation.lock"))
        .map_err(failure)?
        .ok_or_else(|| Failure::unanswered("this upload is being reserved; retry the same operation"))?;
    let attachment = Attachment {
        id,
        owner: Address { machine: storage.clone(), resident: resident.clone() },
        conversation: Some(Box::new(Address { machine: node.machine(), resident: resident.clone() })),
        town,
        path: String::new(),
        file,
    };
    let reservation =
        Reservation { from: from.clone(), acting: Some(acting.clone()), target, operation, composer, attachment };
    let path = directory.join("reservation.json");
    if path.exists() {
        let kept: Reservation = read_json(&path)?;
        if kept != reservation {
            return Err(failure("the upload operation was reused for different content"));
        }
    } else {
        write_new(&path, &reservation)?;
    }
    if let Some(scope) = &reservation.composer {
        let mut guard = crate::composer::uploads::begin(node, &resident, from, scope, &reservation.attachment).await?;
        guard.release();
    }
    Ok(reservation.attachment)
}

pub(crate) async fn commit(
    node: &Arc<Node>,
    from: &MachineName,
    acting: &Acting,
    attachment: Attachment,
    preview: Option<Thumbnail>,
) -> Result<Uploaded, Failure> {
    let _place = node.places.using(attachment.conversation().resident.as_str()).await;
    commit_held(node, from, acting, attachment, preview).await
}

async fn commit_held(
    node: &Arc<Node>,
    from: &MachineName,
    acting: &Acting,
    attachment: Attachment,
    preview: Option<Thumbnail>,
) -> Result<Uploaded, Failure> {
    let home = attachment.conversation();
    if !node.is_me(&home.machine) {
        return Err(failure("the attachment belongs to another conversation machine"));
    }
    let resident = home.resident.clone();
    if node.residents.deleted(&resident).is_some() {
        return Err(crate::forget::gone(node, &resident));
    }
    let directory = node.world.conversation(&resident).attachments().join(attachment.id.as_str());
    let reservation: Reservation = read_json(&directory.join("reservation.json"))?;
    let expected = &reservation.attachment;
    // 2026-10-20: only the authenticated uploader may complete its own reservation.
    if reservation.from != *from
        || reservation.acting.as_ref() != Some(acting)
        || expected.id != attachment.id
        || expected.owner != attachment.owner
        || expected.conversation != attachment.conversation
        || expected.town != attachment.town
        || expected.file != attachment.file
        || attachment.path.is_empty()
    {
        return Err(failure("the uploaded receipt differs from its reservation"));
    }
    // A remote image comes with its small picture for history, unless it is
    // of a kind the thumbnails cannot read, and history has none of it.
    if let Some(preview) = &preview {
        validate_preview(preview, &attachment)?;
        put(&directory.join("preview.json"), preview)?;
    }
    if !node.is_me(&attachment.owner.machine) {
        put(&directory.join("reference.json"), &attachment)?;
    }
    let composer = match &reservation.composer {
        Some(scope) => Some(crate::composer::uploads::complete(node, &resident, scope, &attachment).await),
        None => None,
    };
    Ok(Uploaded { attachment, composer })
}

fn validate_preview(preview: &Thumbnail, attachment: &Attachment) -> Result<(), Failure> {
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.decode(&preview.base64).map_err(failure)?;
    if data.len() > 64 * 1024
        || preview.width > 320
        || preview.height > 320
        || !preview
            .content
            .as_ref()
            .is_some_and(|content| content.sha256 == attachment.file.sha256 && content.size == attachment.file.size)
        || !matches!(fastctx_hands::image::sniff(&data), Some((_, w, h)) if w == preview.width && h == preview.height)
    {
        return Err(failure("the history preview is not a small image of this attachment"));
    }
    Ok(())
}

struct Prepared {
    acting: Acting,
    home: Written,
    attachment: Attachment,
    arriving: Option<Arriving>,
    _lock: FileLock,
    _place: crate::data::places::Using,
}

#[allow(clippy::too_many_arguments)]
async fn prepare_sent(
    node: &Arc<Node>,
    from: &MachineName,
    acting: &Acting,
    machine: &Written,
    target: UploadTarget,
    file: UploadFile,
    operation: String,
    composer: Option<UploadScope>,
) -> Result<Prepared, Failure> {
    let attachment = if node.is_here(machine) {
        reserve(node, from, &node.machine(), acting, target, file, operation, composer).await?
    } else {
        let call = RemoteCall::ReserveAttachment { target, file, operation, composer };
        match node.call(acting, machine, Purpose::Agents, call).await.map_err(broke)?? {
            RemoteReply::Uploaded(attachment)
                if attachment.owner.machine == node.machine()
                    && node
                        .locate(machine)?
                        .iter()
                        .any(|link| link.view().same(&attachment.conversation().machine, &machine.name)) =>
            {
                attachment
            }
            _ => return Err(failure("the conversation machine returned an invalid reservation")),
        }
    };
    let owner = attachment.conversation();
    let room = node.world.owner(&node.names(), owner);
    let place = node.places.using(room.path().file_name().expect("room").to_str().expect("room name")).await;
    let directory = room.attachments().join(attachment.id.as_str());
    fastctx_platform::ensure_private_dir(&directory).map_err(failure)?;
    let lock = FileLock::try_acquire(&directory.join("upload.lock"))
        .map_err(failure)?
        .ok_or_else(|| Failure::unanswered("this upload is still arriving; retry the same operation"))?;
    let receipt = directory.join("receipt.json");
    let (attachment, arriving) = if receipt.exists() {
        let kept: Attachment = read_json(&receipt)?;
        if kept.owner != attachment.owner
            || kept.conversation != attachment.conversation
            || kept.file != attachment.file
        {
            return Err(failure("the stored original differs from the reservation"));
        }
        verified_original(node, &kept, &Default::default())?;
        (kept, None)
    } else {
        let mut arriving = arriving_in(&directory, attachment, true)?;
        arriving.keep = true;
        (arriving.attachment.clone(), Some(arriving))
    };
    Ok(Prepared { acting: acting.clone(), home: machine.clone(), attachment, arriving, _lock: lock, _place: place })
}

async fn complete_sent(node: &Arc<Node>, from: &MachineName, prepared: Prepared) -> Result<Uploaded, Failure> {
    let attachment = match prepared.arriving {
        Some(arriving) => arriving.finish()?,
        None => prepared.attachment,
    };
    let home = &prepared.home;
    // A picture of a kind the thumbnails cannot read still goes, without one.
    let preview = if attachment.is_image() && !node.is_here(home) {
        match crate::thumbnail::history(node, attachment.clone()).await {
            Ok(preview) => Some(preview),
            Err(error) => {
                node.log.line(format_args!("no small picture of an attachment kept elsewhere: {error}"));
                None
            }
        }
    } else {
        None
    };
    if node.is_here(home) {
        return commit_held(node, from, &prepared.acting, attachment, preview).await;
    }
    let call = RemoteCall::CommitAttachment { attachment: attachment.clone(), preview };
    match node.call(&prepared.acting, home, Purpose::Agents, call).await.map_err(broke)?? {
        RemoteReply::Uploaded(attachment) => Ok(Uploaded { attachment, composer: None }),
        RemoteReply::ComposerUploaded(result) => {
            Ok(Uploaded { attachment: result.attachment, composer: Some(result.updated) })
        }
        _ => Err(failure("the conversation machine did not confirm its attachment")),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn upload(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Written,
    target: UploadTarget,
    operation: String,
    file: UploadFile,
    composer: Option<UploadScope>,
    mut source: FrameReader,
) -> Result<Uploaded, Failure> {
    let from = node.machine();
    if node.is_here(&machine) {
        check_project(node, acting, &target).await?;
    }
    let mut prepared = prepare_sent(node, &from, acting, &machine, target, file.clone(), operation, composer).await?;
    if prepared.arriving.is_none() {
        return complete_sent(node, &from, prepared).await;
    }
    let arriving = prepared.arriving.as_mut().expect("unfinished");
    let mut skipped = Skipping(arriving.received);
    let mut sent = 0u64;
    loop {
        let piece = source
            .recv_up_to(UPLOAD_CHUNK)
            .await
            .map_err(broke)?
            .ok_or_else(|| broke("the uploader disconnected before ending the file"))?;
        if piece.is_empty() {
            break;
        }
        sent = sent.checked_add(piece.len() as u64).ok_or_else(|| failure("upload length overflow"))?;
        if sent > file.size {
            return Err(failure("the source exceeded its declared length"));
        }
        arriving.write(skipped.past(&piece))?;
    }
    if sent != file.size {
        return Err(broke("the source ended before its declared length"));
    }
    complete_sent(node, &from, prepared).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn receive(
    node: &Arc<Node>,
    from: &MachineName,
    acting: &Acting,
    machine: Written,
    target: UploadTarget,
    operation: String,
    file: UploadFile,
    composer: Option<UploadScope>,
    resumes: bool,
    mut stream: Stream,
) -> Result<Uploaded, Failure> {
    if !node.is_here(&machine) {
        return Err(failure("a device uploads to the conversation's machine"));
    }
    let mut prepared = prepare_sent(node, from, acting, &machine, target, file.clone(), operation, composer).await?;
    if prepared.arriving.is_none() {
        return complete_sent(node, from, prepared).await;
    }
    let mut arriving = prepared.arriving.take().expect("unfinished");
    if resumes {
        stream.say(&UploadStart { from: arriving.received }).await.map_err(broke)?;
    }
    let skip = Skipping(if resumes { 0 } else { arriving.received });
    let wanted = if resumes { file.size - arriving.received } else { file.size };
    let ((back, _), taken) = stream
        .take_writing(wanted, (arriving, skip), |(arriving, skip), bytes| {
            arriving.write(skip.past(bytes)).map_err(|e| e.message)
        })
        .await
        .map_err(broke)?;
    arriving = back;
    taken.map_err(broke)?;
    let end: String = stream.hear_step("uploaded").await.map_err(broke)?;
    if end != "uploaded" {
        return Err(failure("the uploader did not end the file"));
    }
    prepared.arriving = Some(arriving);
    let uploaded = complete_sent(node, from, prepared).await?;
    stream.done().await.map_err(broke)?;
    Ok(uploaded)
}
