//! A conversation's account-owned Markdown canvas and its editing contract.

use crate::{Address, ResidentId, account_records::Metadata};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub const KIND: &str = "plan";
/// The most a plan may take as it is kept, its text escaped as JSON. A plan
/// comes back into the model's context whole after every compaction, and the
/// Claude Code plugin reads each block of that context in one read of at most
/// 4 MiB; beyond this a plan could no longer reach the model. It guards
/// against what would break, not against how much a plan says.
pub const MAX_KEPT_BYTES: usize = 4 * 1024 * 1024 - 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    User,
    /// The writing conversation is on a machine unavailable to this reader.
    Elsewhere,
    Resident {
        address: Address,
    },
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
    /// The durable working-copy submission that created this revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<Box<String>>,
}

impl Body {
    /// The conversation it is known to be of: the one written with it, else
    /// the conversation that last wrote it; none for one only the user wrote
    /// before owners were written.
    pub fn of(&self) -> Option<&Address> {
        self.owner.as_deref().or(match &self.author {
            Author::Resident { address } => Some(address),
            Author::User | Author::Elsewhere => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub owner: ResidentId,
    pub metadata: Option<Metadata>,
    pub body: Option<Body>,
}

/// Another conversation whose plan a command reads: its short name, and the
/// machine it lives on where the short name alone could name two.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Of {
    pub resident: ResidentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<crate::MachineName>,
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
    // Quotes, backslashes and control characters take more room as kept.
    let kept = serde_json::to_string(markdown).map_or(usize::MAX, |text| text.len());
    if kept > MAX_KEPT_BYTES {
        return Err(words::too_large(kept));
    }
    Ok(())
}

pub mod words {
    pub const HELP: &str = "Show this conversation's plan, or another's by its id";
    pub const OF_HELP: &str = "Another conversation's id, such as a colleague's from `fastctx w`; append @machine where the id names more than one";
    pub fn invalid_body(error: impl std::fmt::Display) -> String {
        format!("invalid plan body: {error}")
    }
    pub const RESIDENT: &str = "this command needs a resident conversation";
    pub const ASSISTED: &str = "this conversation keeps no plan it can change";
    pub const READ_FIRST: &str = "read the plan with fastctx plan before writing it";
    pub const CONFLICT: &str = "the plan changed; read it with fastctx plan before writing again";
    pub const DELETED: &str = "this conversation's plan was deleted";
    pub const REPLY: &str = "the plan request received an unexpected reply";
    pub const OLDER: &str = "update the hub and the conversation's machine to use plans from a remote job";
    pub const ROUTE: &str = "the job has no unique authorized route to its conversation's plan";
    pub const JOB: &str = "a plan request must return from a machine that worked for this conversation";
    pub const CONTROL: &str = "a plan request must be a complete control request through the hub";
    pub fn too_large(bytes: usize) -> String {
        format!("the plan takes {bytes} bytes as kept; the most it can take is {}", super::MAX_KEPT_BYTES)
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
    /// The heading of another conversation's plan.
    pub fn version_of(resident: &crate::ResidentId, version: Option<u64>) -> String {
        version.map_or_else(
            || format!("{resident} has no plan yet."),
            |version| format!("The plan of {resident}, version {version}."),
        )
    }
}
