//! Indexed artifact transfers and their provisional resources.

use crate::{
    Error,
    cache::lines::{Index, Row},
};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

pub(crate) struct ArtifactSink {
    pub(crate) output: tokio::io::BufWriter<tokio::fs::File>,
    pub(crate) index: Index,
    pub(crate) offset: u64,
    pub(crate) from: u64,
    pub(crate) digest: Sha256,
}

impl ArtifactSink {
    pub(crate) async fn create(
        folder: &std::path::Path,
        extent: fastctx_types::lines::Extent,
        body: Option<String>,
    ) -> Result<Self, Error> {
        tokio::fs::create_dir_all(folder).await.map_err(Error::Kept)?;
        Ok(Self {
            output: tokio::io::BufWriter::new(
                tokio::fs::File::create(folder.join("parts.jsonl")).await.map_err(Error::Kept)?,
            ),
            index: Index { extent, rows: Vec::new(), body },
            offset: 0,
            from: 0,
            digest: Sha256::new(),
        })
    }

    pub(crate) fn complete(&self) -> bool {
        self.index.rows.len() as u64 == self.index.extent.length && self.from == self.offset
    }

    pub(crate) async fn sync(&mut self) -> Result<(), Error> {
        self.output.flush().await.map_err(Error::Kept)?;
        self.output.get_ref().sync_all().await.map_err(Error::Kept)
    }
}

impl crate::line_rows::Sink for ArtifactSink {
    async fn put(&mut self, part: fastctx_types::lines::Part) -> Result<(), Error> {
        if part.line != self.index.rows.len() as u64 {
            return Err(Error::Unexpected("immutable line parts came out of order".into()));
        }
        let ended = part.ended;
        let mut bytes = serde_json::to_vec(&part).map_err(|error| Error::Other(error.to_string()))?;
        bytes.push(b'\n');
        self.output.write_all(&bytes).await.map_err(Error::Kept)?;
        self.digest.update(&bytes);
        self.offset += bytes.len() as u64;
        if ended {
            self.index.rows.push(Row {
                from: self.from,
                to: self.offset,
                digest: std::mem::take(&mut self.digest).finalize().into(),
            });
            self.from = self.offset;
        }
        Ok(())
    }
}

pub(crate) struct Pending(pub(crate) std::path::PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("discarding an unfinished artifact cache: {error}");
        }
    }
}

pub(crate) struct SourceOwner {
    pub(crate) sources: std::sync::Arc<std::sync::Mutex<crate::lines::Sources>>,
    pub(crate) machine: fastctx_types::access::Written,
    pub(crate) source: String,
}
impl Drop for SourceOwner {
    fn drop(&mut self) {
        self.sources.lock().expect("line sources").remove(&self.machine, &self.source);
    }
}
