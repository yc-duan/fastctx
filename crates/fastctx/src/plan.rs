//! The resident's whole-page plan command.

use clap::Args;
use fastctx_types::{
    AccountName, Call, HubName, Reply,
    plan::{Operation, Result, words},
};

#[derive(Args)]
pub(crate) struct PlanArgs {
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
    #[arg(long, value_parser = crate::paths::parser())]
    file: Option<std::path::PathBuf>,
}

pub(crate) fn run(args: PlanArgs) -> anyhow::Result<()> {
    let writing = args.file.is_some();
    let operation = match args.file {
        Some(path) => {
            Operation::Write { markdown: std::fs::read_to_string(crate::paths::absolute("fastctx plan", &path)?)? }
        }
        None => Operation::Read,
    };
    let (reply, _) = crate::ask::call(Call::Plan { hub: args.hub, account: args.account, operation })?;
    match reply {
        Reply::Plan { result: Result::Page { page } } => {
            println!("{}", words::version(page.version()));
            if !writing && let Some(body) = page.body {
                print!("\n{}", body.markdown);
            }
            Ok(())
        }
        Reply::Plan { result: Result::Conflict { current } } => {
            eprintln!("{}", serde_json::to_string(&current)?);
            anyhow::bail!(words::CONFLICT)
        }
        _ => anyhow::bail!(words::REPLY),
    }
}
