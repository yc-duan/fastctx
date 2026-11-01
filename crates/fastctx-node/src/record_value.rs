//! Large immutable values are staged before a small state transaction publishes them.
use crate::{record_payloads, records};
use fastctx_storage::{Priority, Result};
use fastctx_types::layout::ConversationDir;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "storage", content = "value", rename_all = "snake_case")]
pub(crate) enum Value {
    Inline(serde_json::Value),
    Payload(record_payloads::Reference),
}
pub(crate) struct Prepared {
    pub value: Value,
    staged: Option<record_payloads::Staged>,
}
impl Prepared {
    pub fn new(directory: &ConversationDir, value: &impl Serialize) -> io::Result<Self> {
        let bytes = serde_json::to_vec(value)?;
        if bytes.len() <= 4096 {
            return Ok(Self { value: Value::Inline(serde_json::from_slice(&bytes)?), staged: None });
        }
        let (database, scope) = records::locate(directory)?;
        let staged =
            record_payloads::stage(&database, &scope, bytes, Priority::Interactive).map_err(io::Error::other)?;
        Ok(Self { value: Value::Payload(staged.reference.clone()), staged: Some(staged) })
    }
    pub fn install(&self, rows: &records::Rows<'_>, family: &str, key: &str) -> Result<()> {
        let reference = match &self.value {
            Value::Inline(_) => None,
            Value::Payload(reference) => Some(reference),
        };
        record_payloads::link(rows.connection(), rows.conversation(), family, key, reference)?;
        if let Some(staged) = &self.staged {
            record_payloads::finish(rows.connection(), rows.conversation(), staged)?;
        }
        Ok(())
    }
}
impl Value {
    pub fn read<T: DeserializeOwned>(&self, rows: &records::Rows<'_>) -> Result<T> {
        match self {
            Self::Inline(value) => {
                serde_json::from_value(value.clone()).map_err(|error| fastctx_storage::Error::new(error.to_string()))
            }
            Self::Payload(reference) => {
                records::decode(&record_payloads::read(rows.connection(), rows.conversation(), reference)?)
            }
        }
    }
}
