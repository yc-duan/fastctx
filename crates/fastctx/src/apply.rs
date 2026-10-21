//! `fastctx apply` and `fastctx unapply`: the World's tools in the sessions
//! the user opens on this machine (`fastctx_install::apply`).

use anyhow::bail;
use clap::{Args, ValueEnum};
use fastctx_install::apply::{self, Hosts, Places, Report};
use fastctx_install::codex::Approval;

mod homes;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    Codex,
    Claude,
}

#[derive(Args)]
pub struct ApplyArgs {
    #[command(flatten)]
    homes: homes::Homes,
    /// Raise Codex's tool_output_token_limit when it is set lower than the
    /// World's answers need. It is shared by every tool, so it is not raised
    /// without this.
    #[arg(long)]
    raise_output_limit: bool,
    /// Rewrite what the World put in that was changed since as this fastctx
    /// writes it: Codex's mcp_servers.fastctx entry and native tool settings,
    /// Claude Code's limits and denied tools, and the fastctx block in
    /// AGENTS.md or CLAUDE.md; the rest of each file stays as it is.
    #[arg(long)]
    take_back: bool,
}

#[derive(Args)]
pub struct UnapplyArgs {
    #[command(flatten)]
    homes: homes::Homes,
}

pub fn apply(args: ApplyArgs) -> anyhow::Result<()> {
    let places = Places::here()?;
    let approval = if args.raise_output_limit { Approval::RaiseOutputLimit } else { Approval::Require };
    let edits = if args.take_back { apply::Edits::TakeBack } else { apply::Edits::Keep };
    let mut errors = Vec::new();
    let mut applied = Hosts { codex: false, claude: false };
    for (places, hosts) in args.homes.places(&places, true)? {
        applied.codex |= hosts.codex;
        applied.claude |= hosts.claude;
        let report = match apply::apply_reporting(&places, hosts, approval, edits, None) {
            Ok(report) => report,
            Err(apply::Error::Edit(fastctx_install::Error::TokenLimitApproval { location, current, requested })) => {
                errors.push(format!(
                    "{} is {current}, and the World's answers need {requested}. It is the \
                     limit for every tool in Codex: repeat this command with --raise-output-limit to raise it",
                    shown(std::path::Path::new(&location))
                ));
                continue;
            }
            Err(error) => {
                errors.push(error.to_string());
                continue;
            }
        };
        say(&report);
        if report.tools.iter().any(|check| check.result.is_err()) {
            errors.push("Configuration was committed and recorded, but native tool verification did not pass; see the checks above".into());
        }
        if report.changed.is_empty() && report.commands.is_empty() {
            println!("Already in place.");
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    // Codex reads its configuration when a session starts. Claude Code
    // rereads its permissions at once, but loads plugins only at the start.
    if applied.codex {
        println!(
            "Codex sessions opened from now on have the World's tools; those open now keep what they started with."
        );
    }
    if applied.claude {
        println!(
            "Claude Code sessions open now lose Bash, Grep and Glob at once and get the World's tools only when \
             opened again; sessions opened from now on have them from the start."
        );
    }
    Ok(())
}

pub fn unapply(args: UnapplyArgs) -> anyhow::Result<()> {
    let places = Places::here()?;
    let mut errors = Vec::new();
    let mut changed = false;
    for (places, hosts) in args.homes.places(&places, false)? {
        match apply::unapply(&places, hosts) {
            Ok(report) => {
                changed |= !report.changed.is_empty()
                    || !report.commands.is_empty()
                    || !report.notices.is_empty()
                    || !report.leftovers.is_empty();
                say(&report);
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    if !changed {
        println!("Nothing of the World's was in place.");
    } else {
        println!(
            "New sessions use the resulting host configuration; Claude Code sessions open now get Bash, Grep and \
             Glob back at once."
        );
    }
    Ok(())
}

/// The hosts asked for, or, for an apply, those installed here; an unapply
/// takes out whatever its record names.
fn hosts(places: &Places, apply: bool) -> anyhow::Result<Hosts> {
    if !apply {
        return Ok(Hosts { codex: true, claude: true });
    }
    let present = Hosts::present(places);
    if !present.codex {
        println!("Codex: {} does not exist; skipped.", places.codex_home.display());
    }
    if !present.claude {
        println!("Claude Code: the claude command is not on PATH; skipped.");
    }
    if !present.codex && !present.claude {
        bail!("neither Codex nor Claude Code is installed here");
    }
    Ok(present)
}

fn say(report: &Report) {
    for message in &report.messages {
        println!("{message}");
    }
    for manifest in &report.preserved {
        println!("preserved file versions: {}", shown(manifest));
    }
    for command in &report.commands {
        println!("ran {command}");
    }
    for path in &report.changed {
        println!("changed {}", shown(path));
    }
    for notice in &report.notices {
        println!("left {}: {}", notice.location, notice.reason);
    }
    for leftover in &report.leftovers {
        println!("could not clear: {}", leftover.replace(r"\\?\", ""));
    }
    for check in &report.tools {
        match &check.result {
            Ok(()) => println!("{}: verified {}", check.host, check.scope),
            Err(reason) => println!("{}: not verified ({}): {reason}", check.host, check.scope),
        }
    }
}

/// A path as the user would write it, without Windows' verbatim prefix.
fn shown(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(text)
}
