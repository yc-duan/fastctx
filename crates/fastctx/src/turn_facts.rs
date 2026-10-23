//! Hidden manual access to the World's turn observations.

use clap::Args;
use fastctx_types::{Call, Reply, ResidentId};

#[derive(Args)]
pub struct TurnFactsArgs {
    resident: ResidentId,
    #[arg(long)]
    turn: Option<String>,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    quota: bool,
    #[arg(long)]
    compact: bool,
    #[arg(long)]
    stop: bool,
}

pub fn run(args: TurnFactsArgs) -> anyhow::Result<()> {
    let call = if args.stop {
        Call::Stop { resident: args.resident, machine: None }
    } else if args.compact {
        Call::CompactBetweenTurns { resident: args.resident }
    } else {
        Call::TurnEvidence { resident: args.resident, turn: args.turn, refresh_limits: args.quota }
    };
    match crate::ask::call(call)?.0 {
        Reply::TurnEvidence { evidence, text } => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&evidence)?);
            } else {
                print!("{text}");
            }
        }
        Reply::Compacted { result } => println!("{}", serde_json::to_string_pretty(&result)?),
        reply @ Reply::Stopped { .. } => println!("{}", serde_json::to_string_pretty(&reply)?),
        reply => anyhow::bail!("unexpected turn facts reply: {reply:?}"),
    }
    Ok(())
}
