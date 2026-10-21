//! Persisted edit identities and delivery reservations, separate from view snapshots.

use std::collections::{HashMap, HashSet};

use fastctx_types::composer::{Change, Character, CharacterId, Composer, Draft, Mutation, Queued, QueuedCommand, Target};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Landed};
use fastctx_types::{MachineName, NoticeId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Document {
    pub epoch: u64,
    atoms: Vec<Atom>,
    pub attachments: Vec<fastctx_types::attachments::Attachment>,
    pub uploads: Vec<fastctx_types::composer::Uploading>,
    #[serde(default)]
    pub files_on: Option<fastctx_types::access::Written>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Atom {
    character: Character,
    deleted: bool,
}

impl Document {
    pub fn view(&self) -> Draft {
        let characters: Vec<_> = self.atoms.iter().filter(|a| !a.deleted).map(|a| a.character.clone()).collect();
        Draft {
            epoch: self.epoch,
            text: characters.iter().map(|c| c.text.as_str()).collect(),
            characters,
            attachments: self.attachments.clone(),
            uploads: self.uploads.clone(),
            files_on: self.files_on.clone(),
        }
    }

    pub fn edit(
        &mut self,
        operation: &str,
        after: &Option<CharacterId>,
        delete: &[CharacterId],
        insert: &str,
    ) -> Result<(), Failure> {
        let position = match after {
            None => 0,
            Some(id) => {
                self.atoms
                    .iter()
                    .position(|a| &a.character.id == id)
                    .ok_or_else(|| Failure::new("the insertion anchor is not in this draft"))?
                    + 1
            }
        };
        if !delete.is_empty() {
            let known: HashSet<_> = self.atoms.iter().map(|atom| &atom.character.id).collect();
            if delete.iter().any(|id| !known.contains(id)) {
                return Err(Failure::new("a deleted character is not in this draft"));
            }
            let deleted: HashSet<_> = delete.iter().collect();
            for atom in &mut self.atoms {
                if deleted.contains(&atom.character.id) {
                    atom.deleted = true;
                }
            }
        }
        self.atoms.splice(
            position..position,
            insert.chars().enumerate().map(|(index, c)| Atom {
                character: Character { id: CharacterId { operation: operation.into(), index }, text: c.to_string() },
                deleted: false,
            }),
        );
        Ok(())
    }

    /// Takes back words that could not go: they come before whatever was
    /// typed since, on a line of their own, with their files. `line` names
    /// the line break put between them, a fresh operation of the node's.
    pub fn take_back(&mut self, words: Document, line: &str) {
        let mut back: Vec<Atom> = words.atoms.into_iter().filter(|atom| !atom.deleted).collect();
        let typed = self.atoms.iter().any(|atom| !atom.deleted);
        if typed && back.last().is_some_and(|atom| atom.character.text != "\n") {
            back.push(Atom {
                character: Character { id: CharacterId { operation: line.into(), index: 0 }, text: "\n".into() },
                deleted: false,
            });
        }
        self.atoms.splice(0..0, back);
        for attachment in words.attachments {
            if !self.attachments.iter().any(|kept| kept.id == attachment.id) {
                self.attachments.push(attachment);
            }
        }
        if self.files_on.is_none() {
            self.files_on = words.files_on;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    pub failure: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stored {
    pub revision: u64,
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
}

impl Stored {
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
        let flight = Flight { entry: id.clone(), delivery, automatic, idle_only, files_on: draft.files_on };
        self.sending = Some(flight.clone());
        if automatic {
            self.opportunity = None;
        }
        Ok(flight)
    }
}
