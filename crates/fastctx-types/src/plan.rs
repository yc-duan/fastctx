//! A conversation's account-owned Markdown canvas and its editing contract.

use crate::{Address, ResidentId, account_records::Metadata};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub const KIND: &str = "plan";
pub const MAX_MARKDOWN_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    User,
    Resident { address: Address },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub markdown: String,
    pub author: Author,
    /// The conversation whose plan it is. Plans are kept under the short
    /// name of a conversation, which a conversation on another machine may
    /// share: this tells them apart. Earlier releases wrote none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Box<Address>>,
}

impl Body {
    /// The conversation it is known to be of: the one written with it, else
    /// the conversation that last wrote it; none for one only the user wrote
    /// before owners were written.
    pub fn of(&self) -> Option<&Address> {
        self.owner.as_deref().or(match &self.author {
            Author::Resident { address } => Some(address),
            Author::User => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub owner: ResidentId,
    pub metadata: Option<Metadata>,
    pub body: Option<Body>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Operation {
    Read,
    /// The conversation's stored seen revision supplies the comparison.
    Write {
        markdown: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Result {
    Page { page: Page },
    Conflict { current: Option<Metadata> },
}

impl Page {
    pub fn version(&self) -> Option<u64> {
        self.metadata.as_ref().map(|metadata| metadata.version)
    }
    pub fn deleted(&self) -> bool {
        self.metadata.as_ref().is_some_and(|metadata| metadata.deleted)
    }
    pub fn changed_at(&self) -> Option<Timestamp> {
        self.metadata.as_ref().map(|metadata| metadata.at)
    }
}

pub fn validate(markdown: &str) -> std::result::Result<(), String> {
    if markdown.len() > MAX_MARKDOWN_BYTES {
        return Err(words::too_large(markdown.len()));
    }
    Ok(())
}

pub mod words {
    pub const HELP: &str = "Show this conversation's plan";
    pub fn invalid_body(error: impl std::fmt::Display) -> String {
        format!("invalid plan body: {error}")
    }
    pub const RESIDENT: &str = "this command needs a resident conversation";
    pub const READ_ONLY: &str = "a colleague may read its requester's plan but cannot write a plan";
    pub const READ_FIRST: &str = "read the plan with fastctx plan before writing it";
    pub const CONFLICT: &str = "the plan changed; read it with fastctx plan before writing again";
    pub const DELETED: &str = "this conversation's plan was deleted";
    pub const REPLY: &str = "the plan request received an unexpected reply";
    pub const OLDER: &str = "update the hub and the conversation's machine to use plans from a remote job";
    pub const ROUTE: &str = "the job has no unique authorized route to its conversation's plan";
    pub const JOB: &str = "a plan request must return from a machine that worked for this conversation";
    pub const CONTROL: &str = "a plan request must be a complete control request through the hub";
    pub fn too_large(bytes: usize) -> String {
        format!("the plan is {bytes} bytes; the limit is {}", super::MAX_MARKDOWN_BYTES)
    }
    /// The plan kept under this conversation's short name is another's.
    pub fn elsewhere(owner: &crate::Address) -> String {
        format!(
            "the conversation {} on {} keeps its plan under this conversation's short name, so this conversation cannot keep one",
            owner.resident, owner.machine
        )
    }
    /// Said to a command that would write the plan whole.
    pub fn file(path: &std::path::Path, machine: &crate::MachineName) -> String {
        format!(
            "the plan is the Markdown file {} on {machine}: change it as any file, and each change saved is its next version",
            path.to_string_lossy().replace('\\', "/")
        )
    }
    pub fn version(version: Option<u64>) -> String {
        version.map_or_else(|| "No plan yet.".into(), |version| format!("Plan version {version}."))
    }
}
