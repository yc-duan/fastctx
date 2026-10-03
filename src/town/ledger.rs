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
use crate::world::messages::{CallBudget, CallOrigin};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long fetching an attachment from another machine may take: what `copy` gives a copy
/// when its caller names no limit.
const FETCH_TIMEOUT: Duration =
    Duration::from_millis(crate::world::surface::DEFAULT_COPY_TIMEOUT_MS);

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
    /// Files on other machines of the World to attach.
    #[serde(default)]
    pub(crate) remote_attachments: Vec<RemoteFile>,
}

/// A file on another machine: the machine, and the file's native path there.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RemoteFile {
    pub(crate) machine: String,
    pub(crate) path: String,
}

/// An attachment on its way into an entry: inside it while it fits, else kept on this machine.
struct Attached {
    name: String,
    size: u64,
    /// Where its bytes are on this machine now.
    origin: PathBuf,
    /// Its bytes, read when it may still travel inside the entry.
    bytes: Option<Vec<u8>>,
    /// Already a copy under the World's keeping, not the writer's own file.
    kept: bool,
}

impl Town {
    /// Opens a project (`opening`) or writes the next version of its charter. `writer` is the
    /// resident writing it, `None` for the user.
    pub(crate) async fn write_charter(
        &self,
        town: &str,
        writer: Option<&str>,
        change: CharterChange,
        opening: bool,
    ) -> Result<Charter, String> {
        if !valid_town(town) {
            return Err(format!(
                "A town's short name is lowercase letters, digits and hyphens, such as mark50; got \"{town}\"."
            ));
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
            (true, Some(_)) => {
                return Err(format!(
                    "The town {town} is open already; fastctx project edit changes its charter."
                ));
            }
            (false, None) => {
                return Err(format!(
                    "The town {town} has no charter yet; fastctx project new opens it."
                ));
            }
            _ => {}
        }
        let name = change
            .name
            .or_else(|| current.map(|charter| charter.name.clone()))
            .ok_or("A project needs a name (--name).")?;
        let what = change
            .what
            .or_else(|| current.map(|charter| charter.what.clone()))
            .ok_or("A project needs to say what it is (--what).")?;
        let machines = change
            .machines
            .or_else(|| current.map(|charter| charter.machines.clone()))
            .ok_or("A project needs its machines (--machines).")?;
        let members: BTreeSet<String> = self
            .world()
            .all_node_views()
            .into_iter()
            .map(|node| node.name)
            .collect();
        let unknown: Vec<&String> = machines
            .iter()
            .filter(|machine| !members.contains(*machine))
            .collect();
        if machines.is_empty() || !unknown.is_empty() {
            return Err(format!(
                "A project's machines are machines of this World ({}); not: {}.",
                members.iter().cloned().collect::<Vec<_>>().join(", "),
                unknown
                    .iter()
                    .map(|machine| machine.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let canon = if change.no_canon {
            None
        } else {
            change
                .canon
                .or_else(|| current.and_then(|charter| charter.canon.clone()))
        };
        let now = crate::world::now_rfc3339();
        let author = writer.unwrap_or("user").to_string();
        let version = versions.len() as u64 + 1;
        let charter = Charter {
            version,
            name,
            what,
            machines,
            places: change
                .places
                .or_else(|| current.map(|charter| charter.places.clone()))
                .unwrap_or_default(),
            canon,
            opened_by: current.map_or_else(|| author.clone(), |charter| charter.opened_by.clone()),
            opened_at: current.map_or_else(|| now.clone(), |charter| charter.opened_at.clone()),
            written_by: author,
            written_on: self.machine().to_string(),
            written_at: now,
        };
        let path = project::charter_path(version);
        let bytes = serde_json::to_vec_pretty(&charter)
            .map_err(|error| format!("Cannot encode the charter: {error}"))?;
        let update = ProjectUpdate {
            path: path.clone(),
            previous: version - 1,
        };
        self.world()
            .append_ledger(
                town.to_string(),
                vec![LedgerFile { path, bytes }],
                Some(update),
            )
            .await?;
        Ok(charter)
    }

    /// Adds an entry to a journal, starting the journal when it is new. Returns the entry's id.
    /// Attachments come from this machine or from others; they travel inside the entry while
    /// it fits the one message the hub takes it in, and the largest of the rest are copied
    /// into this machine's keeping, where the entry names them. Either way the evidence is
    /// fixed when the entry is written.
    pub(crate) async fn add_entry(
        &self,
        writer: Option<&str>,
        draft: EntryDraft,
    ) -> Result<String, String> {
        let (id, kept) = self.entry_id(&draft.town, &draft.journal)?;
        let written = self.write_entry(writer, draft, &id, &kept).await;
        if written.is_err() {
            // Nothing names what was kept for an entry that was not written.
            let _ = std::fs::remove_dir_all(&kept);
        }
        written
    }

    async fn write_entry(
        &self,
        writer: Option<&str>,
        draft: EntryDraft,
        id: &str,
        kept: &Path,
    ) -> Result<String, String> {
        let EntryDraft {
            town,
            journal: name,
            text,
            description,
            cites,
            attachments,
            remote_attachments,
        } = draft;
        let id = id.to_string();
        journal::validate_name(&name)?;
        project::charter(self.world_dir(), &town)?;
        let fresh = journal::descriptions(self.world_dir(), &town, &name).is_empty()
            && journal::entries(self.world_dir(), &town, &name).is_empty();
        match (fresh, &description) {
            (true, None) => {
                return Err(format!(
                    "{name} is a new journal; say what it covers with --description (its scope, not what it found)."
                ));
            }
            (false, Some(_)) => {
                return Err(format!(
                    "{name} has a description already; change it with fastctx journal describe."
                ));
            }
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
        let mut attached: Vec<Attached> = Vec::new();
        for path in attachments {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("{} has no file name.", path.display()))?
                .to_string();
            if attached_names(&attached).contains(&name) {
                return Err(format!(
                    "Two attachments are named {name}; attach files with different names."
                ));
            }
            attached.push(Attached::read(name, path, false)?);
        }
        for file in remote_attachments {
            let name = file
                .path
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| {
                    !name.is_empty() && *name != "." && *name != ".." && !name.contains(':')
                })
                .ok_or_else(|| {
                    format!(
                        "{} on {} does not end in a file name.",
                        file.path, file.machine
                    )
                })?
                .to_string();
            if attached_names(&attached).contains(&name) {
                return Err(format!(
                    "Two attachments are named {name}; attach files with different names."
                ));
            }
            let path = if file.machine == self.machine() {
                PathBuf::from(&file.path)
            } else {
                self.fetch(&file, &kept.join(&name)).await?
            };
            let fetched = file.machine != self.machine();
            attached.push(Attached::read(name, path, fetched)?);
        }
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
                        bytes: file.size,
                        machine: self.machine().to_string(),
                        path: file
                            .bytes
                            .is_none()
                            .then(|| crate::paths::display_path(&kept.join(&file.name))),
                    })
                    .collect(),
            };
            let base = format!("journals/{name}/entries/{id}");
            let mut files = vec![
                LedgerFile {
                    path: format!("{base}/text.md"),
                    bytes: format!("{}\n", text.trim_end()).into_bytes(),
                },
                LedgerFile {
                    path: format!("{base}/entry.json"),
                    bytes: serde_json::to_vec_pretty(&stamp)
                        .map_err(|error| format!("Cannot encode the entry's stamp: {error}"))?,
                },
            ];
            for file in &attached {
                if let Some(bytes) = &file.bytes {
                    files.push(LedgerFile {
                        path: format!("{base}/attachments/{}", file.name),
                        bytes: bytes.clone(),
                    });
                }
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
                    bytes: serde_json::to_vec_pretty(&first)
                        .map_err(|error| format!("Cannot encode the description: {error}"))?,
                });
            }
            let entry = shared::Entry {
                scope: town.clone(),
                id: String::new(),
                author: self.machine().to_string(),
                files: files.clone(),
                project: None,
            };
            if shared::fits(&entry)? {
                for file in attached
                    .iter()
                    .filter(|file| file.bytes.is_none() && !file.kept)
                {
                    keep(&file.origin, &kept.join(&file.name))?;
                }
                self.world()
                    .append_ledger(town.clone(), files, None)
                    .await?;
                // A file fetched from elsewhere that travels inside the entry needs no keeping.
                for file in attached
                    .iter()
                    .filter(|file| file.bytes.is_some() && file.kept)
                {
                    let _ = std::fs::remove_file(&file.origin);
                }
                let _ = std::fs::remove_dir(kept);
                return Ok(id);
            }
            match attached
                .iter_mut()
                .filter(|file| file.bytes.is_some())
                .max_by_key(|file| file.size)
            {
                Some(largest) => largest.bytes = None,
                None => {
                    return Err(format!(
                        "The entry's text is {} bytes, more than one journal entry carries; split it into several entries.",
                        text.len()
                    ));
                }
            }
        }
    }

    /// Fetches a file another machine holds into `to` on this machine, through `copy`.
    async fn fetch(&self, file: &RemoteFile, to: &Path) -> Result<PathBuf, String> {
        let args = serde_json::to_value(crate::world::copy::CopyArgs {
            from: file.path.clone(),
            to: crate::paths::display_path(to),
            to_node: self.machine().to_string(),
            caller: self.machine().to_string(),
            timeout_ms: Some(crate::world::surface::DEFAULT_COPY_TIMEOUT_MS),
        })
        .map_err(|error| format!("Cannot encode the copy of {}: {error}", file.path))?;
        let outcome = self
            .world()
            .call(
                "copy",
                &file.machine,
                args,
                CallBudget::default(),
                FETCH_TIMEOUT,
                CallOrigin::Tool,
            )
            .await
            .map_err(|error| {
                format!("Cannot fetch {} from {}: {error}", file.path, file.machine)
            })?;
        match &outcome.response {
            Some(response) if outcome.status == "ok" && !response.is_error && to.is_file() => {
                Ok(to.to_path_buf())
            }
            _ => Err(format!(
                "Cannot fetch {} from {}: {}",
                file.path,
                file.machine,
                super::remote::calls::outcome_error(&outcome)
            )),
        }
    }

    /// Writes the next version of a journal's description. Returns its version.
    pub(crate) async fn describe_journal(
        &self,
        writer: Option<&str>,
        town: &str,
        name: &str,
        text: &str,
    ) -> Result<u64, String> {
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
        let bytes = serde_json::to_vec_pretty(&description)
            .map_err(|error| format!("Cannot encode the description: {error}"))?;
        // Two writers of the same version write the same path, and the ledger takes only the
        // first: a description is never overwritten unseen.
        let path = format!("journals/{name}/description/v{version:06}.json");
        self.world()
            .append_ledger(town.to_string(), vec![LedgerFile { path, bytes }], None)
            .await?;
        Ok(version)
    }

    /// A new entry's id, and the directory this machine keeps its attachments in: the second
    /// it is written and this machine, or the next second free here. Making the directory
    /// takes the id, so entries written at the same time never share one; it is left once
    /// the entry is in the ledger, which then holds the id.
    fn entry_id(&self, town: &str, journal: &str) -> Result<(String, PathBuf), String> {
        let format =
            time::macros::format_description!("[year][month][day]T[hour][minute][second]Z");
        let entries = journal::journals_dir(self.world_dir(), town)
            .join(journal)
            .join("entries");
        let keeping = self
            .world_dir()
            .join("towns")
            .join(town)
            .join("attachments");
        std::fs::create_dir_all(&keeping).map_err(|error| {
            format!(
                "Cannot create {}: {error}",
                crate::paths::display_path(&keeping)
            )
        })?;
        let mut at = time::OffsetDateTime::now_utc();
        loop {
            let id = format!(
                "{}-{}",
                at.format(&format).expect("a UTC time formats"),
                self.machine()
            );
            let kept = keeping.join(&id);
            if !entries.join(&id).exists() {
                match std::fs::create_dir(&kept) {
                    Ok(()) => return Ok((id, kept)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(format!(
                            "Cannot create {}: {error}",
                            crate::paths::display_path(&kept)
                        ));
                    }
                }
            }
            at += time::Duration::seconds(1);
        }
    }
}

impl Attached {
    /// An attachment at `origin`; its bytes are read only when it may travel inside the entry.
    fn read(name: String, origin: PathBuf, kept: bool) -> Result<Self, String> {
        let size = std::fs::metadata(&origin)
            .map_err(|error| {
                format!(
                    "Cannot read {}: {error}",
                    crate::paths::display_path(&origin)
                )
            })?
            .len();
        let bytes = if size <= crate::world::envelope::MAX_CONTROL_MESSAGE_BYTES as u64 {
            Some(std::fs::read(&origin).map_err(|error| {
                format!(
                    "Cannot read {}: {error}",
                    crate::paths::display_path(&origin)
                )
            })?)
        } else {
            None
        };
        Ok(Self {
            name,
            size,
            origin,
            bytes,
            kept,
        })
    }
}

fn attached_names(attached: &[Attached]) -> Vec<String> {
    attached.iter().map(|file| file.name.clone()).collect()
}

/// Copies an attachment into the World's keeping on this machine, where the entry names it.
fn keep(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Cannot create {}: {error}",
                crate::paths::display_path(parent)
            )
        })?;
    }
    std::fs::copy(from, to).map(|_| ()).map_err(|error| {
        format!(
            "Cannot keep {} in {}: {error}",
            crate::paths::display_path(from),
            crate::paths::display_path(to)
        )
    })
}
