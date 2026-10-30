//! Reads one of the account's skills whole (`fastctx_types::skills`): the
//! conversation is told their names and when each applies, and reads one
//! before the work it applies to.

use clap::Args;
use fastctx_types::{Call, Reply};

#[derive(Args)]
pub(crate) struct SkillArgs {
    /// The skill's name, as the conversation's context lists it.
    name: String,
}

pub(crate) fn run(args: SkillArgs) -> anyhow::Result<()> {
    let (reply, _) = crate::ask::call(Call::Skill { name: args.name })?;
    match reply {
        Reply::Skill { skill } => {
            println!("{}", skill.instructions.trim_end());
            Ok(())
        }
        _ => anyhow::bail!("the node answered something other than a skill"),
    }
}
