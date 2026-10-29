//! Persisted edit identities and delivery reservations, separate from view snapshots.

use std::collections::{HashMap, HashSet};

use fastctx_types::composer::{Change, Composer, Mutation, Queued, QueuedCommand, Target};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Landed};
use fastctx_types::{MachineName, NoticeId};
use serde::{Deserialize, Serialize};

pub(super) use super::document::Document;

#[derive(Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: NoticeId,
    pub from: MachineName,
    pub document: Document,
    #[serde(skip)]
    pub editors: HashSet<String>,
    pub error: Option<String>,
    pub submitted: Option<NoticeId>,
    /// The place of a command the node took to do once a turn ends
    /// (`control_queue`), its id the command's: it has no words, and is
    /// moved among the words but never sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub command: bool,
}

impl Entry {
    pub fn command(id: NoticeId, from: MachineName) -> Entry {
        Entry {
            id,
            from,
            document: Document::default(),
            editors: HashSet::new(),
            error: None,
            submitted: None,
            command: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Flight {
    pub entry: NoticeId,
    pub delivery: Delivery,
    pub automatic: bool,
    pub idle_only: bool,
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub files_on: Option<fastctx_types::access::Written>,
}

impl Flight {
    /// Sent as the user sent it, the agent idle and nothing on its way
    /// before it: these words never waited. A queued message sent at once
    /// is not idle-only, and the queue's own release is automatic.
    pub fn straight(&self) -> bool {
        !self.automatic && self.idle_only
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub from: MachineName,
    pub mutation: Mutation,
    pub landed: Option<Landed>,
    /// The id the words it sent went under, told again with a repeated answer.
    #[serde(default)]
    pub delivery: Option<NoticeId>,
    pub failure: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Stored {
    pub revision: u64,
    #[serde(skip)]
    pub persisted: bool,
    #[serde(skip)]
    pub document_names: HashSet<String>,
    #[serde(skip)]
    pub observed_documents: HashSet<String>,
    #[serde(skip)]
    queued_documents: HashMap<String, usize>,
    #[serde(skip)]
    pending_entries: HashSet<NoticeId>,
    pub draft: Document,
    pub queue: Vec<Entry>,
    pub unobserved: Vec<Entry>,
    pub held: Option<String>,
    pub awaiting_turn: Option<String>,
    pub sending: Option<Flight>,
    pub pending: Vec<Flight>,
    pub opportunity: Option<String>,
    #[serde(default)]
    pub settled_through: u64,
    pub ended: HashSet<String>,
    pub receipts: HashMap<String, Receipt>,
    pub uploads: HashMap<String, super::uploads::UploadRecord>,
    /// The edits each sent epoch of the draft took, by `keeping::taken_key`,
    /// till they are written as records of their own.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub taken: HashMap<String, Vec<String>>,
}

impl Stored {
    pub fn reindex(&mut self) {
        self.queued_documents = self
            .queue
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.command)
            .map(|(index, entry)| (entry.document.id.clone(), index))
            .collect();
        self.pending_entries = self.pending.iter().map(|flight| flight.entry.clone()).collect();
    }
    pub fn editable_document(&mut self, document: &str) -> Result<&mut Document, Failure> {
        if self.draft.id == document {
            return Ok(&mut self.draft);
        }
        let index = self
            .queued_documents
            .get(document)
            .copied()
            .ok_or_else(|| Failure::new("that shared document no longer exists"))?;
        let entry = &mut self.queue[index];
        if self.pending_entries.contains(&entry.id)
            || self.sending.as_ref().is_some_and(|flight| flight.entry == entry.id)
        {
            return Err(Failure::new("that queued message is being delivered"));
        }
        Ok(&mut entry.document)
    }

    pub fn visible_document_ids(&self) -> HashSet<String> {
        std::iter::once(self.draft.id.clone())
            .chain(
                self.queue
                    .iter()
                    .filter(|entry| !entry.command && !self.going_straight(&entry.id))
                    .map(|entry| entry.document.id.clone()),
            )
            .chain(self.unobserved.iter().map(|entry| entry.document.id.clone()))
            .collect()
    }

    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        std::iter::once(&self.draft).chain(self.queue.iter().chain(&self.unobserved).map(|entry| &entry.document))
    }
    pub fn documents_mut(&mut self) -> impl Iterator<Item = &mut Document> {
        std::iter::once(&mut self.draft)
            .chain(self.queue.iter_mut().chain(&mut self.unobserved).map(|entry| &mut entry.document))
    }

    pub fn retire_unobserved(&mut self, id: &NoticeId) {
        if let Some(position) = self.queue.iter().position(|entry| &entry.id == id) {
            self.unobserved.push(self.queue.remove(position));
        }
    }

    /// Whether these words are on their way as sent straight on.
    fn going_straight(&self, id: &NoticeId) -> bool {
        self.sending.iter().chain(self.pending.iter()).any(|flight| &flight.entry == id && flight.straight())
    }

    pub fn view(&self) -> Composer {
        Composer {
            revision: self.revision,
            draft: self.draft.view(),
            // Words sent straight on are not shown waiting while they go:
            // the device that sent them shows them in the conversation.
            // Ones a turn begun meanwhile keeps out stay in the queue, shown
            // from then on; ones that fail go back to the input.
            queue: self
                .queue
                .iter()
                .filter(|entry| !entry.command && !self.going_straight(&entry.id))
                .map(|entry| Queued {
                    id: entry.id.clone(),
                    from: entry.from.clone(),
                    draft: entry.document.view(),
                    editing: !entry.editors.is_empty(),
                    sending: self.sending.iter().chain(self.pending.iter()).any(|flight| flight.entry == entry.id),
                    error: entry.error.clone(),
                })
                .collect(),
            commands: self.commands(),
            unobserved: self
                .unobserved
                .iter()
                .map(|entry| Queued {
                    id: entry.id.clone(),
                    from: entry.from.clone(),
                    draft: entry.document.view(),
                    editing: false,
                    sending: false,
                    error: entry.error.clone(),
                })
                .collect(),
            held: self.held.clone(),
            awaiting_turn: self.awaiting_turn.clone(),
        }
    }

    pub fn metadata(&self) -> fastctx_types::composer::sync::State {
        use fastctx_types::composer::sync::{Queued, State};
        let queued = |entry: &Entry, observed| Queued {
            id: entry.id.clone(),
            from: entry.from.clone(),
            document: entry.document.metadata(),
            editing: observed && !entry.editors.is_empty(),
            sending: observed && self.sending.iter().chain(&self.pending).any(|flight| flight.entry == entry.id),
            error: entry.error.clone(),
        };
        State {
            revision: self.revision,
            draft: self.draft.metadata(),
            queue: self
                .queue
                .iter()
                .filter(|entry| !entry.command && !self.going_straight(&entry.id))
                .map(|entry| queued(entry, true))
                .collect(),
            commands: self.commands(),
            unobserved: self.unobserved.iter().map(|entry| queued(entry, false)).collect(),
            held: self.held.clone(),
            awaiting_turn: self.awaiting_turn.clone(),
        }
    }

    /// Each command's place: after the last of the words shown before it.
    fn commands(&self) -> Vec<QueuedCommand> {
        let mut after = None;
        let mut commands = Vec::new();
        for entry in &self.queue {
            if entry.command {
                commands.push(QueuedCommand { id: entry.id.clone(), after: after.clone() });
            } else if !self.going_straight(&entry.id) {
                after = Some(entry.id.clone());
            }
        }
        commands
    }

    /// Queued words, for what is done to words.
    pub fn entry(&mut self, id: &NoticeId) -> Result<&mut Entry, Failure> {
        let entry = self.movable(id)?;
        if entry.command {
            return Err(Failure::new("that is a waiting command; withdraw it instead"));
        }
        Ok(entry)
    }

    /// Queued words or a command's place, for moving.
    fn movable(&mut self, id: &NoticeId) -> Result<&mut Entry, Failure> {
        if self.sending.iter().chain(self.pending.iter()).any(|flight| &flight.entry == id) {
            return Err(Failure::new("that queued message is being delivered"));
        }
        self.queue.iter_mut().find(|e| &e.id == id).ok_or_else(|| Failure::new("that queued message no longer exists"))
    }

    pub fn document(&mut self, target: &Target, epoch: u64) -> Result<&mut Document, Failure> {
        let document = match target {
            Target::Draft => &mut self.draft,
            Target::Queued { id } => &mut self.entry(id)?.document,
        };
        if document.epoch != epoch {
            return Err(Failure::new(
                "this draft was already sent; keep your local edit and apply it to the new epoch",
            ));
        }
        Ok(document)
    }

    pub fn edit(&mut self, mutation: &Mutation) -> Result<(), Failure> {
        match &mutation.change {
            Change::Edit { target, epoch, after, delete, insert } => {
                self.document(target, *epoch)?.edit(&mutation.operation, after, delete, insert)
            }
            Change::Attach { target, epoch, attachment } => {
                let document = self.document(target, *epoch)?;
                if !document.attachments.iter().any(|a| a.id == attachment.id) {
                    document.attachments.push(attachment.clone());
                }
                Ok(())
            }
            Change::Detach { target, epoch, attachment } => {
                let document = self.document(target, *epoch)?;
                document.attachments.retain(|a| &a.id != attachment);
                document.uploads.retain(|upload| &upload.id != attachment);
                Ok(())
            }
            Change::FilesOn { target, epoch, files_on } => {
                self.document(target, *epoch)?.files_on = files_on.clone();
                Ok(())
            }
            Change::Delete { id } => {
                if self.unobserved.iter().any(|entry| &entry.id == id) {
                    self.unobserved.retain(|entry| &entry.id != id);
                    return Ok(());
                }
                self.entry(id)?;
                self.queue.retain(|e| &e.id != id);
                Ok(())
            }
            Change::Move { id, before } => {
                self.movable(id)?;
                if before.as_ref() == Some(id) {
                    return Ok(());
                }
                if let Some(before) = before {
                    self.movable(before)?;
                }
                let position = self.queue.iter().position(|e| &e.id == id).expect("checked entry");
                let entry = self.queue.remove(position);
                let position = before
                    .as_ref()
                    .and_then(|id| self.queue.iter().position(|e| &e.id == id))
                    .unwrap_or(self.queue.len());
                self.queue.insert(position, entry);
                Ok(())
            }
            _ => Err(Failure::new("this operation requires the delivery coordinator")),
        }
    }

    pub fn reserve(&mut self, id: &NoticeId, automatic: bool, idle_only: bool) -> Result<Flight, Failure> {
        if self.sending.is_some() || (automatic && !self.pending.is_empty()) {
            return Err(Failure::new("another composer message is being delivered"));
        }
        let entry = self.entry(id)?;
        if !entry.document.uploads.is_empty() {
            return Err(Failure::new("this message still has an unfinished attachment upload"));
        }
        if entry.submitted.is_some() {
            return Err(Failure::new(
                "this message crossed the host boundary without a known acceptance; it cannot be submitted again",
            ));
        }
        let draft = entry.document.view();
        let delivery = crate::attachments::delivery(&entry.from, draft.text, draft.attachments, None);
        let flight =
            Flight { entry: id.clone(), delivery, automatic, idle_only, files_on: draft.files_on, operation: None };
        self.sending = Some(flight.clone());
        if automatic {
            self.opportunity = None;
        }
        Ok(flight)
    }
}
