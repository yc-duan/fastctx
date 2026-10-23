//! A hidden manual entry to the node's durable timer table.

use clap::{Args, Subcommand};
use fastctx_types::timers::{Action, Owner, TimerCall};
use fastctx_types::{Call, Reply, ResidentId};

#[derive(Args)]
pub struct TimerArgs {
    #[command(subcommand)]
    command: TimerCommand,
}

#[derive(Subcommand)]
enum TimerCommand {
    Set { id: String, resident: ResidentId, at: jiff::Timestamp, text: String },
    Cancel { id: String },
    List { resident: Option<ResidentId> },
}

pub fn run(args: TimerArgs) -> anyhow::Result<()> {
    let timer = match args.command {
        TimerCommand::Set { id, resident, at: due, text } => TimerCall::Set {
            id,
            owner: Owner::Conversation { resident: resident.clone() },
            due,
            action: Action::Tell { resident, text },
        },
        TimerCommand::Cancel { id } => TimerCall::Cancel { id },
        TimerCommand::List { resident } => {
            TimerCall::List { owner: resident.map(|resident| Owner::Conversation { resident }) }
        }
    };
    match crate::ask::call(Call::Timer { timer })?.0 {
        Reply::Timers { timers } => println!("{}", serde_json::to_string_pretty(&timers)?),
        Reply::Done => {}
        reply => anyhow::bail!("unexpected timer reply: {reply:?}"),
    }
    Ok(())
}
