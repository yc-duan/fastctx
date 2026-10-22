//! Files the user attaches to their words, carried to the agent's machine
//! as the core's upload streams them: a file named by its path on this
//! computer (dropped on the window), or bytes the page holds (pasted). The
//! far end verifies the size and the digest it is told, so both are taken
//! first; a file that changes in between fails there, as it should.

use fastctx_types::access::Written;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use fastctx_client::World;
use fastctx_types::attachments::{Attachment, UploadFile, UploadTarget};
use sha2::{Digest, Sha256};
use tauri::ipc::Channel;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

#[fastctx_gui_command::command]
pub async fn attachment_places(
    shell: tauri::State<'_, crate::Shell>,
    target: UploadTarget,
    machine_name: Option<String>,
    account: Option<fastctx_types::access::HubAccount>,
    preferred: Option<Written>,
) -> Result<fastctx_types::attachments::AttachmentPlaces, String> {
    shell
        .world()
        .attachment_places(target, crate::machine(machine_name)?, account, preferred)
        .await
        .map_err(|error| error.to_string())
}

/// Uploads the file at `path` on this computer; `sent` hears how many of
/// its bytes have gone so far.
pub async fn from_path(
    world: &World,
    target: UploadTarget,
    node: Option<Written>,
    path: &Path,
    sent: Channel<u64>,
) -> Result<Attachment, String> {
    let name =
        path.file_name().map(|name| name.to_string_lossy().into_owned()).ok_or("an attachment has no file name")?;
    let (size, sha256) = digest(path).await.map_err(|error| format!("{}: {error}", path.display()))?;
    let upload = UploadFile { media_type: media_type_of(&name).to_string(), name, size, sha256 };
    let mut again = fastctx_client::Again::default();
    loop {
        let file = tokio::fs::File::open(path).await.map_err(|error| format!("{}: {error}", path.display()))?;
        match world
            .upload(target.clone(), node.clone(), upload.clone(), Counted { inner: file, sent: sent.clone(), total: 0 })
            .await
        {
            Ok(attachment) => return Ok(attachment),
            Err(error) if matches!(target, UploadTarget::Sent { .. }) && World::unanswered(&error) => {
                again.broke(error.to_string());
                again.pause(world, node.as_ref()).await;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Uploads bytes the page holds, under the name and type it gives.
pub async fn from_bytes(
    world: &World,
    target: UploadTarget,
    node: Option<Written>,
    name: String,
    media_type: String,
    bytes: Vec<u8>,
) -> Result<Attachment, String> {
    let upload = UploadFile {
        media_type: if media_type.is_empty() { media_type_of(&name).to_string() } else { media_type },
        name,
        size: bytes.len() as u64,
        sha256: hex(&Sha256::digest(&bytes)),
    };
    let mut again = fastctx_client::Again::default();
    loop {
        match world.upload(target.clone(), node.clone(), upload.clone(), bytes.as_slice()).await {
            Ok(attachment) => return Ok(attachment),
            Err(error) if matches!(target, UploadTarget::Sent { .. }) && World::unanswered(&error) => {
                again.broke(error.to_string());
                again.pause(world, node.as_ref()).await;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

async fn digest(path: &Path) -> std::io::Result<(u64, String)> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok((size, hex(&hasher.finalize())));
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
}

pub async fn composer_path(
    world: &World,
    resident: fastctx_types::ResidentId,
    node: Option<Written>,
    scope: fastctx_types::composer::UploadScope,
    path: &Path,
    sent: Channel<u64>,
) -> Result<fastctx_types::composer::UploadResult, String> {
    let name =
        path.file_name().map(|name| name.to_string_lossy().into_owned()).ok_or("an attachment has no file name")?;
    let (size, sha256) = digest(path).await.map_err(|error| format!("{}: {error}", path.display()))?;
    let file = tokio::fs::File::open(path).await.map_err(|error| format!("{}: {error}", path.display()))?;
    let upload = UploadFile { media_type: media_type_of(&name).to_string(), name, size, sha256 };
    world
        .upload_composer(resident, node, upload, scope, Counted { inner: file, sent, total: 0 })
        .await
        .map_err(|error| error.to_string())
}

pub async fn composer_bytes(
    world: &World,
    resident: fastctx_types::ResidentId,
    node: Option<Written>,
    scope: fastctx_types::composer::UploadScope,
    name: String,
    media_type: String,
    bytes: Vec<u8>,
) -> Result<fastctx_types::composer::UploadResult, String> {
    let upload = UploadFile {
        media_type: if media_type.is_empty() { media_type_of(&name).to_string() } else { media_type },
        name,
        size: bytes.len() as u64,
        sha256: hex(&Sha256::digest(&bytes)),
    };
    world.upload_composer(resident, node, upload, scope, bytes.as_slice()).await.map_err(|error| error.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The type a file's name says it is; what the name does not say is bytes.
fn media_type_of(name: &str) -> &'static str {
    let ending = name.rsplit_once('.').map(|(_, ending)| ending.to_ascii_lowercase()).unwrap_or_default();
    match ending.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        _ => "application/octet-stream",
    }
}

/// A reader that says how many bytes have been read through it.
struct Counted<R> {
    inner: R,
    sent: Channel<u64>,
    total: u64,
}

impl<R: AsyncRead + Unpin> AsyncRead for Counted<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(context, buffer);
        let read = buffer.filled().len() - before;
        if read > 0 {
            self.total += read as u64;
            // The page may have gone; the upload goes on regardless.
            let _ = self.sent.send(self.total);
        }
        polled
    }
}
