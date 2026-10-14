//! Typed results for a still-open interface using the selected World kernel.

use std::path::PathBuf;

use clap::Args;
use fastctx_install::apply::{self, Edits, Host, Hosts, Places, kernel::Failure};
use fastctx_install::codex::Approval;

#[derive(Args)]
pub struct HostChangeArgs {
    #[arg(long)]
    codex: bool,
    #[arg(long)]
    claude: bool,
    #[arg(long, value_parser = crate::paths::parser())]
    codex_home: Option<PathBuf>,
    #[arg(long, value_parser = crate::paths::parser())]
    claude_home: Option<PathBuf>,
    #[arg(long)]
    into: bool,
    #[arg(long)]
    raise_output_limit: bool,
    /// Rewrite what the World put in and the user changed since.
    #[arg(long)]
    take_back: bool,
}

pub fn run(args: HostChangeArgs) -> anyhow::Result<()> {
    let mut places = Places::here()?;
    for (host, home) in [(Host::Codex, args.codex_home), (Host::Claude, args.claude_home)] {
        if let Some(home) = home {
            places = places.at_home(host, &home)?;
        }
    }
    let hosts = Hosts { codex: args.codex, claude: args.claude };
    let approval = if args.raise_output_limit { Approval::RaiseOutputLimit } else { Approval::Require };
    let edits = if args.take_back { Edits::TakeBack } else { Edits::Keep };
    let result = if args.into {
        apply::apply_reporting(&places, hosts, approval, edits, None)
    } else {
        apply::unapply(&places, hosts)
    };
    println!("{}", serde_json::to_string(&result.map_err(Failure::from))?);
    Ok(())
}
