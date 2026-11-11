//! A hidden manual entry to the node's durable timer table.

use clap::{Args, Subcommand};
use fastctx_types::timers::{Action, Owner, TimerCall};
use fastctx_types::{Call, NoticeId, Reply, ResidentId};

#[derive(Args)]
pub struct TimerArgs {
    #[command(subcommand)]
    command: TimerCommand,
}

#[derive(Subcommand)]
enum TimerCommand {
    Set {
        id: String,
        resident: ResidentId,
        at: jiff::Timestamp,
        text: String,
    },
    /// Replace a listed timer without repeating its potentially oversized identity.
    Replace {
        handle: NoticeId,
        resident: ResidentId,
        at: jiff::Timestamp,
        text: String,
    },
    Cancel {
        id: String,
    },
    CancelHandle {
        handle: NoticeId,
    },
    List {
        resident: Option<ResidentId>,
        #[arg(long)]
        after: Option<String>,
        /// Accept a bounded page; use its continuation even when the array is empty.
        #[arg(long)]
        page: bool,
    },
}

pub fn run(args: TimerArgs) -> anyhow::Result<()> {
    let timer = match args.command {
        TimerCommand::Set { id, resident, at: due, text } => TimerCall::Set {
            id,
            owner: Owner::Conversation { resident: resident.clone() },
            due,
            action: Action::Tell { resident, text },
            handle: None,
        },
        TimerCommand::Replace { handle, resident, at: due, text } => TimerCall::Set {
            id: String::new(),
            owner: Owner::Conversation { resident: resident.clone() },
            due,
            action: Action::Tell { resident, text },
            handle: Some(handle),
        },
        TimerCommand::Cancel { id } => TimerCall::Cancel { id, handle: None },
        TimerCommand::CancelHandle { handle } => TimerCall::Cancel { id: String::new(), handle: Some(handle) },
        TimerCommand::List { resident, after, page } => TimerCall::List {
            owner: resident.map(|resident| Owner::Conversation { resident }),
            paged: page || after.is_some(),
            after,
        },
    };
    let paged = matches!(&timer, TimerCall::List { paged: true, .. });
    match crate::ask::call(Call::Timer { timer })?.0 {
        // A page is not the whole list: where it continues and the rows too
        // large to show are part of what it prints, so a script reading its
        // output alone cannot take a page for all of them.
        Reply::Timers { timers, next, oversized } if paged => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "timers": timers,
                    "oversized": oversized,
                    "next": next,
                }))?
            );
        }
        Reply::Timers { timers, next, oversized } => {
            println!("{}", serde_json::to_string_pretty(&timers)?);
            for row in oversized {
                eprintln!(
                    "Oversized timer {} (identity {} bytes, row {} bytes): {}; cancel with fastctx timer cancel-handle {}",
                    row.handle,
                    row.identity_bytes,
                    row.body_bytes,
                    serde_json::to_string(&row.identity_preview)?,
                    row.handle
                );
            }
            if let Some(next) = next {
                eprintln!(
                    "More timers remain (an empty page is not the end); continue with --page --after {}",
                    serde_json::to_string(&next)?
                );
            }
        }
        Reply::Done => {}
        reply => anyhow::bail!("unexpected timer reply: {reply:?}"),
    }
    Ok(())
}
