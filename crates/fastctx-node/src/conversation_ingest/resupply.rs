//! A checkpoint edge does not grant an object the same bytes at a new position.
//! Promotion supplies every node again through the current source step. Traversal
//! retains only root/index cursors, never a stack of decoded manifest pages.
use super::step_content::StepContent;
use fastctx_sync::{
    blocks::manifest::{self, Reference},
    manifest::{ContentBlock, ContentRef, ManifestRoot},
    store::Error,
};
struct Frame {
    root: ManifestRoot,
    next: usize,
}

pub async fn state_content(content: &StepContent<'_>, root: &ContentRef) -> Result<(), Error> {
    let root = match root {
        ContentRef::Single { block: id, decoded_bytes } => {
            return block(content, ContentBlock { id: id.clone(), decoded_bytes: *decoded_bytes }).await;
        }
        ContentRef::Manifest { root } => root.clone(),
    };
    let mut stack = vec![Frame { root, next: 0 }];
    while let Some(frame) = stack.last() {
        let page = content.state_manifest(frame.root.clone()).await?;
        let checked = manifest::validate(&frame.root.0.domain, page)?;
        if let Some(reference) = checked.references().get(frame.next).cloned() {
            stack.last_mut().expect("nonempty traversal").next += 1;
            match reference {
                Reference::Block { id, decoded_bytes } => block(content, ContentBlock { id, decoded_bytes }).await?,
                Reference::Manifest { root } => {
                    if stack.len() >= 64 || stack.iter().any(|frame| frame.root == root) {
                        return Err(Error::Corrupt(
                            "private state closure has a cycle or exceeds its depth bound".into(),
                        ));
                    }
                    stack.push(Frame { root, next: 0 });
                }
            }
        } else {
            content.supply_manifest(checked.reply().clone()).await?;
            stack.pop();
        }
    }
    Ok(())
}
async fn block(content: &StepContent<'_>, block: ContentBlock) -> Result<(), Error> {
    content.supply_block(content.state_block(block).await?).await
}
