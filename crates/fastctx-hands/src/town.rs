//! Project attributes and their generated memory index.

use fastctx_types::project::{MapView, Record, Stamp, Writer};
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;
/// The town's record as a resident reads it: the user's instructions for
/// all work in it, its machines, where things are, and who opened and last
/// changed it.
pub fn record(view: &MapView) -> String {
    record_of(&view.town, &view.record, &view.opened)
}

/// The record of `town` as a resident reads it, `opened` saying who opened
/// the town (`record`).
pub fn record_of(town: &TownName, record: &Record, opened: &Stamp) -> String {
    record_with(town, record, opened, None)
}

/// `record_of`, for a session working in the project: `description_at` says
/// where the project's description is kept for it to change, as `<path> on
/// <machine>`.
pub fn record_with(town: &TownName, record: &Record, opened: &Stamp, description_at: Option<&str>) -> String {
    let mut text = format!("=== project {town} ({}), version {} of its record ===\n", record.name, record.version);
    // What the user wrote for whoever works here, as they wrote it (须知).
    let instructions = record.what.trim();
    if !instructions.is_empty() {
        text.push_str(
            "The user's instructions for all work in this project, its CLAUDE.md and AGENTS.md; follow them, and \
             leave them to the user to change:\n",
        );
        text.push_str(instructions);
        text.push('\n');
    }
    // What the agents working here keep of the project as it stands.
    let description = record.description.trim();
    if !description.is_empty() {
        text.push_str("The project's description, as the agents working in it keep it:\n");
        text.push_str(description);
        text.push('\n');
    } else if description_at.is_some() {
        text.push_str("The project has no description yet.\n");
    }
    if let Some(at) = description_at {
        text.push_str(&format!(
            "You keep the description with the others working here: one coherent account of the whole project as it \
             stands now, what it is, how it is built and where its parts are, what is under way and the environment \
             it runs in. It is {at}; change it with edit or write whenever what it says no longer holds or something \
             the next one needs is missing, and take out what is no longer true. Every conversation here is given it \
             whole, so keep it to what matters, at most 32 KB, and say where secrets are kept rather than writing \
             them in it.\n"
        ));
    }
    text.push_str(&format!("Machines: {}\n", names(&record.machines)));
    if record.places.is_empty() {
        text.push_str("Where things are: nothing listed\n");
    } else {
        text.push_str("Where things are:\n");
        for place in &record.places {
            text.push_str(&format!("  {}: {} {}\n", place.what, place.location.machine, place.location.path));
        }
    }
    // A project holds several lines of work, each a conversation of its own
    // (`world-projects.md`). The colleague tools are not every session's, so
    // only the command that every session has is named.
    text.push_str(
        "Other conversations may work in this project too, each on its own line of work; `fastctx w` shows what each is doing. Where your work would change theirs, ask them first.\n",
    );
    text.push_str(&format!("Opened by {}", placed(opened, &record.machines, day)));
    if record.version > 1 {
        text.push_str(&format!(
            "; version {} by {}",
            record.version,
            placed(&record_stamp(record), &record.machines, day)
        ));
    }
    text.push_str(".\n");
    text
}

/// Who wrote a version and where, and when; the place is named only when
/// it is one of `machines`, those whoever reads it may work on: of a
/// machine they may not, they are not told even the name (`world-network.md`,
/// 共享机器与智能体协助).
fn placed(stamp: &Stamp, machines: &[MachineName], when: fn(Timestamp) -> String) -> String {
    if let Some(on) = stamp.on.as_ref().filter(|on| machines.contains(on)) {
        return format!("{} on {on}, {}", writer(&stamp.by, Some(on)), when(stamp.at));
    }
    let by = match &stamp.by {
        Writer::Resident { .. } => writer(&Writer::Elsewhere { page: false }, None),
        Writer::Page { .. } => writer(&Writer::Elsewhere { page: true }, None),
        by => writer(by, None),
    };
    format!("{by}, {}", when(stamp.at))
}

/// Attributes followed by the account's optional memory index.
pub fn map(view: &MapView) -> String {
    let mut text = record(view);
    if let Some(memory) = &view.memory {
        text.push('\n');
        text.push_str(memory);
    }
    text
}

pub fn brief(view: &MapView) -> String {
    map(view)
}

/// Who wrote a version of something, for a line that says where already
/// (`on`, when it does).
pub fn writer(by: &Writer, on: Option<&MachineName>) -> String {
    match by {
        Writer::Page { page } => page.to_string(),
        Writer::User => "the user".to_string(),
        Writer::Resident { address } if Some(&address.machine) == on => address.resident.to_string(),
        Writer::Resident { address } => address.tag(),
        Writer::Session { agent } => format!("a {} session fastctx does not know", program(agent, "", "")),
        Writer::Elsewhere { page: true } => "a page".to_string(),
        Writer::Elsewhere { page: false } => "a conversation".to_string(),
    }
}

/// A version of a record, in a line, its place named only when it is one
/// of `machines` (`placed`).
pub fn stamped(stamp: &Stamp, machines: &[MachineName]) -> String {
    placed(stamp, machines, minute)
}

pub fn record_stamp(record: &Record) -> Stamp {
    Stamp { by: record.by.clone(), on: record.on.clone(), at: record.at }
}

fn program(agent: &str, client: &str, version: &str) -> String {
    let name = match agent {
        "codex" => "Codex".to_string(),
        "claude" => "Claude Code".to_string(),
        _ if !client.is_empty() => client.to_string(),
        other => other.to_string(),
    };
    if version.is_empty() { name } else { format!("{name} {version}") }
}

fn names(machines: &[MachineName]) -> String {
    machines.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

/// A moment in UTC to the minute: entries are read on machines in any zone.
fn minute(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d %H:%MZ").to_string()
}

fn day(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d").to_string()
}
