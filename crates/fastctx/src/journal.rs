//! `fastctx journal`: a town's journals (`world-journals.md`). The text of
//! an entry is written in a draft file first and handed over whole, so no
//! shell's quoting ever touches it.

use std::path::{Path, PathBuf};

use clap::{ArgAction, Subcommand};
use fastctx_hands::town::{journal, journal_line, size};
use fastctx_types::project::Kept;
use fastctx_types::{Attach, Call, EntryId, JournalCall, JournalName, Reply, TownName};

use crate::ask;

#[derive(Subcommand)]
pub enum JournalCommand {
    /// Add an entry to a journal; with --description, start the journal.
    Add {
        /// The journal: lowercase words joined by hyphens, naming a thing.
        journal: String,
        /// The draft file holding the entry's text.
        #[arg(long)]
        file: PathBuf,
        /// What the journal covers, to start it when it does not exist yet.
        #[arg(long)]
        description: Option<String>,
        /// An earlier entry this one answers, by id; once for each.
        #[arg(long, action = ArgAction::Append)]
        cite: Vec<String>,
        /// A file on this machine to attach as evidence; once for each.
        #[arg(long, action = ArgAction::Append)]
        attach: Vec<PathBuf>,
        /// A file on another machine to attach: the machine, then its
        /// absolute path; once for each.
        #[arg(long = "attach-from", num_args = 2, value_names = ["MACHINE", "PATH"], action = ArgAction::Append)]
        attach_from: Vec<String>,
        /// Your model, when fastctx does not know it; stamped as said by you.
        #[arg(long)]
        model: Option<String>,
        /// The project; without it, the one you work in.
        #[arg(long = "project", alias = "town", value_name = "NAME")]
        town: Option<String>,
    },
    /// Write the next version of what a journal covers.
    Describe {
        journal: String,
        description: String,
        /// The project; without it, the one you work in.
        #[arg(long = "project", alias = "town", value_name = "NAME")]
        town: Option<String>,
    },
    /// Read a journal: its latest entries that fit in one answer, oldest
    /// first, each stamped with who wrote it, where and when.
    Read {
        journal: String,
        /// Read the entries before this one.
        #[arg(long)]
        before: Option<String>,
        /// The project; without it, the one you work in.
        #[arg(long = "project", alias = "town", value_name = "NAME")]
        town: Option<String>,
        /// The entries as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// The journals of a project, the latest written first.
    List {
        /// The project; without it, the one you work in.
        #[arg(long = "project", alias = "town", value_name = "NAME")]
        town: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: JournalCommand) -> anyhow::Result<()> {
    let town = |given: Option<String>| -> anyhow::Result<Option<TownName>> {
        Ok(match given {
            Some(town) => Some(TownName::try_from(town)?),
            None => ask::town_from_run(),
        })
    };
    let mut json = false;
    let call = match command {
        JournalCommand::Add { journal, file, description, cite, attach, attach_from, model, town: given } => {
            let file = fastctx_platform::local_path(file)?;
            let text = std::fs::read(&file).map_err(|error| anyhow::anyhow!("reading {}: {error}", file.display()))?;
            let text = String::from_utf8(text)
                .map_err(|_| anyhow::anyhow!("{} is not UTF-8 text; write the draft in UTF-8", file.display()))?;
            let mut files: Vec<Attach> = attach
                .iter()
                .map(|path| Ok(Attach { machine: None, path: absolute(path)?.to_string_lossy().into_owned() }))
                .collect::<anyhow::Result<_>>()?;
            for pair in attach_from.chunks(2) {
                files.push(Attach {
                    machine: Some(fastctx_types::access::Written::parse(&pair[0])?),
                    path: pair[1].clone(),
                });
            }
            JournalCall::Add {
                town: town(given)?,
                journal: JournalName::try_from(journal)?,
                text,
                description,
                cites: cite.into_iter().map(EntryId::try_from).collect::<Result<_, _>>()?,
                attach: files,
                model,
            }
        }
        JournalCommand::Describe { journal, description, town: given } => {
            JournalCall::Describe { town: town(given)?, journal: JournalName::try_from(journal)?, text: description }
        }
        JournalCommand::Read { journal, before, town: given, json: as_json } => {
            json = as_json;
            JournalCall::Read {
                town: town(given)?,
                journal: JournalName::try_from(journal)?,
                before: before.map(EntryId::try_from).transpose()?,
            }
        }
        JournalCommand::List { town: given, json: as_json } => {
            json = as_json;
            JournalCall::List { town: town(given)? }
        }
    };
    let (reply, _) = ask::call(Call::Journal(call))?;
    let text = match reply {
        Reply::EntryAdded { town, journal, id, attachments } => {
            let mut text = format!("=== entry {id} added to {journal} · {town} ===\n");
            for attachment in &attachments {
                let at = match &attachment.kept {
                    Kept::InEntry => "in the entry".to_string(),
                    Kept::OnMachine { location } => format!("kept on {} at {}", location.machine, location.path),
                };
                text.push_str(&format!("{} ({}): {at}\n", attachment.name, size(attachment.bytes)));
            }
            text
        }
        Reply::Journal { view } if json => format!("{}\n", serde_json::to_string_pretty(&view)?),
        Reply::Journal { view } => {
            let text = journal(&view, fastctx_hands::town::JOURNAL_BYTES);
            // Inside a World `run`, reading a journal is reading a file.
            if std::env::var_os("FASTCTX_MACHINE").is_some() { fastctx_hands::town::marked(&text) } else { text }
        }
        Reply::Journals { town, journals } if json => {
            format!("{}\n", serde_json::to_string_pretty(&serde_json::json!({ "town": town, "journals": journals }))?)
        }
        Reply::Journals { town, journals } if journals.is_empty() => {
            format!("=== journals of {town}: none; `fastctx journal add --description` starts one ===\n")
        }
        Reply::Journals { town, journals } => {
            let mut text = format!("=== journals of {town} ({}, the latest written first) ===\n", journals.len());
            for journal in &journals {
                text.push_str(&format!("{}\n", journal_line(journal)));
            }
            text
        }
        other => anyhow::bail!("the node answered something else: {other:?}"),
    };
    crate::output::print(&text)?;
    Ok(())
}

/// A path as the node reads it: the node may stand elsewhere, so a relative
/// path is made whole from where this command runs.
fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(std::path::absolute(fastctx_platform::local_path(path)?)?)
}
