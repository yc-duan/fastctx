//! Read and edit an account's prompt and supported host settings.

use clap::{Args, Subcommand};

mod subagents;

#[derive(Subcommand)]
enum ProfileCommand {
    Subagents(subagents::SubagentsArgs),
}
use fastctx_types::{AccountName, Call, HubName, Reply};

#[derive(Args)]
pub(crate) struct ProfileArgs {
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
    /// Replace the user's instructions; an empty string clears them.
    #[arg(long, conflicts_with = "prompt_file")]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<std::path::PathBuf>,
    /// Replace the allowed Claude Code settings with this JSON object.
    #[arg(long)]
    claude: Option<String>,
    /// Replace the allowed Codex settings with this JSON object.
    #[arg(long)]
    codex: Option<String>,
    #[command(subcommand)]
    command: Option<ProfileCommand>,
}
pub(crate) fn run(args: ProfileArgs) -> anyhow::Result<()> {
    if let Some(ProfileCommand::Subagents(command)) = args.command {
        if args.prompt.is_some() || args.prompt_file.is_some() || args.claude.is_some() || args.codex.is_some() {
            anyhow::bail!("edit subagents separately from the prompt and host settings");
        }
        return subagents::run(args.hub, args.account, command);
    }
    let (reply, _) = crate::ask::call(Call::ProfileRead { hub: args.hub.clone(), account: args.account.clone() })?;
    let Reply::Profile(mut view) = reply else {
        anyhow::bail!("the node did not answer the profile");
    };
    if !view.kept {
        anyhow::bail!("upgrade hub {} to use user-level configuration", view.hub);
    }
    let writing = args.prompt.is_some() || args.prompt_file.is_some() || args.claude.is_some() || args.codex.is_some();
    if let Some(prompt) = args.prompt {
        view.profile.prompt = prompt;
    }
    if let Some(path) = args.prompt_file {
        view.profile.prompt = std::fs::read_to_string(path)?;
    }
    if let Some(settings) = args.claude {
        view.profile.claude = serde_json::from_str(&settings)?;
    }
    if let Some(settings) = args.codex {
        view.profile.codex = serde_json::from_str(&settings)?;
    }
    if writing {
        view.profile.validate().map_err(anyhow::Error::msg)?;
        let (reply, _) = crate::ask::call(Call::ProfileWrite {
            hub: Some(view.hub),
            account: Some(view.account),
            profile: view.profile,
        })?;
        let Reply::ProfileWritten { version } = reply else {
            anyhow::bail!("the node did not confirm the profile write");
        };
        println!("profile version {version}");
    } else {
        println!("{}", serde_json::to_string_pretty(&view)?);
    }
    Ok(())
}
