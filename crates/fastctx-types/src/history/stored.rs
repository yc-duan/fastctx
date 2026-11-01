//! The account record body retains the exact Markdown and submission receipt.

use super::{HistoryRevision, markdown};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub revision: HistoryRevision,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<String>,
}

impl Stored {
    pub fn validate(&self) -> Result<(), String> {
        if self.revision.version == 0 {
            return Err("a history revision starts at one".into());
        }
        let parsed = markdown::parse(&self.revision.name, &self.text)?;
        if parsed.description != self.revision.description {
            return Err("the history revision description differs from its text".into());
        }
        if self.submission.as_ref().is_some_and(|id| id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err("invalid history submission identity".into());
        }
        Ok(())
    }
}
