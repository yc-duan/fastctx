//! A project's charter (立项属性) and the map (`specs/world-projects.md`, `world-canon.md`,
//! `world-map.md`).
//!
//! A town's shared files live in its ledger directory, `<world>/towns/<town>/ledger/`, which
//! the World replicates to every machine of the town and never rewrites. Each version of the
//! charter is a new file `project/v<NNNNNN>.json`; the newest is the current one. Journals sit
//! beside it (`journal.rs`). The map is compiled from these files whenever it is asked for.

use super::journal::{self, JournalSummary};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr as _;

/// One version of a project's charter. The ledger reads `machines` from it to know which
/// machines the town spans.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Charter {
    /// 1 for the charter the project was opened with, then one more for every edit.
    pub(crate) version: u64,
    /// The project's name as its opener wrote it; the town's handle is the directory name.
    pub(crate) name: String,
    /// One or two sentences on what the project is for.
    pub(crate) what: String,
    pub(crate) machines: Vec<String>,
    /// The address book: where things are. Facts, not boundaries.
    #[serde(default)]
    pub(crate) places: Vec<Place>,
    /// Where the canon (the user-confirmed specs) lives, when the project has one.
    #[serde(default)]
    pub(crate) canon: Option<Location>,
    pub(crate) opened_by: String,
    pub(crate) opened_at: String,
    /// Who wrote this version, on which machine, and when; stamped by the World.
    pub(crate) written_by: String,
    pub(crate) written_on: String,
    pub(crate) written_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Place {
    pub(crate) what: String,
    #[serde(flatten)]
    pub(crate) location: Location,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Location {
    pub(crate) machine: String,
    pub(crate) path: String,
}

/// `<world>/towns/<town>/ledger`.
pub(crate) fn ledger_dir(world_dir: &Path, town: &str) -> PathBuf {
    world_dir.join("towns").join(town).join("ledger")
}

/// The ledger path of a charter version, relative to the ledger directory.
pub(crate) fn charter_path(version: u64) -> String {
    format!("project/v{version:06}.json")
}

/// Every town whose ledger this machine holds, by handle.
pub(crate) fn towns(world_dir: &Path) -> Vec<String> {
    let mut towns: Vec<String> = std::fs::read_dir(world_dir.join("towns"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join("ledger").join("project").is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    towns.sort();
    towns
}

/// Every version of a town's charter, oldest first.
pub(crate) fn charters(world_dir: &Path, town: &str) -> Result<Vec<Charter>, String> {
    let dir = ledger_dir(world_dir, town).join("project");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("This machine holds no town named {town}."),
            _ => format!("Cannot read {}: {error}", crate::paths::display_path(&dir)),
        })?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    names.sort();
    names
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path).map_err(|error| {
                format!("Cannot read {}: {error}", crate::paths::display_path(path))
            })?;
            serde_json::from_slice(&bytes).map_err(|error| {
                format!(
                    "{} is not a charter ({error})",
                    crate::paths::display_path(path)
                )
            })
        })
        .collect()
}

/// The current charter: the newest version.
pub(crate) fn charter(world_dir: &Path, town: &str) -> Result<Charter, String> {
    charters(world_dir, town)?
        .pop()
        .ok_or_else(|| format!("The town {town} has no charter yet."))
}

/// The charter as the top of the map and as `fastctx project show` prints it.
pub(crate) fn render_charter(town: &str, charter: &Charter) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Project: {} (town {town})", charter.name);
    let _ = writeln!(out, "What it is: {}", charter.what);
    let _ = writeln!(out, "Machines: {}", charter.machines.join(", "));
    let mut places: Vec<(&str, &Location)> = charter
        .places
        .iter()
        .map(|place| (place.what.as_str(), &place.location))
        .collect();
    if let Some(canon) = &charter.canon {
        places.push(("canon (specs)", canon));
    }
    if !places.is_empty() {
        out.push_str("Where to find things:\n");
        let what_width = places
            .iter()
            .map(|(what, _)| what.width())
            .max()
            .unwrap_or(0);
        let machine_width = places
            .iter()
            .map(|(_, location)| location.machine.len())
            .max()
            .unwrap_or(0);
        for (what, location) in places {
            let pad = what_width - what.width();
            let _ = writeln!(
                out,
                "  {what}{}   {:<machine_width$}   {}",
                " ".repeat(pad),
                location.machine,
                location.path
            );
        }
    }
    let _ = write!(
        out,
        "Opened by {} on {}",
        charter.opened_by,
        date(&charter.opened_at)
    );
    if charter.version > 1 {
        let _ = write!(
            out,
            "; version {}, written by {} on {} at {}",
            charter.version,
            charter.written_by,
            charter.written_on,
            date(&charter.written_at)
        );
    }
    out.push('\n');
    out
}

/// The map of a town: its charter, then one line per journal (most recently added first) and
/// one line per canon document. `machine` is where it is compiled; canon on another machine is
/// named with where it is, since it is read only where it lives.
pub(crate) fn render_map(world_dir: &Path, town: &str, machine: &str) -> Result<String, String> {
    let charter = charter(world_dir, town)?;
    let mut out = format!("=== map: {} (town {town}) ===\n", charter.name);
    out.push_str(&render_charter(town, &charter));
    let mut journals: Vec<JournalSummary> = journal::summaries(world_dir, town)?;
    journals.sort_by(|left, right| right.last_at.cmp(&left.last_at));
    if journals.is_empty() {
        out.push_str("Journals: none yet. Add one with fastctx journal.\n");
    } else {
        let _ = writeln!(
            out,
            "Journals ({}, most recently added first):",
            journals.len()
        );
        for summary in &journals {
            let last = match (&summary.last_by, &summary.last_at) {
                (Some(by), Some(at)) => format!("; last by {by} {}", date(at)),
                _ => String::new(),
            };
            let _ = writeln!(
                out,
                "  {} — {} ({} {}, {} {}{last})",
                summary.slug,
                summary.description,
                summary.entries,
                if summary.entries == 1 {
                    "entry"
                } else {
                    "entries"
                },
                summary.writers,
                if summary.writers == 1 {
                    "resident"
                } else {
                    "residents"
                }
            );
        }
    }
    match &charter.canon {
        None => {}
        Some(canon) if canon.machine == machine => {
            let documents = canon_lines(Path::new(&canon.path));
            let count = if documents.len() == 1 {
                "1 document".to_string()
            } else {
                format!("{} documents", documents.len())
            };
            let _ = writeln!(out, "Canon ({} {}, {count}):", canon.machine, canon.path);
            for (file, line) in documents {
                let _ = writeln!(out, "  {file} — {line}");
            }
        }
        Some(canon) => {
            let _ = writeln!(
                out,
                "Canon: on {} at {}; read it there with run.",
                canon.machine, canon.path
            );
        }
    }
    Ok(out)
}

/// Each canon document's title and the sentence or two after it, which say what it covers.
fn canon_lines(dir: &Path) -> Vec<(String, String)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            (name, describe(&text))
        })
        .collect()
}

/// The title and the first paragraph after it, on one line.
fn describe(text: &str) -> String {
    let mut lines = text.lines().map(str::trim);
    let title = lines
        .by_ref()
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .trim_start_matches('#')
        .trim()
        .to_string();
    let paragraph: Vec<&str> = lines
        .skip_while(|line| line.is_empty())
        .take_while(|line| !line.is_empty())
        .collect();
    if paragraph.is_empty() {
        title
    } else {
        format!("{title}: {}", paragraph.join(" "))
    }
}

fn date(at: &str) -> &str {
    at.get(..10).unwrap_or(at)
}
