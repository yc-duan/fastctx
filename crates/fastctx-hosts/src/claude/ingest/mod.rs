//! Incremental decoding of claimed Claude sources. Durable ingestion commits
//! belong to the node's registered writer, not to the format decoder. Activating
//! a decoder requires the caller's shared byte admission for its input, output
//! buffers and staged content; this module does not create per-source budgets.

pub mod body;
pub mod document;
pub mod image_base64;
pub mod jsonl;
pub mod lexical;
