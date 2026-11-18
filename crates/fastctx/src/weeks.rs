//! The agents' logins on the machines the user may use, with the week each
//! is reckoned of, and setting that week (`usage-assist-limits`): what an
//! assistance's share of a login comes to.

use clap::Args;
use fastctx_types::access::Written;
use fastctx_types::logins::{MachineLogins, MachineLoginsHeard};
use fastctx_types::usage::Usd;
use fastctx_types::{AgentName, Call, MachineName, Reply};

#[derive(Args)]
pub struct LoginsArgs {
    /// One machine; every machine you may use when not given.
    machine: Option<MachineName>,
}

#[derive(Args)]
pub struct WeekArgs {
    /// The machine the login is on.
    machine: MachineName,
    /// The agent: codex or claude.
    #[arg(long)]
    agent: AgentName,
    /// The login's directory on that machine, as `fastctx logins` lists it.
    #[arg(long)]
    home: std::path::PathBuf,
    /// The login's week in US dollars of quota, such as 2800: an
    /// assistance's share is a share of it.
    #[arg(value_parser = crate::world::dollars, required_unless_present = "default", conflicts_with = "default")]
    dollars: Option<Usd>,
    /// The World's default week again.
    #[arg(long)]
    default: bool,
    /// How far a week's own figure may stand from that, in percent either
    /// way, from 1 to 20: each week's figure is what the week before came
    /// to, held within this reach. Five where never set; as it is when not
    /// given.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=20))]
    swing: Option<u8>,
}

fn amount(usd: Usd) -> String {
    let cents = usd.0.div_ceil(10_000);
    if cents.is_multiple_of(100) { format!("${}", cents / 100) } else { format!("${}.{:02}", cents / 100, cents % 100) }
}

/// Lists the logins the World uses on one machine or on all.
pub fn logins(args: LoginsArgs) -> anyhow::Result<()> {
    let call = Call::AgentLogins { machine: args.machine.map(Written::from) };
    let machines: Vec<MachineLogins> = match crate::ask::call(call)?.0 {
        Reply::AgentLogins(world) => world
            .machines
            .into_iter()
            .filter_map(|heard| match heard {
                MachineLoginsHeard::Answered { logins, .. } => Some(*logins),
                MachineLoginsHeard::Offline { machine, .. } => {
                    println!("{machine}: offline");
                    None
                }
                MachineLoginsHeard::NoAnswer { machine, reason, .. } => {
                    println!("{machine}: no answer ({reason})");
                    None
                }
            })
            .collect(),
        Reply::MachineAgentLogins(one) => vec![*one],
        other => anyhow::bail!("the node answered something else: {other:?}"),
    };
    for machine in machines {
        for (agent, logins) in [("codex", &machine.codex), ("claude", &machine.claude)] {
            for login in &logins.listed {
                let default = if login.default { ", its default" } else { "" };
                let week = login.week.map_or_else(|| "the default week".to_string(), |week| format!("a week of {}", amount(week)));
                println!("{} {agent} {}{default}: {week}", machine.machine, login.dir.display());
            }
        }
    }
    Ok(())
}

/// Sets the week of one login, or the default again.
pub fn week(args: WeekArgs) -> anyhow::Result<()> {
    let week = if args.default { None } else { args.dollars };
    let call = Call::SetAgentWeek {
        machine: args.machine.clone().into(),
        agent: args.agent.clone(),
        dir: args.home.clone(),
        week,
        swing: args.swing,
    };
    crate::ask::call(call)?;
    match week {
        Some(week) => println!("the {} login {} on {} has a week of {}", args.agent, args.home.display(), args.machine, amount(week)),
        None => println!("the {} login {} on {} has the default week again", args.agent, args.home.display(), args.machine),
    }
    Ok(())
}
