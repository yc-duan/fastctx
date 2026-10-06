//! Uploads retain the caller's reader and use one queued data block. The
//! signed destination receipt is authoritative; an uncertain write is never
//! repeated through a relay or another machine.

use std::sync::{Arc, Mutex};

use fastctx_net::link::Feed;
use fastctx_types::MachineName;
use fastctx_types::attachments::{UPLOAD_CHUNK, UploadFile, UploadTarget, Uploaded};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::mpsc;

use super::{Device, failed};
use crate::Error;

impl Device {
    pub(crate) async fn upload_scoped(
        &self,
        target: UploadTarget,
        machine: Option<MachineName>,
        file: UploadFile,
        composer: Option<fastctx_types::composer::UploadScope>,
        mut source: impl AsyncRead + Unpin,
    ) -> Result<Uploaded, Error> {
        let session = self.connected()?;
        session.check_identity().await?;
        let machine = match &target {
            UploadTarget::Resident { resident } => session.locate(resident, machine).await?,
            _ => machine.ok_or_else(|| failed("choose a machine for the attachment; a device has no local node"))?,
        };
        session.machine(&machine)?;
        let size = file.size;
        let (sender, mut receiver) = mpsc::channel::<Option<Vec<u8>>>(1);
        let stream_error = Arc::new(Mutex::new(None));
        let feeding_error = stream_error.clone();
        let feed: Feed = Box::new(move |mut stream| {
            Box::pin(async move {
                let sending = async {
                    while let Some(bytes) = receiver.recv().await {
                        match bytes {
                            Some(bytes) => stream.send(&bytes).await?,
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
        let remote = RemoteCall::Upload { target: target.clone(), file: file.clone(), composer };
        let answer = session.link.call_with_stream(&machine, remote, feed);
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
                let reason = match error {
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => error.to_string(),
                    Ok(Ok(_)) => unreachable!(),
                };
                return Err(failed(match stream_error.lock().expect("never poisoned").take() {
                    Some(error) => format!("{reason}; source stream: {error}"),
                    None => reason,
                }));
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
        let owner_matches = match target {
            UploadTarget::Resident { resident } => attachment.owner.resident == resident,
            UploadTarget::Draft { resident, town } => attachment.owner.resident == resident && attachment.town == town,
            UploadTarget::New { town, .. } => attachment.town == town,
        };
        if attachment.owner.machine != machine || attachment.file != file || !owner_matches {
            return Err(failed("the upload receipt differs from the requested file or destination"));
        }
        Ok(uploaded)
    }
}
