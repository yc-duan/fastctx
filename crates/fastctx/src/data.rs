//! `fastctx data`: what the World keeps on this machine, and deleting it,
//! for a machine without the graphical settings, such as a server reached
//! over SSH. It manages this machine's data only.

use std::io::{BufRead as _, IsTerminal as _, Write as _};

use fastctx_types::data::{ConversationSpace, DataCall, DataReply, DeletionOutcome, Group, HostRecord, Space};
use fastctx_types::{Call, Reply};
use jiff::Timestamp;

#[derive(clap::Args)]
pub struct DataArgs {
    #[command(subcommand)]
    command: Option<DataCommand>,
}

#[derive(clap::Subcommand)]
enum DataCommand {
    /// List the conversations this machine keeps, the largest first, and
    /// what everything here takes. The same as `fastctx data`.
    List {
        /// Order them by size, or by when they were last active.
        #[arg(long, value_enum, default_value_t = Order::Size)]
        sort: Order,
        /// Print what the node answered, as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Delete conversations from this machine, by the names `fastctx data`
    /// lists. One that lives here goes with everything it has here,
    /// including the record its agent keeps of it; for one that lives on
    /// another machine, only what it left here goes. One still working, or
    /// with a command still running here, is not deleted.
    Delete {
        #[arg(required = true)]
        names: Vec<String>,
        /// Delete without asking first.
        #[arg(long)]
        yes: bool,
    },
    /// Show how many days the full output of a command is kept after the
    /// model got its answer, or set it: 1, 3, 7, 14 or 30.
    Output { days: Option<u32> },
    /// Empty what this machine keeps as viewing cache.
    ClearCache,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Order {
    Size,
    Activity,
}

pub fn run(args: DataArgs) -> anyhow::Result<()> {
    match args.command.unwrap_or(DataCommand::List { sort: Order::Size, json: false }) {
        DataCommand::ClearCache => {
            let Reply::Data(DataReply::CacheCleared { bytes }) = call(DataCall::ClearCache)? else {
                anyhow::bail!("the node answered something other than what it cleared");
            };
            println!("cache cleared, {} freed", size(bytes));
            Ok(())
        }
        DataCommand::List { sort, json } => {
            let mut space = list()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&space)?);
                return Ok(());
            }
            if let Order::Activity = sort {
                space.conversations.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
            }
            print!("{}", render(&space, Timestamp::now()));
            Ok(())
        }
        DataCommand::Delete { names, yes } => delete(names, yes),
        DataCommand::Output { days } => {
            let Reply::Data(DataReply::OutputDays { days }) = call(DataCall::OutputDays { days })? else {
                anyhow::bail!("the node answered something other than the days");
            };
            println!("full command output is kept {days} days after the model got its answer");
            Ok(())
        }
    }
}

fn list() -> anyhow::Result<Space> {
    match call(DataCall::List)? {
        Reply::Data(DataReply::Listed(space)) => Ok(*space),
        _ => anyhow::bail!("the node answered something other than what it keeps"),
    }
}

fn call(call: DataCall) -> anyhow::Result<Reply> {
    Ok(crate::ask::call(Call::Data(call))?.0)
}

fn delete(names: Vec<String>, yes: bool) -> anyhow::Result<()> {
    if !yes {
        anyhow::ensure!(
            std::io::stdin().is_terminal(),
            "deleting cannot be undone; add --yes to delete without being asked"
        );
        let space = list()?;
        let mut asked = Vec::new();
        for name in &names {
            let Some(found) = space.conversations.iter().find(|one| one.name == *name) else {
                anyhow::bail!("nothing of {name} is kept on {}", space.machine);
            };
            asked.push(format!("  {}  {}", size(found.bytes), line(found, Timestamp::now())));
        }
        println!("These go from {} for good, with everything they keep here:", space.machine);
        println!("{}", asked.join("\n"));
        print!("Delete them? Type yes to delete: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if answer.trim() != "yes" {
            println!("nothing was deleted");
            return Ok(());
        }
    }
    let Reply::Data(DataReply::Deleted { deletions }) = call(DataCall::Delete { names })? else {
        anyhow::bail!("the node answered something other than what it deleted");
    };
    let mut failed = false;
    for deletion in deletions {
        match deletion.outcome {
            DeletionOutcome::Deleted { bytes } => println!("{}: deleted, {} freed", deletion.name, size(bytes)),
            DeletionOutcome::Refused { why } => {
                failed = true;
                println!("{}: not deleted: {why}", deletion.name);
            }
            DeletionOutcome::Failed { why } => {
                failed = true;
                println!("{}: deleting stopped: {why}", deletion.name);
            }
        }
    }
    anyhow::ensure!(!failed, "not everything named was deleted");
    Ok(())
}

fn render(space: &Space, now: Timestamp) -> String {
    let kinds = &space.kinds;
    let mut text = format!("{} keeps {} on {}, in {}\n", "fastctx", size(space.root_bytes), space.machine, space.root);
    let rows = [
        ("conversations", kinds.conversations),
        ("command output", kinds.command_output),
        ("cache", kinds.cache),
        ("program", kinds.program),
        ("other", kinds.other),
    ];
    for (name, bytes) in rows {
        text.push_str(&format!("  {name:<15} {:>10}\n", size(bytes)));
    }
    if kinds.host_records > 0 {
        text.push_str(&format!(
            "and {} the agents keep of these conversations, in their own directories\n",
            size(kinds.host_records)
        ));
    }
    text.push_str(&format!("full command output is kept {} days after the model got its answer\n", space.output_days));
    for login in &space.claude_logins {
        let whose =
            login.home.as_deref().map_or_else(|| "Claude Code".to_string(), |home| format!("Claude Code ({home})"));
        match (login.days, &login.unreadable) {
            (Some(days), _) => text.push_str(&format!(
                "{whose} deletes sessions untouched for {days} days{}\n",
                if login.set { "" } else { " (its default)" }
            )),
            (None, Some(why)) => text.push_str(&format!("{whose}: how long it keeps sessions is unknown: {why}\n")),
            (None, None) => {}
        }
    }
    for (group, title) in
        [(Group::Listed, "listed"), (Group::Removed, "removed"), (Group::Elsewhere, "from other machines")]
    {
        let ones: Vec<&ConversationSpace> = space.conversations.iter().filter(|one| one.group == group).collect();
        if ones.is_empty() {
            continue;
        }
        text.push_str(&format!("\n{title}\n"));
        for one in ones {
            text.push_str(&format!("  {:>10}  {}\n", size(one.bytes), line(one, now)));
            let host = match &one.parts.host {
                HostRecord::None => None,
                HostRecord::Kept { bytes, .. } => Some(format!("its agent's record {}", size(*bytes))),
                HostRecord::GoneByClaude { days: Some(days), .. } => {
                    Some(format!("Claude Code deleted its record, as it does after {days} days untouched"))
                }
                HostRecord::GoneByClaude { days: None, .. } => Some("Claude Code deleted its record".to_string()),
                HostRecord::Missing { .. } => Some("its agent's record is not where its agent keeps it".to_string()),
            };
            let mut parts = vec![
                format!("records {}", size(one.parts.records)),
                format!("command output {}", size(one.parts.command_output)),
            ];
            parts.extend(host);
            text.push_str(&format!("              {}\n", parts.join(" · ")));
            if let Some(held) = &one.held {
                text.push_str(&format!("              cannot be deleted now: {held}\n"));
            }
        }
    }
    text
}

fn line(one: &ConversationSpace, now: Timestamp) -> String {
    let mut words = vec![one.name.clone(), agent(one.agent.as_str()).to_string()];
    if let Some(title) = &one.title {
        words.push(format!("\"{title}\""));
    }
    if one.unopened {
        words.push("files attached, never sent".to_string());
    }
    if let Some(town) = &one.town {
        words.push(format!("project {town}"));
    }
    if one.group == Group::Elsewhere {
        words.push(match &one.lives_on {
            Some(machine) => format!("lives on {machine}"),
            None => "its machine is unknown".to_string(),
        });
    }
    if let Some(at) = one.last_activity {
        let ago = now.as_second().saturating_sub(at.as_second()).max(0) as u64;
        words.push(format!("active {} ago", fastctx_hands::told::duration(ago)));
    }
    words.join("  ")
}

fn agent(agent: &str) -> &str {
    match agent {
        "codex" => "Codex",
        "claude" => "Claude Code",
        other => other,
    }
}

fn size(bytes: u64) -> String {
    crate::copy::size(bytes)
}
