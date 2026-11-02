//! One current selection per conversation, at most 64 KiB, retained until its
//! conversation is deleted. Replacements retain no earlier option snapshots.
use crate::{
    Node,
    records::{self, Source},
};
use fastctx_storage::Priority;
use fastctx_types::{ResidentId, agents::SessionOptions, ipc::Failure, layout::ConversationDir};
use std::{
    io::{self, Read},
    sync::Arc,
};

const BYTES: usize = 64 * 1024;
const FAMILY: &str = "session-options";
fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

fn encoded(options: &SessionOptions) -> Result<Vec<u8>, Failure> {
    let bytes = serde_json::to_vec(options).map_err(failed)?;
    if bytes.len() > BYTES {
        return Err(Failure::new("session options exceed their 64 KiB budget"));
    }
    Ok(bytes)
}

pub(crate) fn prepare(directory: &ConversationDir) -> io::Result<()> {
    let path = directory.session_options();
    records::ensure_each(directory, FAMILY, move |push| {
        crate::record_migration::file(&path, push, |push| {
            let mut bytes = Vec::new();
            fastctx_platform::disk::open(&path)?.take(BYTES as u64 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "legacy session options exceed 64 KiB; source preserved",
                ));
            }
            let _: SessionOptions = serde_json::from_slice(&bytes)?;
            push(Source::new("current".into(), path.clone(), bytes)?)
        })
    })
}

fn store(directory: &ConversationDir, bytes: Vec<u8>, create: bool) -> io::Result<()> {
    prepare(directory)?;
    records::write_at(directory, Priority::Interactive, bytes.len(), move |rows| {
        if create && rows.exists(FAMILY, "current")? {
            return Err(fastctx_storage::Error::new("session options already exist"));
        }
        rows.put_raw(FAMILY, "current", &bytes)
    })
}

fn current(directory: &ConversationDir) -> io::Result<SessionOptions> {
    prepare(directory)?;
    records::read(directory, |rows| Ok(rows.get(FAMILY, "current")?.unwrap_or_default()))
}

/// Creates the initial selection without overwriting another successful birth.
pub async fn keep(node: &Arc<Node>, id: &ResidentId, options: &SessionOptions) -> Result<(), Failure> {
    let (directory, bytes) = (node.world.conversation(id), encoded(options)?);
    fastctx_platform::task::spawn_blocking(move || store(&directory, bytes, true))
        .await
        .map_err(failed)?
        .map_err(failed)
}

/// Reads on a blocking worker. The resident's current MCP choice is authoritative.
pub fn read(node: &Node, id: &ResidentId) -> Result<SessionOptions, Failure> {
    let mut options = current(&node.world.conversation(id)).map_err(failed)?;
    if let Some((about, _)) = node.residents.about(id) {
        options.mcp_off = about.mcp_off;
    }
    Ok(options)
}

pub(crate) async fn load(node: &Arc<Node>, id: &ResidentId) -> Result<SessionOptions, Failure> {
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn_blocking(move || read(&node, &id)).await.map_err(failed)?
}

pub(crate) async fn set_mcp(
    node: &Arc<Node>,
    id: &ResidentId,
    off: std::collections::BTreeSet<String>,
) -> Result<(), Failure> {
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn_blocking(move || {
        let mut options = read(&node, &id)?;
        options.mcp_off = off.clone();
        let bytes = encoded(&options)?;
        node.residents.set_mcp_off(&node.world, &id, off).map_err(failed)?;
        store(&node.world.conversation(&id), bytes, false).map_err(failed)
    })
    .await
    .map_err(failed)?
}

pub fn configure(config: &mut serde_json::Value, options: SessionOptions) {
    if let Some(model) = options.model {
        config["model"] = model.into();
    }
    if let Some(effort) = options.reasoning_effort {
        config["model_reasoning_effort"] = effort.into();
    }
}
