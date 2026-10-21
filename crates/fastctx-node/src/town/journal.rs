//! `fastctx journal`: a town's journals (`world-journals.md`). An entry is
//! one entry of the town's ledger: its stamp, its text and the attachments
//! small enough to go with it; larger ones stay on the machine that wrote
//! it, named in the stamp.

use std::collections::HashMap;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_net::ledger::{self, Entry, File};
use fastctx_net::sign::Bytes;
use fastctx_types::copy::Place;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::project::{
    Attachment, AttachmentKind, Description, EntryStamp, EntryView, JournalView, Kept, Location, Model,
    description_path, entry_path,
};
use fastctx_types::{Attach, EntryId, JournalCall, JournalName, MachineName, Reply, TownName};
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

use crate::Node;
use crate::town::caller::Caller;
use crate::town::{map, read};

pub async fn call(node: &Arc<Node>, caller: &Caller, call: JournalCall) -> Result<Reply, Failure> {
    match call {
        JournalCall::Add { town, journal, text, description, cites, attach, model } => {
            let town = caller.town_or(node, town)?;
            let adding = Adding { journal, text, description, cites, attach, model };
            add(node, caller, town, adding).await
        }
        JournalCall::Describe { town, journal, text } => {
            let town = caller.town_or(node, town)?;
            describe(node, caller, town, journal, text).await
        }
        JournalCall::Read { town, journal, before } => {
            let town = caller.town_or(node, town)?;
            map::query(node, &town).await?;
            let (world, budget) = (node.world.clone(), fastctx_hands::town::JOURNAL_BYTES);
            let view = blocking(&town, move |town| read(&world, town, &journal, before.as_ref(), budget)).await?;
            Ok(Reply::Journal { view: Box::new(view) })
        }
        JournalCall::List { town } => {
            let town = caller.town_or(node, town)?;
            map::query(node, &town).await?;
            let world = node.world.clone();
            let journals = blocking(&town, move |town| read::summaries(&world, town)).await?;
            Ok(Reply::Journals { town, journals })
        }
    }
}

struct Adding {
    journal: JournalName,
    text: String,
    description: Option<String>,
    cites: Vec<EntryId>,
    attach: Vec<Attach>,
    model: Option<String>,
}

async fn add(node: &Arc<Node>, caller: &Caller, town: TownName, adding: Adding) -> Result<Reply, Failure> {
    let _writing = node.writing.lock().await;
    let here = node.machine();
    let journal = adding.journal;
    if adding.text.trim().is_empty() {
        return Err(Failure::new("the entry has no text"));
    }
    map::query(node, &town).await?;
    let world = node.world.clone();
    let (exists, missing) = {
        let (journal, cites) = (journal.clone(), adding.cites.clone());
        blocking(&town, move |town| {
            if read::latest(&world, town)?.is_none() {
                return Ok(None);
            }
            let exists = !read::descriptions(&world, town, &journal)?.is_empty()
                || !read::entries(&world, town, &journal)?.is_empty();
            let mut missing = Vec::new();
            for cite in &cites {
                if read::find_entry(&world, town, cite)?.is_none() {
                    missing.push(cite.to_string());
                }
            }
            Ok(Some((exists, missing)))
        })
        .await?
        .ok_or_else(|| map::no_ledger(node, &town))?
    };
    if !missing.is_empty() {
        return Err(Failure::new(format!("{town} has no entry {}", missing.join(", "))));
    }
    let description = match (exists, adding.description) {
        (true, Some(_)) => {
            return Err(Failure::new(format!(
                "{journal} exists already; change what it covers with fastctx journal describe {journal}"
            )));
        }
        (false, None) => {
            return Err(Failure::new(format!(
                "{town} has no journal {journal}; start it by saying what it covers with --description"
            )));
        }
        (false, Some(text)) if text.trim().is_empty() => {
            return Err(Failure::new("the description is empty"));
        }
        (_, description) => description,
    };

    let at = Timestamp::now();
    let id = free_id(node, &town, at, &here).await?;
    let (agent, model, session) = match caller {
        Caller::Resident(address) if node.is_me(&address.machine) => match node.residents.about(&address.resident) {
            Some((about, _)) => (Some(about.host.clone()), about.model.clone(), about.session.clone()),
            None => (None, None, None),
        },
        _ => (None, None, None),
    };
    let model = match (model, adding.model) {
        (Some(name), _) => Some(Model { name, self_reported: false }),
        (None, Some(name)) => Some(Model { name, self_reported: true }),
        (None, None) => None,
    };

    // Every attachment is copied in first, so the evidence is fixed as it is
    // now; those that fit go into the entry, the rest stay here.
    let staging = node.world.town(&town).attachments().join(id.as_str());
    let acting = crate::around::acting_of(node, caller);
    let staged = match stage(node, &acting, &staging, &adding.attach).await {
        Ok(staged) => staged,
        Err(failure) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(failure);
        }
    };
    let stamp = EntryStamp {
        id: id.clone(),
        at,
        machine: here.clone(),
        by: caller.writer(),
        agent,
        model,
        session,
        cites: adding.cites,
        attachments: Vec::new(),
    };
    let description =
        description.map(|text| Description { version: 1, text, by: caller.writer(), on: here.clone(), at });
    let packed = pack(node, &town, &journal, stamp, &adding.text, description.as_ref(), &staged);
    let (files, stamp) = match packed {
        Ok(packed) => packed,
        Err(failure) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(failure);
        }
    };
    if let Err(failure) = crate::ledger::write(node, &town, files, None).await {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(failure);
    }
    // What went into the entry is in the ledger now; what stays is kept
    // read-only, as the entry's files are.
    for (attachment, staged) in stamp.attachments.iter().zip(&staged) {
        let kept = matches!(attachment.kept, Kept::OnMachine { .. });
        let done = if kept { read_only(&staged.path) } else { std::fs::remove_file(&staged.path) };
        if let Err(error) = done {
            node.log.line(format_args!("settling the attachment {}: {error}", staged.path.display()));
        }
    }
    if stamp.attachments.iter().all(|attachment| attachment.kept == Kept::InEntry) {
        let _ = std::fs::remove_dir(&staging);
    }
    Ok(Reply::EntryAdded { town, journal, id, attachments: stamp.attachments })
}

/// A ledger entry's files: each path in the ledger, and its bytes.
type Files = Vec<(String, Vec<u8>)>;

/// An attachment copied in, with what the World found it to be.
struct Staged {
    name: String,
    path: PathBuf,
    bytes: u64,
    sha256: String,
    kind: AttachmentKind,
}

/// Copies each attachment into `staging`, from this machine or another.
async fn stage(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    staging: &Path,
    attach: &[Attach],
) -> Result<Vec<Staged>, Failure> {
    let _ = std::fs::remove_dir_all(staging);
    if !attach.is_empty() {
        fastctx_platform::ensure_private_dir(staging)
            .map_err(|error| Failure::new(format!("making {}: {error}", staging.display())))?;
    }
    let mut staged: Vec<Staged> = Vec::new();
    for one in attach {
        let name = file_name(&one.path)?;
        ledger::check_path(&name).map_err(Failure::new)?;
        if staged.iter().any(|other| other.name.eq_ignore_ascii_case(&name)) {
            return Err(Failure::new(format!("two attachments are named {name}")));
        }
        let path = staging.join(&name);
        match one.machine.as_ref().filter(|machine| !node.is_here(machine)) {
            None => {
                let from = fastctx_platform::local_path(&one.path).map_err(|error| Failure::new(error.to_string()))?;
                let to = path.clone();
                tokio::task::spawn_blocking(move || std::fs::copy(&from, &to))
                    .await
                    .map_err(|error| Failure::new(error.to_string()))?
                    .map_err(|error| Failure::new(format!("attaching {}: {error}", one.path)))?;
            }
            Some(machine) => {
                let from =
                    Place { machine: Some(machine.name.clone()), hub: machine.hub.clone(), path: one.path.clone() };
                let to = Place { machine: None, hub: None, path: crate::world::native(&path) };
                crate::copy::copy(node, acting, from, to)
                    .await
                    .map_err(|failure| Failure::new(format!("attaching {} from {machine}: {failure}", one.path)))?;
            }
        }
        let inspected = path.clone();
        let (bytes, sha256, kind) = tokio::task::spawn_blocking(move || inspect(&inspected))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("reading the attachment {name}: {error}")))?;
        staged.push(Staged { name, path, bytes, sha256, kind });
    }
    Ok(staged)
}

/// The ledger entry's files, and the stamp naming where each attachment
/// went: every one that fits goes with the entry, and while the entry is
/// too large for one message the largest of them stays on this machine.
fn pack(
    node: &Node,
    town: &TownName,
    journal: &JournalName,
    mut stamp: EntryStamp,
    text: &str,
    description: Option<&Description>,
    staged: &[Staged],
) -> Result<(Files, EntryStamp), Failure> {
    let here = node.machine();
    let directory = entry_path(journal, &stamp.id);
    // Larger than a message, an attachment cannot go with the entry.
    let mut inside: Vec<bool> =
        staged.iter().map(|staged| staged.bytes <= fastctx_net::wire::MAX_CONTROL_FRAME as u64).collect();
    loop {
        stamp.attachments = staged
            .iter()
            .zip(&inside)
            .map(|(staged, inside)| Attachment {
                name: staged.name.clone(),
                kind: staged.kind,
                bytes: staged.bytes,
                sha256: staged.sha256.clone(),
                kept: if *inside {
                    Kept::InEntry
                } else {
                    Kept::OnMachine {
                        location: Location { machine: here.clone(), path: crate::world::native(&staged.path) },
                    }
                },
            })
            .collect();
        let mut files = vec![
            (format!("{directory}/entry.json"), pretty(&stamp)?),
            (format!("{directory}/text.md"), text.as_bytes().to_vec()),
        ];
        if let Some(description) = description {
            files.push((description_path(journal, description.version), pretty(description)?));
        }
        for (staged, _) in staged.iter().zip(&inside).filter(|(_, inside)| **inside) {
            let bytes = std::fs::read(&staged.path)
                .map_err(|error| Failure::new(format!("reading the attachment {}: {error}", staged.name)))?;
            files.push((format!("{directory}/attachments/{}", staged.name), bytes));
        }
        // The entry as it will travel: its size does not depend on which
        // hub, project or key it names.
        let place = crate::ledger::names::place(&node.world, town)
            .map_err(|error| Failure::new(format!("reading {town}: {error}")))?
            .ok_or_else(|| Failure::new(format!("there is no project {town} on {here}")))?;
        let key = crate::ledger::own_key(&node.world)
            .map_err(|error| Failure::new(format!("this machine's key: {error}")))?;
        let entry = Entry {
            hub: place.hub,
            town: place.town,
            author: here.clone(),
            key: key.public(),
            at: stamp.at,
            machines: None,
            files: files.iter().map(|(path, bytes)| File { path: path.clone(), bytes: Bytes(bytes.clone()) }).collect(),
        };
        match crate::ledger::fits(&entry) {
            Ok(_) => return Ok((files, stamp)),
            Err(over) => {
                let largest = staged
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| inside[*index])
                    .max_by_key(|(_, staged)| staged.bytes)
                    .map(|(index, _)| index);
                match largest {
                    Some(index) => inside[index] = false,
                    None => {
                        return Err(Failure::new(format!(
                            "the entry is {over} bytes more than one ledger entry holds, without its attachments; shorten its text"
                        )));
                    }
                }
            }
        }
    }
}

/// The next version of a journal's description.
async fn describe(
    node: &Arc<Node>,
    caller: &Caller,
    town: TownName,
    journal: JournalName,
    text: String,
) -> Result<Reply, Failure> {
    let _writing = node.writing.lock().await;
    if text.trim().is_empty() {
        return Err(Failure::new("the description is empty"));
    }
    map::query(node, &town).await?;
    let world = node.world.clone();
    let (record, descriptions, entries) = {
        let journal = journal.clone();
        blocking(&town, move |town| {
            Ok((
                read::latest(&world, town)?,
                read::descriptions(&world, town, &journal)?,
                read::entries(&world, town, &journal)?.len(),
            ))
        })
        .await?
    };
    if record.is_none() {
        return Err(map::no_ledger(node, &town));
    }
    if descriptions.is_empty() && entries == 0 {
        return Err(Failure::new(format!(
            "{town} has no journal {journal}; start it with fastctx journal add {journal} --description"
        )));
    }
    let description = Description {
        version: descriptions.last().map_or(1, |latest| latest.version + 1),
        text,
        by: caller.writer(),
        on: node.machine(),
        at: Timestamp::now(),
    };
    let files = vec![(description_path(&journal, description.version), pretty(&description)?)];
    crate::ledger::write(node, &town, files, None).await.map_err(|failure| {
        if failure.message.contains("already") {
            Failure::new(format!(
                "another version {} of {journal}'s description was written first; read it with fastctx journal read {journal} and describe it again",
                description.version
            ))
        } else {
            failure
        }
    })?;
    let summary = blocking(&town, {
        let world = node.world.clone();
        move |town| read::summary(&world, town, &journal)
    })
    .await?;
    Ok(Reply::Journals { town, journals: vec![summary] })
}

/// The latest entries of a journal that fit in `budget` bytes as a resident
/// reads them, before `before` when given.
fn read(
    world: &WorldDir,
    town: &TownName,
    journal: &JournalName,
    before: Option<&EntryId>,
    budget: usize,
) -> io::Result<JournalView> {
    let all = read::entries(world, town, journal)?;
    let description = read::descriptions(world, town, journal)?.pop();
    if all.is_empty() && description.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{town} has no journal {journal} (fastctx journal list lists them)"),
        ));
    }
    let end = match before {
        Some(before) => all
            .iter()
            .position(|(stamp, _)| stamp.id == *before)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{journal} has no entry {before}")))?,
        None => all.len(),
    };
    // Which later entries, in any journal of the town, cite each entry.
    let mut mentions: HashMap<EntryId, Vec<EntryId>> = HashMap::new();
    for other in read::journal_names(world, town)? {
        for (stamp, _) in read::entries(world, town, &other)? {
            for cited in &stamp.cites {
                mentions.entry(cited.clone()).or_default().push(stamp.id.clone());
            }
        }
    }
    let mut view = JournalView {
        town: town.clone(),
        journal: journal.clone(),
        directory: crate::world::native(&read::journal_dir(world, town, journal)),
        description,
        first: end + 1,
        entries: Vec::new(),
        total: all.len(),
        writers: read::writers(all.iter().map(|(stamp, _)| &stamp.by)),
    };
    // From the latest back, while the answer stays within the budget; the
    // latest is given even when it alone is larger.
    for (stamp, directory) in all[..end].iter().rev() {
        let entry = EntryView {
            stamp: stamp.clone(),
            text: read::text(directory)?,
            mentioned_by: mentions.get(&stamp.id).cloned().unwrap_or_default(),
        };
        let mut tried = view.clone();
        tried.entries.insert(0, entry);
        tried.first -= 1;
        if !view.entries.is_empty() && fastctx_hands::town::journal(&tried, usize::MAX).len() > budget {
            break;
        }
        view = tried;
    }
    Ok(view)
}

/// A fresh entry id for now: the second and this machine. Entries written
/// here are written one at a time, so an id is taken only by an entry
/// written here within the same second; the next second is then free.
async fn free_id(node: &Arc<Node>, town: &TownName, mut at: Timestamp, here: &MachineName) -> Result<EntryId, Failure> {
    loop {
        let id = EntryId::try_from(format!("{}-{here}", at.strftime("%Y%m%dT%H%M%SZ")))
            .map_err(|error| Failure::new(error.to_string()))?;
        let world = node.world.clone();
        let taken = {
            let id = id.clone();
            blocking(town, move |town| read::find_entry(&world, town, &id)).await?
        };
        if taken.is_none() {
            return Ok(id);
        }
        let next = Timestamp::from_second(at.as_second() + 1).map_err(|error| Failure::new(error.to_string()))?;
        let wait = next.duration_since(Timestamp::now());
        if wait.is_positive() {
            tokio::time::sleep(wait.unsigned_abs()).await;
        }
        at = Timestamp::now().max(next);
    }
}

/// Size, SHA-256 and kind of a file, read once from start to end.
fn inspect(path: &Path) -> io::Result<(u64, String, AttachmentKind)> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    let mut head = Vec::new();
    let mut utf8 = true;
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        hash.update(chunk);
        size += read as u64;
        if head.len() < 16 {
            head.extend_from_slice(&chunk[..chunk.len().min(16 - head.len())]);
        }
        if utf8 {
            pending.extend_from_slice(chunk);
            match std::str::from_utf8(&pending) {
                Ok(_) => pending.clear(),
                // A character cut by the chunk's end waits for the next.
                Err(error) if error.error_len().is_none() => {
                    pending.drain(..error.valid_up_to());
                }
                Err(_) => utf8 = false,
            }
        }
    }
    let kind = if head.starts_with(b"%PDF-") {
        AttachmentKind::Pdf
    } else if image(&head) {
        AttachmentKind::Image
    } else if utf8 && pending.is_empty() {
        AttachmentKind::Text
    } else {
        AttachmentKind::Binary
    };
    let digest: String = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    Ok((size, digest, kind))
}

fn image(head: &[u8]) -> bool {
    head.starts_with(b"\x89PNG\r\n\x1a\n")
        || head.starts_with(&[0xff, 0xd8, 0xff])
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
        || (head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP")
        || head.starts_with(b"BM")
}

/// The last name of a path, on any system.
fn file_name(path: &str) -> Result<String, Failure> {
    path.rsplit(['/', '\\'])
        .find(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Failure::new(format!("{path:?} names no file")))
}

fn read_only(path: &Path) -> io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)
}

fn pretty(value: &impl serde::Serialize) -> Result<Vec<u8>, Failure> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| Failure::new(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

async fn blocking<T: Send + 'static>(
    town: &TownName,
    work: impl FnOnce(&TownName) -> io::Result<T> + Send + 'static,
) -> Result<T, Failure> {
    let name = town.clone();
    tokio::task::spawn_blocking(move || work(&name))
        .await
        .map_err(|error| Failure::new(format!("reading {town}'s ledger: {error}")))?
        .map_err(|error| Failure::new(error.to_string()))
}
