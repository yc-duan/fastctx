//! Attachment references follow the context actually persisted by a native fork.

use crate::Node;
use fastctx_hosts::claude::{exchange, transcript};
use fastctx_types::{ResidentId, ipc::Failure};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(super) async fn expected(node: &Node, session: &str, prefix: Option<&Path>) -> Result<Vec<Value>, Failure> {
    let path = match prefix {
        Some(path) => path.to_path_buf(),
        None => exchange::transcript(&exchange::sessions(node.world.path()).join(session))
            .map_err(failure)?
            .ok_or_else(|| failure("the source conversation has no saved history"))?,
    };
    let read = read(path).await?;
    if !read.problems.is_empty() {
        return Err(failure(read.problems.join("; ")));
    }
    Ok(read.context_messages)
}

pub(super) async fn inherit(
    node: &Arc<Node>,
    parent: &ResidentId,
    child: &ResidentId,
    expected: &[Value],
) -> Result<(), Failure> {
    let session = node
        .residents
        .about(child)
        .and_then(|(about, _)| about.session)
        .ok_or_else(|| failure("the fork has no saved session"))?;
    let directory = exchange::sessions(node.world.path()).join(session);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(path) = exchange::transcript(&directory).map_err(failure)? {
            let copied = read(path).await?;
            // 2026-10-20: creating the transcript precedes asynchronously saving
            // the copied rows. Grant only the attachments in the confirmed context.
            let inherited = if copied.synthetic_tail && copied.context_messages.len() == expected.len() + 1 {
                &copied.context_messages[..expected.len()]
            } else {
                copied.context_messages.as_slice()
            };
            if copied.problems.is_empty()
                && inherited.len() == expected.len()
                && inherited
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| fastctx_hosts::claude::transcript::same_projection(actual, expected))
            {
                return crate::attachments::inherit_texts(node, parent, child, &copied.context_messages);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(failure("Claude's fork did not persist the expected attachment context within 10 s"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn read(path: PathBuf) -> Result<transcript::Transcript, Failure> {
    tokio::task::spawn_blocking(move || transcript::read_with_cuts(&path, &[])).await.map_err(failure)?.map_err(failure)
}
