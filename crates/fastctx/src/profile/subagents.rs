//! Commands change the same account layer and version as the graphical editor.

use clap::{Args, Subcommand};
use fastctx_types::{
    AccountName, Call, HubName, Reply,
    profile::{SubagentOperation, SubagentView},
};

#[derive(Args)]
pub(super) struct SubagentsArgs {
    /// Refuse a change unless the account still has this profile version.
    #[arg(long)]
    version: Option<u64>,
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    List,
    Show {
        name: String,
    },
    /// Save one JSON agent definition. Use --previous to replace or rename it.
    Save {
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long)]
        previous: Option<String>,
    },
    Delete {
        name: String,
    },
    /// Follow the defaults supplied by the installed Fastctx version again.
    Reset,
    /// Show the instructions, or replace them with --text or --file.
    Instructions {
        #[arg(long, conflicts_with = "file")]
        text: Option<String>,
        #[arg(long)]
        file: Option<std::path::PathBuf>,
    },
}

fn call(
    hub: Option<HubName>,
    account: Option<AccountName>,
    operation: SubagentOperation,
) -> anyhow::Result<SubagentView> {
    let (reply, _) = crate::ask::call(Call::ProfileSubagents { hub, account, operation })?;
    match reply {
        Reply::ProfileSubagents(view) => Ok(*view),
        _ => anyhow::bail!("the node did not answer the account's Claude subagents"),
    }
}

pub(super) fn run(hub: Option<HubName>, account: Option<AccountName>, args: SubagentsArgs) -> anyhow::Result<()> {
    let view = call(hub.clone(), account.clone(), SubagentOperation::List)?;
    if !view.supported {
        anyhow::bail!("upgrade hub {} to configure Claude subagents", view.hub);
    }
    let version = args.version.unwrap_or(view.version);
    let operation = match args.action {
        Action::List => {
            println!("{}", serde_json::to_string_pretty(&view)?);
            return Ok(());
        }
        Action::Show { name } => {
            let agent = view
                .configuration
                .agents
                .iter()
                .find(|v| v["name"].as_str() == Some(&name))
                .ok_or_else(|| anyhow::anyhow!("there is no subagent {name}"))?;
            println!("{}", serde_json::to_string_pretty(agent)?);
            return Ok(());
        }
        Action::Save { file, previous } => {
            SubagentOperation::Save { version, previous, agent: serde_json::from_slice(&std::fs::read(file)?)? }
        }
        Action::Delete { name } => SubagentOperation::Delete { version, name },
        Action::Reset => SubagentOperation::Reset { version },
        Action::Instructions { text, file } => {
            let text = match (text, file) {
                (Some(text), _) => text,
                (_, Some(file)) => std::fs::read_to_string(file)?,
                _ => {
                    print!("{}", view.configuration.instructions);
                    return Ok(());
                }
            };
            SubagentOperation::Instructions { version, text }
        }
    };
    let view = call(hub, account, operation)?;
    println!("profile version {}", view.version);
    Ok(())
}
