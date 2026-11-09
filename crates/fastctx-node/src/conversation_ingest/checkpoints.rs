//! Load and publish parser-state packages through source-gated content ports.
use super::{
    checkpoint_bundle::{self, BoundState},
    step_content::{StepContent, SuppliedContent},
};
use fastctx_hosts::claude::ingest::checkpoint::ClaudeCheckpointV1;
use fastctx_sync::{
    blocks::codec,
    identity::Digest,
    manifest::{
        BlobId, ContentBlock, ContentRef, DigestAlgorithm, DirectoryEntry, Encoding, ManifestPage, ManifestReply,
        ManifestRoot, PathName,
    },
    store::{
        Error,
        ingest::{SourceCheckpoint, SourceIngest},
        source_step::SourceStepTarget,
    },
};

pub async fn load(content: &StepContent<'_>, outer: &SourceCheckpoint) -> Result<BoundState, Error> {
    let Some(ContentRef::Manifest { root }) = &outer.state else {
        return Err(Error::Corrupt("Claude checkpoint lost its private state package".into()));
    };
    let page = content.state_manifest(root.clone()).await?;
    let ManifestPage::Directory { entries } = serde_json::from_str(&page.page_json)
        .map_err(|error| Error::Corrupt(format!("invalid parser state manifest: {error}")))?
    else {
        return Err(Error::Corrupt("parser state is not a directory package".into()));
    };
    let state = entries
        .into_iter()
        .find_map(|entry| match entry {
            DirectoryEntry::File { file } if file.path.components == [PathName::Utf8("state.json".into())] => {
                Some(file.content)
            }
            _ => None,
        })
        .ok_or_else(|| Error::Corrupt("parser state.json is absent".into()))?;
    let ContentRef::Single { block, decoded_bytes } = state else {
        return Err(Error::Corrupt("parser state.json exceeds its single bounded block".into()));
    };
    if decoded_bytes > 64 * 1024 {
        return Err(Error::Corrupt("parser state.json exceeds 64 KiB".into()));
    }
    let bytes = content.state_block(ContentBlock { id: block, decoded_bytes }).await?;
    BoundState::restore(&page, &bytes).map_err(|error| Error::Corrupt(error.to_string()))
}

pub async fn save(content: &StepContent<'_>, checkpoint: &ClaudeCheckpointV1) -> Result<BoundState, Error> {
    let bundle = checkpoint_bundle::encode(checkpoint).map_err(invalid)?;
    let json = content.put_block(bundle.json().to_vec()).await?;
    let page =
        package(&content.target().target().custody.object.scope, bundle.directory(json.into()).map_err(invalid)?)?;
    content.put_manifest(page.clone()).await?;
    bundle.bind(&page).map_err(invalid)
}

/// The supplied snapshot keeps the source window alive after mutable uploads
/// ended. A merge's consumed prefix determines the final checkpoint at this point.
pub async fn save_after_merge(
    source: &dyn SourceIngest,
    target: &SourceStepTarget,
    checkpoint: &ClaudeCheckpointV1,
    _working: &SuppliedContent,
) -> Result<BoundState, Error> {
    let bundle = checkpoint_bundle::encode(checkpoint).map_err(invalid)?;
    let encoded = codec::encode(target.target().custody.object.scope.clone(), bundle.json().to_vec(), Encoding::Raw)?;
    let json = source.put_block(target.clone(), encoded.representation, encoded.bytes).await?.content;
    let page = package(&target.target().custody.object.scope, bundle.directory(json.into()).map_err(invalid)?)?;
    source.put_manifest(target.clone(), page.clone()).await?;
    bundle.bind(&page).map_err(invalid)
}
fn package(scope: &fastctx_sync::identity::ScopeKey, page: ManifestPage) -> Result<ManifestReply, Error> {
    let page_json = serde_json::to_string(&page).map_err(|error| Error::Invalid(error.to_string()))?;
    let root = ManifestRoot(BlobId {
        domain: scope.clone(),
        algorithm: DigestAlgorithm::Sha256,
        digest: Digest::of(page_json.as_bytes()),
    });
    Ok(ManifestReply { root, page_json })
}
fn invalid(error: std::io::Error) -> Error {
    Error::Invalid(error.to_string())
}
