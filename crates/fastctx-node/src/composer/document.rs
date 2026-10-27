//! One document's text journal and attachment metadata. Text is moved with its
//! owning composer while changing; failed writes discard it and reload authority.

use super::failure;
use fastctx_types::composer::{Character, CharacterId, Draft};
use fastctx_types::ipc::Failure;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub(super) struct Document {
    #[serde(default)]
    pub id: String,
    pub epoch: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    atoms: Vec<Atom>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<Alias>,
    pub attachments: Vec<fastctx_types::attachments::Attachment>,
    pub uploads: Vec<fastctx_types::composer::Uploading>,
    #[serde(default)]
    pub files_on: Option<fastctx_types::access::Written>,
    #[serde(skip)]
    live: Option<fastctx_draft::Document>,
    #[serde(skip)]
    pub revision: u64,
    #[serde(skip)]
    pub tail_updates: usize,
    #[serde(skip)]
    pub tail_bytes: usize,
    #[serde(skip)]
    pub pending: Vec<Vec<u8>>,
}
#[derive(Debug, Serialize, Deserialize)]
struct Atom {
    character: Character,
    deleted: bool,
}
#[derive(Debug, Serialize, Deserialize)]
struct Alias {
    old: CharacterId,
    client: u64,
    clock: u32,
}
impl std::fmt::Debug for Document {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Document").field("id", &self.id).field("epoch", &self.epoch).finish_non_exhaustive()
    }
}
impl Default for Document {
    fn default() -> Self {
        Self {
            id: crate::speech::new_id("document").to_string(),
            epoch: 0,
            atoms: Vec::new(),
            aliases: Vec::new(),
            attachments: Vec::new(),
            uploads: Vec::new(),
            files_on: None,
            live: Some(fastctx_draft::Document::new()),
            revision: 0,
            tail_updates: 0,
            tail_bytes: 0,
            pending: Vec::new(),
        }
    }
}
impl Document {
    pub fn hydrate(&mut self, loaded: fastctx_storage::documents::Loaded) -> Result<(), Failure> {
        if self.id.is_empty() {
            self.id = crate::speech::new_id("document").to_string();
        }
        self.revision = loaded.revision;
        self.tail_updates = loaded.updates.len();
        self.tail_bytes = loaded.tail_bytes();
        let mut live = if loaded.checkpoint.is_empty() {
            fastctx_draft::Document::new()
        } else {
            fastctx_draft::Document::restore(&loaded.checkpoint).map_err(failure)?
        };
        for update in loaded.updates {
            live.apply(&update).map_err(failure)?;
        }
        if !self.atoms.is_empty() {
            if self.revision != 0 {
                return Err(failure("the legacy draft and its text journal both contain data"));
            }
            let value: String = self.atoms.iter().map(|atom| atom.character.text.as_str()).collect();
            let update = live.splice(0, 0, &value).map_err(failure)?;
            self.pending.push(update);
            let characters = live.characters();
            if characters.len() != self.atoms.len() {
                return Err(failure("a legacy draft character is not one Unicode scalar"));
            }
            let mut deleted = Vec::new();
            for (atom, character) in self.atoms.drain(..).zip(characters) {
                if atom.deleted {
                    deleted.push(character.id);
                }
                self.aliases.push(Alias {
                    old: atom.character.id,
                    client: character.id.client,
                    clock: character.id.clock,
                });
            }
            if !deleted.is_empty() {
                self.pending.push(live.edit(None, &deleted, "").map_err(failure)?.update);
            }
        }
        self.live = Some(live);
        Ok(())
    }
    fn text(&self) -> &fastctx_draft::Document {
        self.live.as_ref().expect("hydrated draft")
    }
    fn text_mut(&mut self) -> &mut fastctx_draft::Document {
        self.live.as_mut().expect("hydrated draft")
    }
    pub fn view(&self) -> Draft {
        let aliases: HashMap<_, _> =
            self.aliases.iter().map(|alias| ((alias.client, alias.clock), &alias.old)).collect();
        let characters: Vec<_> = self
            .text()
            .characters()
            .into_iter()
            .map(|character| {
                let id = aliases
                    .get(&(character.id.client, character.id.clock))
                    .map(|id| (*id).clone())
                    .unwrap_or_else(|| CharacterId {
                        operation: format!("yjs:{}:{}", character.id.client, character.id.clock),
                        index: 0,
                    });
                Character { id, text: character.text }
            })
            .collect();
        Draft {
            epoch: self.epoch,
            text: characters.iter().map(|c| c.text.as_str()).collect(),
            characters,
            attachments: self.attachments.clone(),
            uploads: self.uploads.clone(),
            files_on: self.files_on.clone(),
        }
    }
    fn identity(&self, id: &CharacterId) -> Result<fastctx_draft::Id, Failure> {
        if let Some(alias) = self.aliases.iter().find(|alias| &alias.old == id) {
            return Ok(fastctx_draft::Id { client: alias.client, clock: alias.clock });
        }
        let (client, clock) = id
            .operation
            .strip_prefix("yjs:")
            .and_then(|value| value.split_once(':'))
            .filter(|_| id.index == 0)
            .ok_or_else(|| failure("the character is not in this draft"))?;
        let client = client.parse().map_err(failure)?;
        let clock = clock.parse().map_err(failure)?;
        Ok(fastctx_draft::Id { client, clock })
    }
    pub fn edit(
        &mut self,
        operation: &str,
        after: &Option<CharacterId>,
        delete: &[CharacterId],
        insert: &str,
    ) -> Result<(), Failure> {
        // An edit that inserted is known by its characters: sent again once
        // its receipt has gone (`keeping`), it is not done twice.
        if self.aliases.iter().any(|alias| alias.old.operation == operation) {
            return Ok(());
        }
        let after = after.as_ref().map(|id| self.identity(id)).transpose()?;
        let delete = delete.iter().map(|id| self.identity(id)).collect::<Result<Vec<_>, _>>()?;
        let edited = self.text_mut().edit(after, &delete, insert).map_err(failure)?;
        self.pending.push(edited.update);
        for (index, character) in edited.inserted.into_iter().enumerate() {
            self.aliases.push(Alias {
                old: CharacterId { operation: operation.into(), index },
                client: character.id.client,
                clock: character.id.clock,
            });
        }
        Ok(())
    }
    pub fn metadata(&self) -> fastctx_types::composer::sync::Document {
        fastctx_types::composer::sync::Document {
            id: self.id.clone(),
            revision: self.revision,
            epoch: self.epoch,
            attachments: self.attachments.clone(),
            uploads: self.uploads.clone(),
            files_on: self.files_on.clone(),
        }
    }
    pub fn update_for(&self, vector: Option<&[u8]>) -> Result<Vec<u8>, Failure> {
        self.text().update_for(vector).map_err(failure)
    }
    pub fn merge(&mut self, update: &[u8]) -> Result<(), Failure> {
        self.text_mut().apply(update).map_err(failure)?;
        self.pending.push(update.to_vec());
        Ok(())
    }

    /// Capture uses the sender's replica, so authority learned meanwhile remains
    /// in the original document. Unselected uploads keep their original epoch.
    pub fn take_snapshot(&mut self, snapshot: &[u8], attachments: &[fastctx_types::NoticeId]) -> Result<Self, Failure> {
        let selected: std::collections::HashSet<_> = attachments.iter().collect();
        if selected.len() != attachments.len() {
            return Err(failure("the send repeats an attachment identity"));
        }
        let files = attachments
            .iter()
            .map(|id| {
                self.attachments
                    .iter()
                    .find(|file| &file.id == id)
                    .cloned()
                    .ok_or_else(|| failure("an attachment selected for sending is no longer on this draft"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut frozen = fastctx_draft::Document::restore(snapshot).map_err(failure)?;
        let text = frozen.text();
        if text.trim().is_empty() && files.is_empty() {
            return Err(failure("the captured draft is empty"));
        }
        let missing = frozen.update_for(Some(&self.text().state_vector())).map_err(failure)?;
        let taken = frozen.take();
        let mut queued = Self { attachments: files, files_on: self.files_on.clone(), ..Self::default() };
        let update = queued.text_mut().splice(0, 0, &text).map_err(failure)?;
        queued.pending.push(update);
        self.merge(&missing)?;
        self.merge(&taken.update)?;
        self.attachments.retain(|file| !selected.contains(&file.id));
        if self.attachments.is_empty() && self.uploads.is_empty() && self.text().is_empty() {
            self.files_on = None;
        }
        Ok(queued)
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        self.text().checkpoint()
    }

    pub fn from_view(view: Draft) -> Result<Self, Failure> {
        let mut document = Self {
            epoch: view.epoch,
            attachments: view.attachments,
            uploads: view.uploads,
            files_on: view.files_on,
            ..Self::default()
        };
        let update = document.text_mut().splice(0, 0, &view.text).map_err(failure)?;
        document.pending.push(update);
        for (old, character) in view.characters.into_iter().zip(document.text().characters()) {
            document.aliases.push(Alias { old: old.id, client: character.id.client, clock: character.id.clock });
        }
        Ok(document)
    }
    pub fn take(&mut self) -> Result<Self, Failure> {
        let view = self.view();
        let queued = Self::from_view(view)?;
        let taken = self.text_mut().take();
        self.pending.push(taken.update);
        self.attachments.clear();
        self.uploads.clear();
        self.files_on = None;
        self.aliases.clear();
        self.epoch = self.epoch.checked_add(1).ok_or_else(|| failure("draft epoch overflow"))?;
        Ok(queued)
    }
    pub fn take_back(&mut self, words: Document, _line: &str) -> Result<(), Failure> {
        let words = words.view();
        let mut value = words.text;
        if !self.text().is_empty() && !value.ends_with('\n') {
            value.push('\n');
        }
        let update = self.text_mut().splice(0, 0, &value).map_err(failure)?;
        self.pending.push(update);
        for attachment in words.attachments {
            if !self.attachments.iter().any(|kept| kept.id == attachment.id) {
                self.attachments.push(attachment);
            }
        }
        if self.files_on.is_none() {
            self.files_on = words.files_on;
        }
        Ok(())
    }
}
