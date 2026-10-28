//! Legacy upload receipts can name home while the original lives elsewhere.
//! Only these small mappings are kept at home; the file stream never touches disk.

use super::*;

#[derive(Clone, Serialize, Deserialize)]
struct Forwarded {
    shown: Attachment,
    original: Attachment,
    acting: Acting,
}

pub(super) enum Input {
    Local(FrameReader),
    Remote(Stream, bool),
}

impl Input {
    async fn transfer(self, mut destination: Stream, size: u64) -> Result<(), String> {
        let need: Need = destination.hear_step("needed bytes").await?;
        match self {
            Self::Remote(mut source, resumes) => {
                if resumes {
                    source.say(&UploadStart { from: 0 }).await?;
                }
                if need.bytes {
                    source.forward(&mut destination, size).await?;
                } else {
                    source.take(size, |_| Ok(())).await?;
                }
                if source.hear_step::<String>("uploaded").await? != "uploaded" {
                    return Err("the uploader did not end the file".into());
                }
                if need.bytes {
                    destination.say(&"placed").await?;
                }
                destination.finish().await?;
                source.done().await
            }
            Self::Local(mut source) => {
                let mut received = 0u64;
                loop {
                    let bytes = source
                        .recv_up_to(UPLOAD_CHUNK)
                        .await
                        .map_err(|error| error.to_string())?
                        .ok_or("the uploader disconnected before ending the file")?;
                    if bytes.is_empty() {
                        break;
                    }
                    received = received.checked_add(bytes.len() as u64).ok_or("upload length overflow")?;
                    if received > size {
                        return Err("the upload exceeds its declared size".into());
                    }
                    if need.bytes {
                        destination.send(&bytes).await?;
                    }
                }
                if received != size {
                    return Err("the upload ended before its declared size".into());
                }
                if need.bytes {
                    destination.say(&"placed").await?;
                }
                destination.finish().await
            }
        }
    }
}

fn mapping(node: &Node, attachment: &Attachment) -> PathBuf {
    node.world
        .conversation(&attachment.conversation().resident)
        .attachments()
        .join(attachment.id.as_str())
        .join("forwarded.json")
}

pub(crate) fn original(node: &Node, wanted: &Attachment) -> Result<Option<(Attachment, Acting)>, Failure> {
    if !node.is_me(&wanted.owner.machine) {
        return Ok(None);
    }
    let Some(kept) = references::optional::<Forwarded>(&mapping(node, wanted))? else { return Ok(None) };
    if kept.shown != *wanted {
        return Err(failure("the forwarded attachment differs from its receipt"));
    }
    destinations::validate_original(node, &kept.acting, &kept.original)?;
    Ok(Some((kept.original, kept.acting)))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn upload(
    node: &Arc<Node>,
    from: &MachineName,
    acting: Acting,
    target: UploadTarget,
    operation: String,
    file: UploadFile,
    composer: Option<fastctx_types::composer::UploadScope>,
    preferred: Option<Written>,
    input: Input,
) -> Result<Uploaded, Failure> {
    let preferred = match (&composer, target.conversation_target()) {
        (Some(scope), UploadTarget::Resident { resident }) if preferred.is_none() => {
            crate::composer::uploads::files_on(node, resident, scope).await?
        }
        _ => preferred,
    };
    let choices = destinations::places(node, from, &target, &acting, preferred.as_ref())?;
    let target_machine = choices.selected.ok_or_else(|| {
        Failure::not_run(if choices.images_only {
            "only text and images can be attached until this account has an online machine for files"
        } else {
            "choose which of this account's machines receives the file before uploading"
        })
    })?;
    let expected = sent::reserve(node, from, &node.machine(), &acting, target, file, operation, composer).await?;
    let resident = expected.conversation().resident.clone();
    let directory = node.world.conversation(&resident).attachments().join(expected.id.as_str());
    let choice = directory.join("destination.json");
    let _lock = FileLock::try_acquire(&directory.join("forward.lock"))
        .map_err(failure)?
        .ok_or_else(|| Failure::unanswered("this attachment is being forwarded; retry the same operation"))?;
    let selected = (target_machine.clone(), acting.clone());
    match references::optional::<(Written, Acting)>(&choice)? {
        Some(kept) if kept != selected => {
            return Err(failure("this upload was already assigned to another destination or account"));
        }
        Some(_) => {}
        None => write_new(&choice, &selected)?,
    }
    if let Acting::Accounts { accounts } = &acting {
        for account in accounts {
            if let Some(link) = node.link_of(&account.hub)
                && !link.view().may_act_for(&account.account, &link.view().me)
            {
                link.refresh().await.map_err(failure)?;
            }
        }
    }
    let (link, account) = node.speaking(&acting, &target_machine, Purpose::Hands).map_err(|error| Failure::not_run(
        format!("cannot place this account's file on its machine: {error}; the hub must authorize this conversation before forwarding files")))?;
    let mut offered = expected.clone();
    offered.town = node.town_across(&link, expected.town.as_ref())?;
    let size = offered.file.size;
    let error = Arc::new(Mutex::new(None));
    let stream_error = error.clone();
    let feed: Feed = Box::new(move |stream| {
        Box::pin(async move {
            if let Err(why) = input.transfer(stream, size).await {
                *stream_error.lock().expect("never poisoned") = Some(why);
            }
            Ok(())
        })
    });
    let result = link
        .call_with_stream(
            &target_machine.name,
            &account,
            Purpose::Hands,
            RemoteCall::Place { resident, attachment: offered },
            feed,
        )
        .await;
    let original = match result {
        Ok(Ok(RemoteReply::Uploaded(receipt)))
            if receipt.id == expected.id
                && receipt.file == expected.file
                && receipt.owner.machine == target_machine.name =>
        {
            receipt
        }
        Ok(Ok(_)) => return Err(failure("the destination returned an invalid file receipt")),
        Ok(Err(error)) => return Err(error),
        Err(why) => {
            return Err(broke(format!("{why}; {}", error.lock().expect("never poisoned").as_deref().unwrap_or(""))));
        }
    };
    destinations::validate_original(node, &acting, &original)?;
    // The old sender requires its upload target as owner. Resolve this alias
    // before the file enters conversation history or a later original read.
    let mut shown = expected;
    shown.path = original.path.clone();
    let kept = Forwarded { shown: shown.clone(), original: original.clone(), acting: acting.clone() };
    fastctx_platform::publish(
        &mapping(node, &shown),
        &serde_json::to_vec(&kept).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)?;
    fastctx_platform::publish(
        &directory.join("reference.json"),
        &serde_json::to_vec(&original).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)?;
    sent::commit(node, from, &acting, shown, None).await
}
