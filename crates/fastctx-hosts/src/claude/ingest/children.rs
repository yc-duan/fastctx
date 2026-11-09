//! Child identities derive from a claimed parent's canonical identity and a
//! native agent key. A filename alone never claims a conversation or a source.
use fastctx_types::sync::{
    conversation_identity::{ConversationKey, NativeAgentKey, NativeId},
    identity::{Digest, ObjectId, SourceId},
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildClaim {
    pub parent: ObjectId,
    pub child: ObjectId,
    pub source: SourceId,
    pub agent: NativeId,
    pub tool_use: NativeId,
}
/// The node verifies `parent` custody before calling this format-only helper.
/// The call key must come from the parent report, tool result or child meta file.
pub fn claim(parent: ObjectId, agent: &str, tool_use: &str) -> io::Result<ChildClaim> {
    if !super::super::subagents::valid_id(agent) {
        return Err(io::Error::other("invalid Claude agent identity"));
    }
    let agent = NativeId::new(agent).map_err(io::Error::other)?;
    let tool_use = NativeId::new(tool_use).map_err(io::Error::other)?;
    let child =
        ConversationKey::NativeChild { parent: parent.clone(), agent: NativeAgentKey::Claude { agent: agent.clone() } }
            .id();
    let source = SourceId::from_bytes(
        Digest::of(&serde_json::to_vec(&("fastctx.claude.child-source.v1", &child, &agent)).map_err(io::Error::other)?)
            .bytes(),
    );
    Ok(ChildClaim { parent, child, source, agent, tool_use })
}
pub fn record_path(parent_record: &Path, claim: &ChildClaim) -> io::Result<PathBuf> {
    super::super::subagents::record(parent_record, claim.agent.as_str())
        .ok_or_else(|| io::Error::other("child source has no valid record path"))
}
