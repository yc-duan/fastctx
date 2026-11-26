//! A conversation deletes one of its history documents for good, naming its
//! file as `write` and `edit` do; writing the file empty only takes it down.

use clap::{Args, Subcommand};
use fastctx_types::history::HistoryReply;
use fastctx_types::{Call, Reply};
use std::path::PathBuf;

#[derive(Args)]
pub struct HistoryArgs {
    #[command(subcommand)]
    command: HistoryCommand,
}

#[derive(Subcommand)]
enum HistoryCommand {
    /// Deletes the history document whose file is PATH for good: it leaves
    /// every list and none of its versions can be read again. Its name can
    /// be written again as a new document.
    Delete {
        #[arg(value_parser = crate::paths::parser())]
        path: PathBuf,
    },
}

pub fn run(args: HistoryArgs) -> anyhow::Result<()> {
    match args.command {
        HistoryCommand::Delete { path } => match crate::ask::call(Call::HistoryDelete { path })?.0 {
            Reply::History(HistoryReply::Saved { revision }) => {
                println!("deleted the history document {} for good", revision.name);
                Ok(())
            }
            _ => anyhow::bail!("the World did not confirm the deletion"),
        },
    }
}
