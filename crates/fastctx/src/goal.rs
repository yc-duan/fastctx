//! The main agent reads or marks its goal; the owning user opens and resumes it through the interface.

use clap::{Args, Subcommand};
use fastctx_types::goals::{AgentRequest, ColleagueControl, Mark, Updated};
use fastctx_types::run_facts::GoalMark;
use fastctx_types::{AccountName, Call, HubName, Reply, ResidentId};
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
    Pause {
        colleague: Option<ResidentId>,
    },
    Resume {
        colleague: ResidentId,
    },
}

pub fn run(args: GoalArgs) -> anyhow::Result<()> {
    // What this conversation marks of its own goal is a step of its run.
    let marked = match &args.command {
        Some(GoalCommand::Complete { .. }) => Some(GoalMark::Complete),
        Some(GoalCommand::Blocked { .. }) => Some(GoalMark::Blocked),
        Some(GoalCommand::Pause { colleague: None }) => Some(GoalMark::Pause),
        _ => None,
    };
    let request = match args.command {
        None => AgentRequest::Read,
        Some(GoalCommand::Pause { colleague: Some(colleague) }) => AgentRequest::ControlColleague {
            operation: fastctx_client::composer::watch_id()?,
            colleague,
            control: ColleagueControl::Pause,
        },
        Some(GoalCommand::Resume { colleague }) => AgentRequest::ControlColleague {
            operation: fastctx_client::composer::watch_id()?,
            colleague,
            control: ColleagueControl::Resume,
        },
        Some(GoalCommand::Complete { file }) => AgentRequest::Mark {
            operation: fastctx_client::composer::watch_id()?,
            mark: Mark::Complete { statement: draft(file)? },
        },
        Some(GoalCommand::Blocked { file }) => AgentRequest::Mark {
            operation: fastctx_client::composer::watch_id()?,
            mark: Mark::Blocked { statement: draft(file)? },
        },
        Some(GoalCommand::Pause { colleague: None }) => {
            AgentRequest::Mark { operation: fastctx_client::composer::watch_id()?, mark: Mark::Pause }
        }
    };
    let job = std::env::var("FASTCTX_JOB").ok().filter(|job| !job.is_empty()).map(|job| job.parse()).transpose()?;
    let began = marked.and_then(|_| crate::steps::began());
    match crate::ask::call(Call::GoalCommand { hub: args.hub, account: args.account, job, request })?.0 {
        Reply::Goal { updated } => {
            println!("{}", serde_json::to_string_pretty(&updated)?);
            if matches!(*updated, Updated::Conflict { .. }) {
                anyhow::bail!(fastctx_types::goals::words::CLI_CONFLICT);
            }
            if let (Some(began), Some(mark)) = (began, marked) {
                began.tell(fastctx_types::run_facts::StepDid::Goal { mark });
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
