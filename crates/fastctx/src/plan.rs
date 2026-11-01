//! Shows the conversation's plan, or another conversation's; a plan changes
//! as its file does (`world-plan.md`).

use clap::Args;
use fastctx_types::{
    AccountName, Call, HubName, MachineName, Reply, ResidentId,
    plan::{Of, Operation, Result, words},
};

#[derive(Args)]
pub(crate) struct PlanArgs {
    #[arg(value_name = "ID", help = words::OF_HELP)]
    of: Option<String>,
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
}

pub(crate) fn run(args: PlanArgs) -> anyhow::Result<()> {
    let of = match &args.of {
        Some(named) => Some(match named.split_once('@') {
            Some((resident, machine)) => {
                Of { resident: ResidentId::try_from(resident)?, machine: Some(MachineName::try_from(machine)?) }
            }
            None => Of { resident: ResidentId::try_from(named.as_str())?, machine: None },
        }),
        None => None,
    };
    let call = Call::Plan { hub: args.hub, account: args.account, operation: Operation::Read, of: of.clone() };
    let (reply, _) = crate::ask::call(call)?;
    match reply {
        Reply::Plan { result: Result::Page { page } } => {
            match &of {
                // A machine of an earlier release answers with the caller's
                // own plan, whatever it was asked.
                Some(of) if page.owner != of.resident => anyhow::bail!(words::OLDER),
                Some(of) => println!("{}", words::version_of(&of.resident, page.version())),
                None => println!("{}", words::version(page.version())),
            }
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
