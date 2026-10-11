//! Attachment-specific errors survive the IPC and Tauri boundaries.

use fastctx_types::attachments::{AttachmentFile, AttachmentSource};
use serde::Serialize;

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
) -> Result<Fetched, Failure> {
    let reading = world.read_attachment(attachment).await.map_err(Failure::from)?;
    let file = reading
        .info()
        .attachment
        .clone()
        .ok_or_else(|| Failure::failed("the node supplied no attachment metadata".into()))?;
    let path = crate::local_files::store(reading, file.name.clone(), cache).await.map_err(Failure::failed)?;
    Ok(Fetched { path, file })
}
