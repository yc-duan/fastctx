//! Document descriptions reference exact UTF-8 content. Text preparation is
//! separate from installing a handler or accepting an authority revision.
pub mod authorize;
mod handler;
pub use handler::Handler;
pub mod legacy;
pub mod lifecycle;
pub mod reconcile;
pub mod schema;
pub mod text;
use crate::{Error, json};
use fastctx_sync::{
    cursor::ByteCount,
    identity::{Digest, ObjectKey},
    manifest::ContentRef,
};
use fastctx_types::{history, plan};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plan,
    ConversationHistory,
    ProjectHistory,
}
impl Kind {
    pub const ALL: [Self; 3] = [Self::Plan, Self::ConversationHistory, Self::ProjectHistory];
    pub fn id(self) -> &'static str {
        match self {
            Self::Plan => "conversation.plan",
            Self::ConversationHistory => "conversation.history",
            Self::ProjectHistory => "project.history",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Conversation { conversation: ObjectKey },
    Project { project: ObjectKey },
}
impl Target {
    pub fn object(&self) -> &ObjectKey {
        match self {
            Self::Conversation { conversation } => conversation,
            Self::Project { project } => project,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextContent {
    pub root: ContentRef,
    pub bytes: ByteCount,
    /// Digest of the exact document bytes, including a possible UTF-8 BOM.
    pub digest: Digest,
    pub utf8_bom: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Details {
    Plan { author: plan::Author, submission: Option<String> },
    History { name: String, description: String, retired: bool, author: history::Author, submission: Option<String> },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub target: Target,
    pub text: TextContent,
    pub details: Details,
}
impl Document {
    pub fn kind(&self) -> Result<Kind, Error> {
        match (&self.target, &self.details) {
            (Target::Conversation { .. }, Details::Plan { .. }) => Ok(Kind::Plan),
            (Target::Conversation { .. }, Details::History { .. }) => Ok(Kind::ConversationHistory),
            (Target::Project { .. }, Details::History { .. }) => Ok(Kind::ProjectHistory),
            _ => Err(json::invalid("a plan belongs to a conversation")),
        }
    }
    /// Checks a newly authored description, independently of body preparation.
    pub fn validate(&self) -> Result<(), Error> {
        self.validate_shape()?;
        schema::validate(self, false)
    }
    /// For verified old records and byte-preserving lifecycle operations only.
    /// Passing this check never authorizes a replacement or changed description.
    pub fn validate_stored(&self) -> Result<(), Error> {
        self.validate_shape()?;
        schema::validate(self, true)
    }
    fn validate_shape(&self) -> Result<(), Error> {
        self.kind()?;
        self.text.root.validate().map_err(Error::Invalid)?;
        if self.text.root.id().domain != self.target.object().scope {
            return Err(json::invalid("document content crosses its target scope"));
        }
        if let ContentRef::Single { block, decoded_bytes } = &self.text.root
            && (u64::from(*decoded_bytes) != self.text.bytes.0 || block.digest != self.text.digest)
        {
            return Err(json::invalid("document extent differs from its exact content reference"));
        }

        Ok(())
    }
    pub fn retired(&self) -> bool {
        matches!(self.details, Details::History { retired: true, .. })
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.target == other.target
            && match (&self.details, &other.details) {
                (Details::Plan { .. }, Details::Plan { .. }) => true,
                (Details::History { name: a, .. }, Details::History { name: b, .. }) => a == b,
                _ => false,
            }
    }
}
pub fn decode(body: &[u8]) -> Result<Document, Error> {
    if body.len() > schema::DESCRIPTION_BYTES {
        return Err(json::invalid("document description exceeds its schema bound"));
    }
    let document: Document = json::typed(serde_json::Value::Object(json::object(body)?))?;
    document.validate()?;
    Ok(document)
}
/// Decodes previously verified descriptions without applying new authoring limits.
/// Migration must still validate the original legacy record and its ownership.
pub fn decode_stored(body: &[u8]) -> Result<Document, Error> {
    if body.len() > schema::LEGACY_DESCRIPTION_BYTES {
        return Err(json::invalid("stored document description exceeds its schema bound"));
    }
    let document: Document = json::typed(serde_json::Value::Object(json::object(body)?))?;
    document.validate_stored()?;
    Ok(document)
}
