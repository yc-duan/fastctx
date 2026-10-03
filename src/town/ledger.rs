//! Writing a town's ledger (`specs/world-projects.md`, `world-journals.md`, `world-network.md`
//! "共享账本"): a project's charter, its journals' descriptions and their entries. The node
//! stamps what it knows and signs; the hub orders the town's entries, and every machine of the
//! town gets them. Nothing written is ever changed: an edited charter or description is a new
//! version, a correction is a new entry.

use super::core::Town;
use super::journal::{self, Attachment, Description, Stamp};
use super::project::{self, Charter, Location, Place};
use super::remote::valid_town;
use crate::world::ledger::{self as shared, LedgerFile, ProjectUpdate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// A charter as its writer gives it; on an edit, the fields left out stay as they are.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CharterChange {
    pub(crate) name: Option<String>,
    pub(crate) what: Option<String>,
    pub(crate) machines: Option<Vec<String>>,
    /// The whole address book, replacing the one before.
    pub(crate) places: Option<Vec<Place>>,
    pub(crate) canon: Option<Location>,
    /// Remove where the canon lives.
    #[serde(default)]
    pub(crate) no_canon: bool,
}

/// A journal entry as its writer hands it over.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct EntryDraft {
    pub(crate) town: String,
    pub(crate) journal: String,
    pub(crate) text: String,
    /// What a new journal covers; its first entry brings it.
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) cites: Vec<String>,
    /// Absolute paths of files on this machine to attach.
    #[serde(default)]
    pub(crate) attachments: Vec<PathBuf>,
}

/// An attachment ready to be written: inside the entry, or named where it stays.
struct Attached {
    name: String,
    bytes: Vec<u8>,
    origin: PathBuf,
    inline: bool,
}

impl Town {
    /// Opens a project (`opening`) or writes the next version of its charter. `writer` is the
    /// resident writing it, `None` for the user.
    pub(crate) async fn write_charter(&self, town: &str, writer: Option<&str>, change: CharterChange, opening: bool) -> Result<Charter, String> {
        if !valid_town(town) {
            return Err(format!("A town's short name is lowercase letters, digits and hyphens, such as mark50; got \"{town}\"."));
        }
        // Edits build on the newest version the hub has, not only on what reached this machine.
        self.world().sync_ledger().await?;
        let versions = match project::charters(self.world_dir(), town) {
            Ok(versions) => versions,
            Err(_) if opening => Vec::new(),
            Err(error) => return Err(error),
        };
        let current = versions.last();
        match (opening, current) {
            (true, Some(_)) => return Err(format!("The town {town} is open already; fastctx project edit changes its charter.")),
            (false, None) => return Err(format!("The town {town} has no charter yet; fastctx project new opens it.")),
            _ => {}
        }
        let name = change.name.or_else(|| current.map(|charter| charter.name.clone())).ok_or("A project needs a name (--name).")?;
        let what = change.what.or_else(|| current.map(|charter| charter.what.clone())).ok_or("A project needs to say what it is (--what).")?;
        let machines = change.machines.or_else(|| current.map(|charter| charter.machines.clone())).ok_or("A project needs its machines (--machines).")?;
        let members: BTreeSet<String> = self.world().all_node_views().into_iter().map(|node| node.name).collect();
        let unknown: Vec<&String> = machines.iter().filter(|machine| !members.contains(*machine)).collect();
        if machines.is_empty() || !unknown.is_empty() {
            return Err(format!(
                "A project's machines are machines of this World ({}); not: {}.",
                members.iter().cloned().collect::<Vec<_>>().join(", "),
                unknown.iter().map(|machine| machine.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        let canon = if change.no_canon { None } else { change.canon.or_else(|| current.and_then(|charter| charter.canon.clone())) };
        let now = crate::world::now_rfc3339();
        let author = writer.unwrap_or("user").to_string();
        let version = versions.len() as u64 + 1;
        let charter = Charter {
            version,
            name,
            what,
            machines,
            places: change.places.or_else(|| current.map(|charter| charter.places.clone())).unwrap_or_default(),
            canon,
            opened_by: current.map_or_else(|| author.clone(), |charter| charter.opened_by.clone()),
            opened_at: current.map_or_else(|| now.clone(), |charter| charter.opened_at.clone()),
            written_by: author,
            written_on: self.machine().to_string(),
            written_at: now,
        };
        let path = project::charter_path(version);
        let bytes = serde_json::to_vec_pretty(&charter).map_err(|error| format!("Cannot encode the charter: {error}"))?;
        let update = ProjectUpdate { path: path.clone(), previous: version - 1 };
        self.world().append_ledger(town.to_string(), vec![LedgerFile { path, bytes }], Some(update)).await?;
        Ok(charter)
    }

    /// Adds an entry to a journal, starting the journal when it is new. Returns the entry's id.
    /// Attachments travel inside the entry while it fits the one message the hub takes it in;
    /// the largest of the rest stay where they are and the entry names them.
    pub(crate) async fn add_entry(&self, writer: Option<&str>, draft: EntryDraft) -> Result<String, String> {
        let EntryDraft { town, journal: name, text, description, cites, attachments } = draft;
        journal::validate_name(&name)?;
        project::charter(self.world_dir(), &town)?;
        let fresh = journal::descriptions(self.world_dir(), &town, &name).is_empty() && journal::entries(self.world_dir(), &town, &name).is_empty();
        match (fresh, &description) {
            (true, None) => {
                return Err(format!("{name} is a new journal; say what it covers with --description (its scope, not what it found)."));
            }
            (false, Some(_)) => return Err(format!("{name} has a description already; change it with fastctx journal describe.")),
            _ => {}
        }
        if text.trim().is_empty() {
            return Err("The entry has no text.".to_string());
        }
        let known: BTreeSet<String> = journal::names(self.world_dir(), &town)
            .iter()
            .flat_map(|journal| journal::entries(self.world_dir(), &town, journal))
            .map(|entry| entry.stamp.id)
            .collect();
        if let Some(unknown) = cites.iter().find(|cited| !known.contains(*cited)) {
            return Err(format!("The town {town} has no entry {unknown} to answer."));
        }
        let mut attached = Vec::new();
        for path in attachments {
            let name = path.file_name().and_then(|name| name.to_str()).ok_or_else(|| format!("{} has no file name.", path.display()))?.to_string();
            if attached.iter().any(|existing: &Attached| existing.name == name) {
                return Err(format!("Two attachments are named {name}; attach files with different names."));
            }
            let bytes = std::fs::read(&path).map_err(|error| format!("Cannot read {}: {error}", crate::paths::display_path(&path)))?;
            attached.push(Attached { name, bytes, origin: path, inline: true });
        }
        let id = self.entry_id(&town, &name);
        let about = writer.and_then(|resident| self.about_of(resident));
        let at = crate::world::now_rfc3339();
        let author = writer.unwrap_or("user");
        loop {
            let stamp = Stamp {
                id: id.clone(),
                at: at.clone(),
                machine: self.machine().to_string(),
                resident: author.to_string(),
                agent: match (&about, writer) {
                    (Some(about), _) => about.agent.clone(),
                    (None, None) => "user".to_string(),
                    (None, Some(_)) => "unknown".to_string(),
                },
                agent_version: about.as_ref().and_then(|about| about.agent_version.clone()),
                model: about.as_ref().and_then(|about| about.model.clone()),
                model_source: about.as_ref().and_then(|about| about.model_source.clone()),
                session: about.as_ref().and_then(|about| about.session.clone()),
                cites: cites.clone(),
                attachments: attached
                    .iter()
                    .map(|file| Attachment {
                        name: file.name.clone(),
                        bytes: file.bytes.len() as u64,
                        machine: self.machine().to_string(),
                        path: (!file.inline).then(|| crate::paths::display_path(&file.origin)),
                    })
                    .collect(),
            };
            let base = format!("journals/{name}/entries/{id}");
            let mut files = vec![
                LedgerFile { path: format!("{base}/text.md"), bytes: format!("{}\n", text.trim_end()).into_bytes() },
                LedgerFile {
                    path: format!("{base}/entry.json"),
                    bytes: serde_json::to_vec_pretty(&stamp).map_err(|error| format!("Cannot encode the entry's stamp: {error}"))?,
                },
            ];
            for file in attached.iter().filter(|file| file.inline) {
                files.push(LedgerFile { path: format!("{base}/attachments/{}", file.name), bytes: file.bytes.clone() });
            }
            if let Some(description) = &description {
                let first = Description {
                    version: 1,
                    text: description.trim().to_string(),
                    written_by: author.to_string(),
                    written_on: self.machine().to_string(),
                    written_at: at.clone(),
                };
                files.push(LedgerFile {
                    path: format!("journals/{name}/description/v000001.json"),
                    bytes: serde_json::to_vec_pretty(&first).map_err(|error| format!("Cannot encode the description: {error}"))?,
                });
            }
            let entry = shared::Entry { scope: town.clone(), id: String::new(), author: self.machine().to_string(), files: files.clone(), project: None };
            if shared::fits(&entry)? {
                self.world().append_ledger(town.clone(), files, None).await?;
                return Ok(id);
            }
            match attached.iter_mut().filter(|file| file.inline).max_by_key(|file| file.bytes.len()) {
                Some(largest) => largest.inline = false,
                None => {
                    return Err(format!(
                        "The entry's text is {} bytes, more than one journal entry carries; split it into several entries.",
                        text.len()
                    ));
                }
            }
        }
    }

    /// Writes the next version of a journal's description. Returns its version.
    pub(crate) async fn describe_journal(&self, writer: Option<&str>, town: &str, name: &str, text: &str) -> Result<u64, String> {
        journal::validate_name(name)?;
        self.world().sync_ledger().await?;
        let versions = journal::descriptions(self.world_dir(), town, name);
        if versions.is_empty() && journal::entries(self.world_dir(), town, name).is_empty() {
            return Err(format!("The town {town} has no journal named {name}."));
        }
        let version = versions.len() as u64 + 1;
        let description = Description {
            version,
            text: text.trim().to_string(),
            written_by: writer.unwrap_or("user").to_string(),
            written_on: self.machine().to_string(),
            written_at: crate::world::now_rfc3339(),
        };
        let bytes = serde_json::to_vec_pretty(&description).map_err(|error| format!("Cannot encode the description: {error}"))?;
        // Two writers of the same version write the same path, and the ledger takes only the
        // first: a description is never overwritten unseen.
        let path = format!("journals/{name}/description/v{version:06}.json");
        self.world().append_ledger(town.to_string(), vec![LedgerFile { path, bytes }], None).await?;
        Ok(version)
    }

    /// A new entry's id: the second it is written and this machine, the next second when that
    /// one is taken here.
    fn entry_id(&self, town: &str, journal: &str) -> String {
        let format = time::macros::format_description!("[year][month][day]T[hour][minute][second]Z");
        let entries = journal::journals_dir(self.world_dir(), town).join(journal).join("entries");
        let mut at = time::OffsetDateTime::now_utc();
        loop {
            let id = format!("{}-{}", at.format(&format).expect("a UTC time formats"), self.machine());
            if !entries.join(&id).exists() {
                return id;
            }
            at += time::Duration::seconds(1);
        }
    }
}
