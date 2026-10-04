//! `fastctx apply` and `fastctx unapply`: the World's tools in the sessions
//! the user opens on this machine (`fastctx_install::apply`).

use anyhow::bail;
use clap::{Args, ValueEnum};
use fastctx_install::apply::{self, Hosts, Places, Report};
use fastctx_install::codex::Approval;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    Codex,
    Claude,
}

#[derive(Args)]
pub struct ApplyArgs {
    /// Only this host; once for each. Without it, every host installed here.
    #[arg(long, value_enum)]
    target: Vec<Target>,
    /// Raise Codex's tool_output_token_limit when it is set lower than the
    /// World's answers need. It is shared by every tool, so it is not raised
    /// without this.
    #[arg(long)]
    raise_output_limit: bool,
}

#[derive(Args)]
pub struct UnapplyArgs {
    /// Only this host; once for each. Without it, every host.
    #[arg(long, value_enum)]
    target: Vec<Target>,
}

pub fn apply(args: ApplyArgs) -> anyhow::Result<()> {
    let places = Places::here()?;
    let hosts = hosts(&places, &args.target, true)?;
    let approval = if args.raise_output_limit { Approval::RaiseOutputLimit } else { Approval::Require };
    let report = match apply::apply(&places, hosts, approval) {
        Ok(report) => report,
        Err(apply::Error::Edit(fastctx_install::Error::TokenLimitApproval { location, current, requested })) => bail!(
            "{location} is {current}, and the World's answers need {requested}; nothing was changed. It is the \
             limit for every tool in Codex: run fastctx apply --raise-output-limit to raise it"
        ),
        Err(error) => return Err(error.into()),
    };
    say(&report);
    if report.changed.is_empty() && report.commands.is_empty() {
        println!("Already in place.");
    }
    println!("Sessions opened from now on have the World's tools; sessions open now keep what they started with.");
    Ok(())
}

pub fn unapply(args: UnapplyArgs) -> anyhow::Result<()> {
    let places = Places::here()?;
    let hosts = hosts(&places, &args.target, false)?;
    let report = apply::unapply(&places, hosts)?;
    say(&report);
    if report.changed.is_empty()
        && report.commands.is_empty()
        && report.notices.is_empty()
        && report.leftovers.is_empty()
    {
        println!("Nothing of the World's was in place.");
    } else {
        println!("New sessions use the resulting host configuration; sessions open now keep what they started with.");
    }
    Ok(())
}

/// The hosts asked for, or, for an apply, those installed here; an unapply
/// takes out whatever its record names.
fn hosts(places: &Places, targets: &[Target], apply: bool) -> anyhow::Result<Hosts> {
    if targets.is_empty() {
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
        return Ok(present);
    }
    Ok(Hosts { codex: targets.contains(&Target::Codex), claude: targets.contains(&Target::Claude) })
}

fn say(report: &Report) {
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
}

/// A path as the user would write it, without Windows' verbatim prefix.
fn shown(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(text)
}
