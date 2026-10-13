//! Attachment-specific errors survive the IPC and Tauri boundaries.

use fastctx_types::attachments::{AttachmentFile, AttachmentSource};
use serde::Serialize;
use tauri::ipc::Channel;

use crate::held_reads::{ReadProgress, tell};

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Failure {
    OriginalGone { message: String },
    Failed { message: String },
}

impl Failure {
    pub fn failed(message: String) -> Self {
        Self::Failed { message }
    }
}

impl From<fastctx_client::Error> for Failure {
    fn from(error: fastctx_client::Error) -> Self {
        if matches!(&error, fastctx_client::Error::Call(fastctx_client::CallError::Failed(failure)) if failure.code == Some(fastctx_types::ipc::FailureCode::OriginalGone))
        {
            Self::OriginalGone { message: error.to_string() }
        } else {
            Self::failed(error.to_string())
        }
    }
}

#[derive(Serialize)]
pub struct Fetched {
    pub path: String,
    #[serde(flatten)]
    pub file: AttachmentFile,
}

pub async fn fetch(
    world: &fastctx_client::World,
    attachment: AttachmentSource,
    cache: &std::path::Path,
    progress: Option<Channel<ReadProgress>>,
) -> Result<Fetched, Failure> {
    let mut reading = world.read_attachment(attachment);
    tell(reading.progress(), progress);
    let path = crate::local_files::store(
        &mut reading,
        |info| {
            info.attachment
                .as_ref()
                .map(|file| file.name.clone())
                .ok_or_else(|| "the node supplied no attachment metadata".into())
        },
        cache,
    )
    .await
    .map_err(|stored| match stored {
        crate::local_files::Stored::Read(error) => Failure::from(error),
        crate::local_files::Stored::Kept(message) => Failure::failed(message),
    })?;
    let file = reading
        .info()
        .and_then(|info| info.attachment.clone())
        .ok_or_else(|| Failure::failed("the node supplied no attachment metadata".into()))?;
    Ok(Fetched { path, file })
}
