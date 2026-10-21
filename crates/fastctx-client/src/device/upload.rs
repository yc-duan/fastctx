//! Uploads retain the caller's reader and use one queued data block. The
//! signed destination receipt is authoritative; an uncertain write is never
//! repeated through a relay or another machine. A shared input's upload that
//! broke is made again by its caller under the same operation, and goes on
//! from what its destination holds: the reader gives the file whole, and
//! what is held is skipped.

use fastctx_types::access::Written;
use std::sync::{Arc, Mutex};

use fastctx_net::link::Feed;
use fastctx_types::MachineName;
use fastctx_types::attachments::{UPLOAD_CHUNK, UploadFile, UploadStart, UploadTarget, Uploaded};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::mpsc;

use super::{Device, failed};
use crate::Error;

impl Device {
    pub(crate) async fn upload_scoped(
        &self,
        mut target: UploadTarget,
        machine: Option<Written>,
        file: UploadFile,
        composer: Option<fastctx_types::composer::UploadScope>,
        mut source: impl AsyncRead + Unpin,
    ) -> Result<Uploaded, Error> {
        let machine = match &target {
            UploadTarget::Sent { machine, .. } => machine.clone(),
            UploadTarget::Resident { resident } => self.find(resident, machine).await?,
            _ => machine.ok_or_else(|| failed("choose a machine for the attachment; a device has no local node"))?,
        };
        let session = self.reaching(&machine)?;
        let machine = machine.name;
        if let UploadTarget::Sent { machine: destination, .. } = &mut target {
            *destination = machine.clone().into();
        }
        let account = match target.conversation_target() {
            UploadTarget::Resident { resident } => session.link.account_of(&machine, resident).map_err(failed)?,
            _ => session.account_for(&machine, fastctx_types::access::Purpose::Agents)?,
        };
        let resumes = (composer.is_some() || matches!(target, UploadTarget::Sent { .. }))
            && resumes_there(&session, &machine).await?;
        let size = file.size;
        let (sender, mut receiver) = mpsc::channel::<Option<Vec<u8>>>(1);
        let stream_error = Arc::new(Mutex::new(None));
        let feeding_error = stream_error.clone();
        let feed: Feed = Box::new(move |mut stream| {
            Box::pin(async move {
                let sending = async {
                    let mut held = if resumes { stream.hear::<UploadStart>().await?.from } else { 0 };
                    while let Some(bytes) = receiver.recv().await {
                        match bytes {
                            Some(bytes) => {
                                let skipped = held.min(bytes.len() as u64);
                                held -= skipped;
                                if (skipped as usize) < bytes.len() {
                                    stream.send(&bytes[skipped as usize..]).await?;
                                }
                            }
                            None => {
                                stream.say(&"uploaded").await?;
                                return stream.finish().await;
                            }
                        }
                    }
                    Err("the uploader stopped before ending the file".to_string())
                }
                .await;
                if let Err(error) = sending {
                    *feeding_error.lock().expect("never poisoned") = Some(error);
                }
                // The destination can reject the digest or run out of disk while
                // the stream closes. Preserve that signed explanation.
                Ok(())
            })
        });
        let remote = RemoteCall::Upload { target: target.clone(), file: file.clone(), composer, resumes };
        let answer =
            session.link.call_with_stream(&machine, &account, fastctx_types::access::Purpose::Agents, remote, feed);
        let sending = async {
            let mut buffer = vec![0u8; UPLOAD_CHUNK];
            let mut sent = 0u64;
            loop {
                let read = source
                    .read(&mut buffer)
                    .await
                    .map_err(|error| failed(format!("reading upload source: {error}")))?;
                if read == 0 {
                    if sent != size {
                        return Err(failed(format!("upload source ended at {sent} bytes, expected {size}")));
                    }
                    return Ok(sender.send(None).await.is_ok());
                }
                if read as u64 > size - sent {
                    return Err(failed(format!("upload source exceeds its declared {size} bytes")));
                }
                sent += read as u64;
                if sender.send(Some(buffer[..read].to_vec())).await.is_err() {
                    return Ok(false);
                }
            }
        };
        tokio::pin!(answer, sending);
        let answered = tokio::select! {
            answer = &mut answer => answer,
            sent = &mut sending => {
                sent?;
                answer.await
            }
        };
        let reply = match answered {
            Ok(Ok(reply)) => reply,
            error => {
                // A refusal keeps what it says of itself; a call that got
                // no answer is made again.
                let failure = match error {
                    Ok(Err(error)) => error,
                    Err(error) => Failure::unanswered(error.to_string()),
                    Ok(Ok(_)) => unreachable!(),
                };
                let failure = match stream_error.lock().expect("never poisoned").take() {
                    Some(error) => {
                        Failure { message: format!("{}; source stream: {error}", failure.message), ..failure }
                    }
                    None => failure,
                };
                return Err(crate::CallError::Failed(failure).into());
            }
        };
        let uploaded = match reply {
            RemoteReply::Uploaded(attachment) => Uploaded { attachment, composer: None },
            RemoteReply::ComposerUploaded(result) => {
                Uploaded { attachment: result.attachment, composer: Some(result.updated) }
            }
            _ => return Err(failed(format!("{machine} answered something other than an upload"))),
        };
        let attachment = &uploaded.attachment;
        let owner_matches = match target.conversation_target() {
            UploadTarget::Sent { .. } => false,
            UploadTarget::Resident { resident } => attachment.owner.resident == *resident,
            UploadTarget::Draft { resident, town } => {
                attachment.owner.resident == *resident && attachment.town == *town
            }
            UploadTarget::New { town, .. } => attachment.town == *town,
        };
        if attachment.owner.machine != machine || attachment.file != file || !owner_matches {
            return Err(failed("the upload receipt differs from the requested file or destination"));
        }
        Ok(uploaded)
    }
}

/// Whether `machine` keeps what arrived of an upload that broke, and takes
/// it up from there. A machine that cannot be asked cannot take the upload
/// either.
async fn resumes_there(session: &super::Session, machine: &MachineName) -> Result<bool, Error> {
    let Ok(account) = session.account_for(machine, fastctx_types::access::Purpose::Look) else { return Ok(false) };
    match session.link.call(machine, &account, fastctx_types::access::Purpose::Look, RemoteCall::Facts).await {
        Ok(Ok(RemoteReply::Facts(facts))) => Ok(facts.upload_resumes),
        Ok(_) => Ok(false),
        Err(error) => Err(crate::CallError::Failed(Failure::unanswered(error.to_string())).into()),
    }
}
