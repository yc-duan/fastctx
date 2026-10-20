//! Streaming uploads use a dedicated IPC connection, just as watches do.

use fastctx_types::attachments::{Attachment, UPLOAD_CHUNK, UploadFile, UploadTarget, Uploaded};
use fastctx_types::ipc::{ClientFrame, NodeFrame};
use fastctx_types::{Call, MachineName, Reply};
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::{CallError, Client, Error, Said, World};

enum SendingError {
    Source(CallError),
    Connection(std::io::Error),
}

impl Client {
    /// Sends the original bytes without an intermediate file. The reader
    /// must end at the declared size; an empty IPC frame commits that end.
    /// Dropping this future closes the connection and abandons the upload.
    pub async fn upload(
        self,
        target: UploadTarget,
        node: Option<MachineName>,
        file: UploadFile,
        source: impl AsyncRead + Unpin,
    ) -> Result<Attachment, CallError> {
        self.upload_scoped(target, node, file, None, source).await.map(|uploaded| uploaded.attachment)
    }

    pub(crate) async fn upload_scoped(
        mut self,
        target: UploadTarget,
        node: Option<MachineName>,
        file: UploadFile,
        composer: Option<fastctx_types::composer::UploadScope>,
        mut source: impl AsyncRead + Unpin,
    ) -> Result<Uploaded, CallError> {
        self.next += 1;
        let id = self.next;
        let size = file.size;
        let call = Call::Upload { target, node, file, composer };
        self.connection.send(&crate::to_json(&ClientFrame::Call { id, call })).await.map_err(CallError::Broken)?;
        let (mut reader, mut writer) = self.connection.split();
        let send = async {
            let mut buffer = vec![0u8; UPLOAD_CHUNK];
            let mut sent = 0u64;
            loop {
                let read =
                    source.read(&mut buffer).await.map_err(|error| SendingError::Source(CallError::Broken(error)))?;
                if read == 0 {
                    if sent != size {
                        return Err(SendingError::Source(broken(format!(
                            "upload source ended at {sent} bytes, expected {size}"
                        ))));
                    }
                    writer.send(&[]).await.map_err(SendingError::Connection)?;
                    return Ok(());
                }
                if read as u64 > size - sent {
                    return Err(SendingError::Source(broken(format!(
                        "upload source exceeds its declared {size} bytes"
                    ))));
                }
                writer.send(&buffer[..read]).await.map_err(SendingError::Connection)?;
                sent += read as u64;
            }
        };
        let answer = async {
            let frame = reader
                .recv()
                .await
                .map_err(CallError::Broken)?
                .ok_or_else(|| broken("the node disconnected during upload"))?;
            match serde_json::from_slice::<NodeFrame>(&frame).map_err(|error| broken(error.to_string()))? {
                NodeFrame::Reply { id: answered, reply } if answered == id => match reply.map_err(CallError::Failed)? {
                    Reply::Uploaded(attachment) => Ok(Uploaded { attachment, composer: None }),
                    Reply::ComposerUploaded(result) => {
                        Ok(Uploaded { attachment: result.attachment, composer: Some(result.updated) })
                    }
                    _ => Err(broken("the node answered something other than an upload")),
                },
                _ => Err(broken("the node answered another call during upload")),
            }
        };
        // A destination may fail before consuming any bytes (authorization,
        // disk full). Observe its answer while writes still have backpressure.
        tokio::pin!(send, answer);
        let sent = tokio::select! {
            result = &mut answer => return result,
            sent = &mut send => sent,
        };
        match sent {
            Ok(()) => answer.await,
            Err(SendingError::Source(error)) => Err(error),
            Err(SendingError::Connection(error)) if disconnected(&error) => {
                // On Unix the node can send ENOSPC and close while a write
                // is pending. Its reply remains buffered on the read side;
                // that explicit outcome takes precedence over EPIPE.
                match answer.await {
                    Ok(attachment) => Ok(attachment),
                    Err(error @ CallError::Failed(_)) => Err(error),
                    Err(CallError::Broken(_)) => Err(CallError::Broken(error)),
                }
            }
            Err(SendingError::Connection(error)) => Err(CallError::Broken(error)),
        }
    }
}

fn broken(reason: impl Into<String>) -> CallError {
    CallError::Broken(std::io::Error::other(reason.into()))
}

fn disconnected(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        error.kind(),
        ErrorKind::BrokenPipe
            | ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::NotConnected
            | ErrorKind::UnexpectedEof
            | ErrorKind::WriteZero
    )
}

impl World {
    pub async fn upload(
        &self,
        target: UploadTarget,
        node: Option<MachineName>,
        file: UploadFile,
        source: impl AsyncRead + Unpin,
    ) -> Result<Attachment, Error> {
        if let Some(device) = self.device() {
            return device.upload_scoped(target, node, file, None, source).await.map(|uploaded| uploaded.attachment);
        }
        Ok(self.connect().await?.upload(target, node, file, source).await?)
    }

    /// Uploads and attaches to the shared draft as one node-owned lifetime.
    pub async fn upload_composer(
        &self,
        resident: fastctx_types::ResidentId,
        node: Option<MachineName>,
        file: UploadFile,
        scope: fastctx_types::composer::UploadScope,
        source: impl AsyncRead + Unpin,
    ) -> Result<fastctx_types::composer::UploadResult, Error> {
        let target = UploadTarget::Resident { resident };
        let target = match &node {
            Some(machine) => UploadTarget::Sent {
                machine: machine.clone(),
                target: Box::new(target),
                operation: scope.operation.clone(),
            },
            None => target,
        };
        let uploaded = match self.device() {
            Some(device) => device.upload_scoped(target, node, file, Some(scope), source).await?,
            None => self.connect().await?.upload_scoped(target, node, file, Some(scope), source).await?,
        };
        let updated = uploaded
            .composer
            .ok_or_else(|| Error::Unexpected("the upload returned no shared-composer result".into()))?;
        Ok(fastctx_types::composer::UploadResult { attachment: uploaded.attachment, updated })
    }

    pub async fn talk_with_attachments(
        &self,
        to: fastctx_types::ResidentId,
        machine: Option<MachineName>,
        text: String,
        attachments: Vec<Attachment>,
        files_on: Option<fastctx_types::access::Written>,
        id: Option<String>,
    ) -> Result<Said, Error> {
        match self.call(Call::Talk { to, machine, text, attachments, files_on, id }).await?.0 {
            Reply::Spoken { to, landed, delivery } => Ok(Said { to, landed, delivery }),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn open_with_attachments(
        &self,
        agent: fastctx_types::AgentName,
        town: Option<fastctx_types::TownName>,
        node: Option<MachineName>,
        text: String,
        options: fastctx_types::agents::SessionOptions,
        attachments: Vec<Attachment>,
        choices: fastctx_types::agents::Choices,
    ) -> Result<Said, Error> {
        let call = Call::Open {
            agent,
            town,
            node,
            text,
            model: options.model,
            reasoning_effort: options.reasoning_effort,
            attachments,
            choices,
        };
        match self.call(call).await?.0 {
            Reply::Opened { resident, landed } => Ok(Said { to: resident, landed, delivery: None }),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
}
