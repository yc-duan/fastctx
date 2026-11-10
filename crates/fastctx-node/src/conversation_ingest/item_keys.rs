//! Display identities use the frozen native coordinates, not row arrival or
//! body hashes. A paired tool result revises the execution's ToolCall item;
//! it does not turn the result's containing user row into another question.
use fastctx_types::sync::{
    conversation_identity::{IdentityError, ItemKey, ItemRole, NativeId, NativeItemKey},
    conversation_timeline::ItemIdentity,
    cursor::Count,
    identity::ObjectId,
};

pub fn user(conversation: &ObjectId, uuid: &str) -> Result<ItemIdentity, IdentityError> {
    Ok(native(conversation, ItemRole::User, NativeItemKey::ClaudeMessage { uuid: NativeId::new(uuid)? }))
}
/// The ordinal counts every host content block, including tools and thinking.
pub fn assistant_block(conversation: &ObjectId, uuid: &str, block: Count) -> Result<ItemIdentity, IdentityError> {
    Ok(native(conversation, ItemRole::AssistantBlock, NativeItemKey::ClaudeBlock { uuid: NativeId::new(uuid)?, block }))
}
/// Both the invocation and a verified paired result address this same item.
pub fn tool(conversation: &ObjectId, tool_use_id: &str) -> Result<ItemIdentity, IdentityError> {
    Ok(native(conversation, ItemRole::ToolCall, NativeItemKey::ClaudeTool { tool_use_id: NativeId::new(tool_use_id)? }))
}
fn native(conversation: &ObjectId, role: ItemRole, native: NativeItemKey) -> ItemIdentity {
    ItemIdentity::Native(ItemKey { conversation: conversation.clone(), role, native })
}
