//! Legacy identities live beside metadata, outside rows rewritten while typing.

use super::{Alias, Document, Failure, failure};
use serde::{Deserialize, Deserializer, Serializer};

const REFERENCE: &str = "composer-alias-journal:1";

// An older reader must reject this forward format instead of silently opening
// an empty checkpoint after a conversation is moved to that reader's device.
pub(super) fn serialize_aliases<S: Serializer>(_: &[Alias], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(REFERENCE)
}
pub(super) fn deserialize_aliases<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Alias>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Legacy(Vec<Alias>),
        Journal(String),
    }
    match Stored::deserialize(deserializer)? {
        Stored::Legacy(aliases) => Ok(aliases),
        Stored::Journal(reference) if reference == REFERENCE => Ok(Vec::new()),
        Stored::Journal(_) => Err(serde::de::Error::custom("the composer identity journal format is not supported")),
    }
}
impl Document {
    pub fn metadata_dirty(&self) -> bool {
        self.aliases_changed || !self.aliases_persisted || self.checkpoint_migration.is_some()
    }
    pub fn load_aliases(&mut self, bytes: Option<Vec<u8>>) -> Result<(), Failure> {
        if let Some(bytes) = bytes {
            if !self.aliases.is_empty() {
                return Err(failure("the composer has two legacy identity authorities"));
            }
            self.aliases = serde_json::from_slice(&bytes).map_err(failure)?;
            self.aliases_persisted = true;
        } else {
            self.aliases_persisted = self.aliases.is_empty();
        }
        Ok(())
    }
    pub fn alias_checkpoint(&mut self) -> Result<Option<Vec<u8>>, Failure> {
        if self.aliases_persisted && !self.aliases_changed {
            return Ok(None);
        }
        let bytes = serde_json::to_vec(&self.aliases).map_err(failure)?;
        self.aliases_persisted = true;
        self.aliases_changed = false;
        Ok(Some(bytes))
    }
    /// Compatibility edits may pay one linear reconciliation; native updates
    /// never visit the old identity array on their editing path.
    pub fn reconcile_legacy(&mut self) {
        self.prune_aliases();
    }
}
