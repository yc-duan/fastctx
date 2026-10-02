//! Journals, the fast layer (`specs/world-journals.md`): their files in a town's ledger and the
//! way `fastctx journal` reads them to a model.
//!
//! Under the ledger directory a journal is `journals/<name>/`:
//!   description/v<NNNNNN>.json   every version of its description
//!   entries/<id>/entry.json      the World's stamp on one entry
//!   entries/<id>/text.md         the entry's text, as its writer wrote it
//!   entries/<id>/attachments/    attachments copied into the entry
//! Nothing is ever changed once written; corrections are new entries.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What one `fastctx journal read` gives at most, the same budget as one `run`
/// (`specs/world-view.md`, "一次给多少").
pub(crate) const READ_BUDGET: usize = 150_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Description {
    pub(crate) version: u64,
    pub(crate) text: String,
    pub(crate) written_by: String,
    pub(crate) written_on: String,
    pub(crate) written_at: String,
}

/// The World's stamp on an entry: facts it knows, and the model as the host or the resident
/// reported it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Stamp {
    /// Writing time and machine, e.g. `20260701T031205Z-laptop`.
    pub(crate) id: String,
    pub(crate) at: String,
    pub(crate) machine: String,
    pub(crate) resident: String,
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) agent_version: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    /// `host` when the host reported the model, `self` when only the resident did.
    #[serde(default)]
    pub(crate) model_source: Option<String>,
    #[serde(default)]
    pub(crate) session: Option<String>,
    /// Earlier entries this one answers.
    #[serde(default)]
    pub(crate) cites: Vec<String>,
    #[serde(default)]
    pub(crate) attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Attachment {
    pub(crate) name: String,
    pub(crate) bytes: u64,
    /// Where its bytes are: inside the entry, or left on the machine it came from because it is
    /// too large to travel in the ledger.
    pub(crate) machine: String,
    #[serde(default)]
    pub(crate) path: Option<String>,
}

pub(crate) struct Entry {
    pub(crate) stamp: Stamp,
    pub(crate) text: String,
    pub(crate) dir: PathBuf,
}

pub(crate) struct JournalSummary {
    pub(crate) slug: String,
    pub(crate) description: String,
    pub(crate) entries: usize,
    pub(crate) writers: usize,
    pub(crate) last_by: Option<String>,
    pub(crate) last_at: Option<String>,
}

pub(crate) fn journals_dir(world_dir: &Path, town: &str) -> PathBuf {
    super::project::ledger_dir(world_dir, town).join("journals")
}

/// A journal name names a thing, in lowercase words joined by hyphens, and is its directory.
pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if valid { Ok(()) } else { Err(format!("A journal name is lowercase words joined by hyphens, such as webrtc-weak-network; got \"{name}\".")) }
}

pub(crate) fn descriptions(world_dir: &Path, town: &str, name: &str) -> Vec<Description> {
    let dir = journals_dir(world_dir, town).join(name).join("description");
    let mut files = json_files(&dir);
    files.sort();
    files.iter().filter_map(|path| serde_json::from_slice(&std::fs::read(path).ok()?).ok()).collect()
}

/// Every entry of a journal, in time order.
pub(crate) fn entries(world_dir: &Path, town: &str, name: &str) -> Vec<Entry> {
    let dir = journals_dir(world_dir, town).join(name).join("entries");
    let mut entries: Vec<Entry> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|item| {
            let dir = item.path();
            let stamp: Stamp = serde_json::from_slice(&std::fs::read(dir.join("entry.json")).ok()?).ok()?;
            let body = dir.join("text.md");
            let text = std::fs::read_to_string(&body).unwrap_or_else(|error| {
                format!("(the text of this entry cannot be read: {}: {error})", crate::paths::display_path(&body))
            });
            Some(Entry { stamp, text, dir })
        })
        .collect();
    entries.sort_by(|left, right| left.stamp.at.cmp(&right.stamp.at).then_with(|| left.stamp.id.cmp(&right.stamp.id)));
    entries
}

pub(crate) fn names(world_dir: &Path, town: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(journals_dir(world_dir, town))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|item| item.path().is_dir())
        .filter_map(|item| item.file_name().to_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

pub(crate) fn summaries(world_dir: &Path, town: &str) -> Result<Vec<JournalSummary>, String> {
    Ok(names(world_dir, town)
        .into_iter()
        .map(|slug| {
            let entries = entries(world_dir, town, &slug);
            let writers: BTreeSet<&str> = entries.iter().map(|entry| entry.stamp.resident.as_str()).collect();
            let last = entries.last();
            JournalSummary {
                description: descriptions(world_dir, town, &slug).pop().map(|description| description.text).unwrap_or_default(),
                entries: entries.len(),
                writers: writers.len(),
                last_by: last.map(|entry| writer(&entry.stamp)),
                last_at: last.map(|entry| entry.stamp.at.clone()),
                slug,
            }
        })
        .collect())
}

/// Reads a journal the way a model reads it: the head says which entries are shown, then the
/// description, then the entries in time order with the World's stamp and who mentioned them
/// later. When it does not all fit, the newest entries that fit are shown; `before` pages back.
pub(crate) fn render(world_dir: &Path, town: &str, name: &str, before: Option<&str>) -> Result<String, String> {
    validate_name(name)?;
    let all = entries(world_dir, town, name);
    let descriptions = descriptions(world_dir, town, name);
    if all.is_empty() && descriptions.is_empty() {
        return Err(format!("The town {town} has no journal named {name}."));
    }
    let end = match before {
        Some(id) => all.iter().position(|entry| entry.stamp.id == id).ok_or_else(|| format!("{name} has no entry {id}."))?,
        None => all.len(),
    };
    let mentioned = mentions(world_dir, town);
    let blocks: Vec<String> = all[..end].iter().map(|entry| entry_block(entry, mentioned.get(&entry.stamp.id))).collect();
    let mut description = String::new();
    if let Some(last) = descriptions.last() {
        let _ = write!(description, "About: {}", last.text);
        if last.version > 1 {
            let _ = write!(description, " (edited {} by {} on {})", &last.written_at[..last.written_at.len().min(10)], last.written_by, last.written_on);
        }
        description.push('\n');
    }
    let mut used = description.chars().count();
    let mut start = end;
    while start > 0 && used + blocks[start - 1].chars().count() <= READ_BUDGET {
        used += blocks[start - 1].chars().count();
        start -= 1;
    }
    let mut blocks = blocks;
    // An entry longer than the whole budget is still shown, as far as the budget goes, and
    // says so; earlier entries are a page back.
    if start == end && end > 0 {
        start = end - 1;
        let block = &blocks[start];
        let room = READ_BUDGET.saturating_sub(used);
        let total = block.chars().count();
        let mut cut: String = block.chars().take(room).collect();
        cut.push_str(&format!("\n(this entry is shown in part: characters 1-{room} of {total}; its files are in {})\n", crate::paths::display_path(&all[start].dir)));
        blocks[start] = cut;
    }
    let shown = &all[start..end];
    let residents: BTreeSet<&str> = all.iter().map(|entry| entry.stamp.resident.as_str()).collect();
    let hidden_attachments: usize = shown.iter().map(|entry| entry.stamp.attachments.len()).sum();
    let range = if shown.is_empty() { "no entries".to_string() } else { format!("entries {}-{} of {}", start + 1, end, all.len()) };
    let mut out = format!(
        "=== journal {name} · {town} ({range}; {} {}; {hidden_attachments} {} not shown) ===\n",
        residents.len(),
        if residents.len() == 1 { "resident" } else { "residents" },
        if hidden_attachments == 1 { "attachment" } else { "attachments" },
    );
    if start > 0 {
        let _ = writeln!(out, "(earlier entries: fastctx journal read {name} --before {})", all[start].stamp.id);
    }
    out.push_str(&description);
    for block in &blocks[start..end] {
        out.push('\n');
        out.push_str(block);
    }
    Ok(out)
}

fn entry_block(entry: &Entry, mentioned_by: Option<&Vec<String>>) -> String {
    let stamp = &entry.stamp;
    let model = match (&stamp.model, stamp.model_source.as_deref()) {
        (Some(model), Some("host")) => format!(" · model {model}"),
        (Some(model), _) => format!(" · model {model} (self-reported)"),
        (None, _) => String::new(),
    };
    let mut out = format!("── {} · {}{model} · {}\n", minute(&stamp.at), writer(stamp), stamp.id);
    out.push_str(entry.text.trim_end());
    out.push('\n');
    if !stamp.cites.is_empty() {
        let _ = writeln!(out, "Answers: {}", stamp.cites.join(", "));
    }
    if !stamp.attachments.is_empty() {
        let list: Vec<String> = stamp
            .attachments
            .iter()
            .map(|attachment| match &attachment.path {
                Some(path) => format!("{} ({}, on {} at {path})", attachment.name, size(attachment.bytes), attachment.machine),
                None => format!(
                    "{} ({}; {})",
                    attachment.name,
                    size(attachment.bytes),
                    crate::paths::display_path(&entry.dir.join("attachments").join(&attachment.name))
                ),
            })
            .collect();
        let _ = writeln!(out, "Attachments: {}", list.join(" · "));
    }
    if let Some(later) = mentioned_by {
        let _ = writeln!(out, "Mentioned by: {}", later.join(", "));
    }
    out
}

/// For every entry in the town, the later entries that cite it.
fn mentions(world_dir: &Path, town: &str) -> BTreeMap<String, Vec<String>> {
    let mut mentioned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in names(world_dir, town) {
        for entry in entries(world_dir, town, &name) {
            for cited in &entry.stamp.cites {
                mentioned.entry(cited.clone()).or_default().push(entry.stamp.id.clone());
            }
        }
    }
    mentioned
}

fn writer(stamp: &Stamp) -> String {
    let agent = match stamp.agent.as_str() {
        "claude-code" => "Claude Code",
        "codex" | "codex-mcp-client" => "Codex",
        "user" => "the user",
        other => other,
    };
    format!("{agent} on {}", stamp.machine)
}

fn minute(at: &str) -> String {
    at.get(..16).map(|text| text.replace('T', " ")).unwrap_or_else(|| at.to_string())
}

fn size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|item| item.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect()
}
