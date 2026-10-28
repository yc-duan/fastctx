//! Shows the conversation's plan; it changes as its file does (`world-plan.md`).

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
}

pub(crate) fn run(args: PlanArgs) -> anyhow::Result<()> {
    let (reply, _) = crate::ask::call(Call::Plan { hub: args.hub, account: args.account, operation: Operation::Read })?;
    match reply {
        Reply::Plan { result: Result::Page { page } } => {
            println!("{}", words::version(page.version()));
            if let Some(body) = page.body {
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
