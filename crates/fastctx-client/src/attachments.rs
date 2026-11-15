//! Streaming uploads use a dedicated IPC connection, just as watches do.

use fastctx_types::access::Written;
use fastctx_types::attachments::{Attachment, UPLOAD_CHUNK, UploadFile, UploadTarget, Uploaded};
use fastctx_types::ipc::{ClientFrame, NodeFrame};
use fastctx_types::{Call, Reply};
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
        node: Option<Written>,
        file: UploadFile,
        source: impl AsyncRead + Unpin,
    ) -> Result<Attachment, CallError> {
        self.upload_scoped(target, node, file, None, source).await.map(|uploaded| uploaded.attachment)
    }

    pub(crate) async fn upload_scoped(
        mut self,
        target: UploadTarget,
        node: Option<Written>,
        file: UploadFile,
        composer: Option<fastctx_types::composer::UploadScope>,
        mut source: impl AsyncRead + Unpin,
    ) -> Result<Uploaded, CallError> {
        self.next += 1;
        let id = self.next;
        let size = file.size;
        let call = Call::Upload { target, node, file, composer };
        self.connection
            .send(&crate::to_json(&ClientFrame::Call { id, call, submission: None }))
            .await
            .map_err(CallError::Broken)?;
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
    pub async fn attachment_places(
        &self,
        target: UploadTarget,
        machine: Option<Written>,
        account: Option<fastctx_types::access::HubAccount>,
        preferred: Option<Written>,
    ) -> Result<fastctx_types::attachments::AttachmentPlaces, Error> {
        match self.call(Call::AttachmentPlaces { target, machine, account, preferred }).await?.0 {
            Reply::AttachmentPlaces(places) => Ok(places),
            _ => Err(Error::Unexpected("upgrade Fastctx to choose attachment destinations".into())),
        }
    }

    pub async fn upload(
        &self,
        target: UploadTarget,
        node: Option<Written>,
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
        node: Option<Written>,
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
                account: None,
                files_on: None,
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
        machine: Option<Written>,
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
        node: Option<Written>,
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

/// What is said beside a text attachment of long words: the words are in it.
const IN_ATTACHMENT: &str = "(The full text is in the attached file.)";
/// The name a text attachment of long words is given, as the input box names
/// one in English.
const TEXT_FILE: &str = "Pasted text.txt";

impl World {
    /// Words longer than an input box holds, as an input box sends them
    /// (design §28.49): uploaded to `target` on `node` as a text attachment,
    /// with a line beside it; shorter ones as they are. The upload is the
    /// ordinary one, its receipt passed on unchanged.
    pub async fn as_attachment(&self, target: UploadTarget, node: Option<Written>, text: String) -> Result<(String, Vec<Attachment>), Error> {
        if !long(&text) {
            return Ok((text, Vec::new()));
        }
        let bytes = text.into_bytes();
        let file = UploadFile { name: TEXT_FILE.into(), media_type: "text/plain;charset=utf-8".into(), size: bytes.len() as u64, sha256: sha256(&bytes) };
        let attachment = self.upload(target, node, file, bytes.as_slice()).await?;
        Ok((IN_ATTACHMENT.into(), vec![attachment]))
    }

    /// Talks to `to`, words longer than an input box holds going as a text
    /// attachment (`as_attachment`): what was said, and the words that went
    /// — the line beside the attachment, where they went as one. A
    /// conversation on another machine keeps the file on this computer, as
    /// the input box keeps what it sends; its machine unnamed, it is found as
    /// talk finds it (the node's upload). The upload is named by the words
    /// and the conversation, so the same words sent again — a failed send
    /// asked again — take the upload made before rather than another.
    pub async fn talk_long(
        &self,
        to: fastctx_types::ResidentId,
        machine: Option<Written>,
        text: String,
        id: Option<String>,
    ) -> Result<(Said, String), Error> {
        if !long(&text) {
            let said = self.talk_with_attachments(to, machine, text.clone(), Vec::new(), None, id).await?;
            return Ok((said, text));
        }
        let inner = UploadTarget::Resident { resident: to.clone() };
        let target = match &machine {
            Some(machine) => UploadTarget::Sent {
                machine: machine.clone(),
                target: Box::new(inner),
                operation: format!("words-{}-{}", to, &sha256(text.as_bytes())[..32]),
                account: None,
                files_on: None,
            },
            None => inner,
        };
        let (text, attachments) = self.as_attachment(target, machine.clone(), text).await?;
        let said = self.talk_with_attachments(to, machine, text.clone(), attachments, None, id).await?;
        Ok((said, text))
    }

    /// Opens a session, first words longer than an input box holds going as a
    /// text attachment (`as_attachment`), for the account and the files'
    /// place `choices` names, as the input box uploads for a new one: what
    /// was said, and the first words as they went.
    pub async fn open_long(
        &self,
        agent: fastctx_types::AgentName,
        town: Option<fastctx_types::TownName>,
        node: Option<Written>,
        text: String,
        options: fastctx_types::agents::SessionOptions,
        choices: fastctx_types::agents::Choices,
    ) -> Result<(Said, String), Error> {
        if !long(&text) {
            let said = self.open_with(agent, town, node, text.clone(), options, choices).await?;
            return Ok((said, text));
        }
        let inner = UploadTarget::New { agent: agent.clone(), town: town.clone() };
        let machine = match &node {
            Some(node) => Some(node.clone()),
            None if self.device().is_none() => Some(self.connect().await?.machine.into()),
            None => None,
        };
        let target = match machine {
            Some(machine) => UploadTarget::Sent {
                machine,
                target: Box::new(inner),
                // One opening: the words do not name it — the same first
                // words may open another conversation after this one.
                operation: new_operation(),
                account: choices.account.clone(),
                files_on: choices.files_on.clone(),
            },
            None => inner,
        };
        let (text, attachments) = self.as_attachment(target, node.clone(), text).await?;
        let said = self.open_with_attachments(agent, town, node, text.clone(), options, attachments, choices).await?;
        Ok((said, text))
    }
}

/// An operation for one upload of its own, stable for its retries within it.
/// A name for words a device sends, its own for them (`Call::Talk::id`):
/// the machine they go to names their delivery by it, and tells it back.
pub fn words_id() -> String {
    new_operation()
}

fn new_operation() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|since| since.as_nanos()).unwrap_or_default();
    format!("words-{}-{at:x}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Whether `text` is longer than an input box holds.
fn long(text: &str) -> bool {
    text.chars().count() > fastctx_types::composer::MAX_DOCUMENT_CHARACTERS
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
