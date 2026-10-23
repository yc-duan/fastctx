//! The main agent reads or marks its goal; the owning user opens and resumes it through the interface.

use clap::{Args, Subcommand};
use fastctx_types::goals::{AgentRequest, Mark, Updated};
use fastctx_types::{AccountName, Call, HubName, Reply};
use std::path::PathBuf;

#[derive(Args)]
#[command(after_help = fastctx_types::goals::words::HELP)]
pub struct GoalArgs {
    #[arg(long)]
    hub: Option<HubName>,
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[command(subcommand)]
    command: Option<GoalCommand>,
}

#[derive(Subcommand)]
enum GoalCommand {
    Complete {
        #[arg(long, value_parser = crate::paths::parser())]
        file: PathBuf,
    },
    Blocked {
        #[arg(long, value_parser = crate::paths::parser())]
        file: PathBuf,
    },
    Pause,
}

pub fn run(args: GoalArgs) -> anyhow::Result<()> {
    let request = match args.command {
        None => AgentRequest::Read,
        Some(command) => {
            let mark = match command {
                GoalCommand::Complete { file } => Mark::Complete { statement: draft(file)? },
                GoalCommand::Blocked { file } => Mark::Blocked { statement: draft(file)? },
                GoalCommand::Pause => Mark::Pause,
            };
            AgentRequest::Mark { operation: fastctx_client::composer::watch_id()?, mark }
        }
    };
    let job = std::env::var("FASTCTX_JOB").ok().filter(|job| !job.is_empty()).map(|job| job.parse()).transpose()?;
    match crate::ask::call(Call::GoalCommand { hub: args.hub, account: args.account, job, request })?.0 {
        Reply::Goal { updated } => {
            println!("{}", serde_json::to_string_pretty(&updated)?);
            if matches!(*updated, Updated::Conflict { .. }) {
                anyhow::bail!(fastctx_types::goals::words::CLI_CONFLICT);
            }
        }
        reply => anyhow::bail!("unexpected goal reply: {reply:?}"),
    }
    Ok(())
}

fn draft(path: PathBuf) -> anyhow::Result<String> {
    let path = crate::paths::absolute("goal", &path)?;
    Ok(std::fs::read_to_string(path)?)
}
