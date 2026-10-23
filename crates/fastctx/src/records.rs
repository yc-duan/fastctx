//! A hidden JSON interface for inspecting account records.

use clap::{Args, Subcommand};
use fastctx_types::{
    AccountName, HubName,
    account_records::{Request, Response},
};

#[derive(Args)]
pub(crate) struct RecordsArgs {
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Read {
        kind: String,
        key: String,
    },
    List {
        kind: String,
    },
    Write {
        kind: String,
        key: String,
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long)]
        version: Option<u64>,
    },
    Delete {
        kind: String,
        key: String,
        #[arg(long)]
        version: u64,
    },
    Watch {
        kind: String,
    },
}

pub(crate) fn run(args: RecordsArgs) -> anyhow::Result<()> {
    let world = fastctx_client::World::here()?;
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async move {
        let request = match args.command {
            Command::Read { kind, key } => Request::Read { kind, key },
            Command::List { kind } => Request::List { kind },
            Command::Write { kind, key, file, version } => {
                let body = serde_json::from_slice(&std::fs::read(file)?)?;
                Request::Write { kind, key, expected: version, body }
            }
            Command::Delete { kind, key, version } => Request::Delete { kind, key, expected: Some(version) },
            Command::Watch { kind } => {
                let mut watch = world.watch_account_records(args.hub, args.account, kind).await?;
                loop {
                    println!("{}", serde_json::to_string(&watch.next().await?)?);
                }
            }
        };
        let response = world.account_records(args.hub, args.account, request).await?;
        println!("{}", serde_json::to_string_pretty(&response)?);
        if matches!(response, Response::Conflict { .. }) {
            anyhow::bail!(fastctx_types::account_records::words::CONFLICT);
        }
        Ok(())
    })
}
