//! File-based page publication and reads.

use clap::{ArgAction, Subcommand};
use fastctx_types::pages::{PageAct, PageCall, PageDraft};
use fastctx_types::{Call, Reply, TownName};
use std::path::PathBuf;

use crate::ask;

/// How to write a page, for the resident who writes one: the elements, the
/// World's tools, what must be declared, and how it is published.
pub const PAGE_HELP: &str = include_str!("page_guide.txt");

#[derive(Subcommand)]
pub enum PageCommand {
    /// Publish a complete HTML page version.
    Publish {
        /// The page's name: lowercase words joined by hyphens.
        name: String,
        /// The HTML file to publish.
        #[arg(long)]
        file: PathBuf,
        /// The title shown above the page; without it, the page's name.
        #[arg(long)]
        title: Option<String>,
        /// One sentence saying what the page shows; without it, none.
        #[arg(long)]
        description: Option<String>,
        /// Run this exact command on this machine; repeat for each command.
        #[arg(long, num_args = 2, value_names = ["MACHINE", "COMMAND"], action = ArgAction::Append)]
        run: Vec<String>,
        /// Speak to this session on this machine; repeat for each session.
        #[arg(long, num_args = 2, value_names = ["MACHINE", "SESSION"], action = ArgAction::Append)]
        say: Vec<String>,
        /// Write to this journal; repeat for each journal. Hidden while
        /// project memory is not wired in (`world-journals.md`).
        #[arg(long, value_name = "JOURNAL", action = ArgAction::Append, hide = true)]
        write: Vec<String>,
        /// The version you started from, when changing a page.
        #[arg(long)]
        base: Option<u32>,
        /// The project; without it, the one you work in.
        #[arg(long = "project", alias = "town", value_name = "NAME")]
        town: Option<String>,
    },
    /// Read a page, optionally writing its exact HTML to a file.
    Read {
        /// The page's name, as `fastctx page list` shows it.
        name: String,
        /// Which version to read; without it, the current one.
        #[arg(long)]
        version: Option<u32>,
        /// Write the version's exact HTML to this file, for editing it and
        /// publishing again; without it, the page is printed.
        #[arg(long)]
        out: Option<PathBuf>,
        /// The project; without it, the one you work in.
        #[arg(long = "project", alias = "town", value_name = "NAME")]
        town: Option<String>,
    },
    /// List a project's pages.
    List {
        /// The project; without it, the one you work in.
        #[arg(long = "project", alias = "town", value_name = "NAME")]
        town: Option<String>,
    },
}

pub fn run(command: PageCommand) -> anyhow::Result<()> {
    let town = |given: Option<String>| -> anyhow::Result<Option<TownName>> {
        Ok(match given {
            Some(town) => Some(town.parse()?),
            None => ask::town_from_run(),
        })
    };
    let mut output = None;
    let mut reading = false;
    let call = match command {
        PageCommand::Publish { name, file, title, description, run, say, write, base, town: given } => {
            let file = fastctx_platform::local_path(file)?;
            let bytes = std::fs::read(&file).map_err(|e| anyhow::anyhow!("reading {}: {e}", file.display()))?;
            let html = String::from_utf8(bytes)
                .map_err(|_| anyhow::anyhow!("{} is not UTF-8 text; write the page in UTF-8", file.display()))?;
            let mut acts = Vec::new();
            for pair in run.chunks(2) {
                acts.push(PageAct::Run { machine: pair[0].parse()?, script: pair[1].clone() });
            }
            for pair in say.chunks(2) {
                acts.push(PageAct::Say {
                    to: fastctx_types::Address { machine: pair[0].parse()?, resident: pair[1].parse()? },
                });
            }
            for journal in write {
                acts.push(PageAct::Journal { journal: journal.parse()? });
            }
            let draft = PageDraft {
                title: title.unwrap_or_else(|| name.clone()),
                description: description.unwrap_or_default(),
                acts,
                html,
            };
            PageCall::Publish { town: town(given)?, page: name.parse()?, base, draft }
        }
        PageCommand::Read { name, version, out, town: given } => {
            reading = true;
            output = out.map(fastctx_platform::local_path).transpose()?;
            PageCall::Read { town: town(given)?, page: name.parse()?, version, account: None }
        }
        PageCommand::List { town: given } => PageCall::List { town: town(given)? },
    };
    let publishing = matches!(call, PageCall::Publish { .. });
    let began = publishing.then(crate::steps::began).flatten();
    let (reply, here) = ask::call(Call::Page(call))?;
    match reply {
        Reply::Page { view } => {
            let version = &view.publication;
            if let Some(began) = began {
                began.tell(fastctx_types::run_facts::StepDid::Page {
                    name: view.page.to_string(),
                    town: view.town.clone(),
                    title: fastctx_types::run_facts::clipped(&version.title),
                    version: u64::from(version.version),
                });
            }
            if let Some(path) = output {
                let html = view.html.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("page {} version {} is withdrawn and has no HTML", view.page, version.version)
                })?;
                std::fs::write(&path, html.as_bytes())?;
                println!(
                    "=== page {} · {} on {here}, version {}, HTML written to {} ===",
                    view.page,
                    view.town,
                    version.version,
                    path.display()
                );
            } else {
                println!(
                    "=== page {} · {} on {here}, version {}{} ===",
                    view.page,
                    view.town,
                    version.version,
                    if version.withdrawn { "; withdrawn" } else { "" }
                );
                if reading {
                    println!(
                        "{}\n{}\nActions: {}",
                        version.title,
                        version.description,
                        serde_json::to_string(&version.acts)?
                    );
                    if let Some(html) = &view.html {
                        print!("{html}");
                    }
                }
            }
        }
        Reply::Pages { town, pages } => {
            println!("=== pages of {town} on {here} ({}) ===", pages.len());
            for page in pages {
                println!("{}", fastctx_hands::pages::line(&page));
            }
        }
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
    Ok(())
}
