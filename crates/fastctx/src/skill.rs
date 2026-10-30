//! Read a skill and its complete folder on the conversation's working computer.
use clap::Args;
use fastctx_types::{Call, Reply};

#[derive(Args)]
pub(crate) struct SkillArgs {
    /// The skill's name, as the conversation's context lists it.
    #[arg(required_unless_present = "list")]
    name: Option<String>,
    /// All names and descriptions for this account.
    #[arg(long, conflicts_with = "name")]
    list: bool,
}

pub(crate) fn run(args: SkillArgs) -> anyhow::Result<()> {
    if args.list {
        let (reply, _) = crate::ask::call(Call::SkillFiles {
            hub: None,
            account: None,
            request: fastctx_types::skill_files::Request::List,
        })?;
        match reply {
            Reply::SkillFiles { result } => match *result {
                fastctx_types::skill_files::Reply::Listed { skills } => {
                    for skill in skills {
                        println!("{}: {}", skill.name, skill.description);
                    }
                    return Ok(());
                }
                _ => anyhow::bail!("the hub did not list skills"),
            },
            _ => anyhow::bail!("the node did not list skills"),
        }
    }
    let (reply, _) = crate::ask::call(Call::Skill { name: args.name.expect("required name"), folder: true })?;
    match reply {
        Reply::Skill { skill, location } => {
            if let Some(location) = location {
                println!(
                    "Skill folder on {}: {}\nRead its files there; scripts run from this folder.\n",
                    location.machine, location.path
                );
            }
            println!("{}", skill.instructions.trim_end());
            Ok(())
        }
        _ => anyhow::bail!("the node answered something other than a skill"),
    }
}
