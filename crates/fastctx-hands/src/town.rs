//! A town as residents read it: its map (`world-map.md`), a journal
//! (`world-journals.md`, 读), and what the World tells every resident about
//! keeping journals.

use fastctx_types::project::{
    Attachment, AttachmentKind, Description, EntryStamp, EntryView, JournalSummary, JournalView, Kept, MapView, Record,
    Stamp, Writer,
};
use fastctx_types::{EntryId, MachineName, TownName};
use jiff::Timestamp;

/// How journals are kept, as every resident is told it: with the town it
/// lives in, and in `fastctx journal --help`.
pub const JOURNALS: &str = "\
This project keeps journals. They are history, not verdicts. Write only
sentences that will still be true a year from now, whatever is found later.

Add an entry when something happened that whoever works here next needs and
cannot recover from the specs, the code, or git:
  - you tried something and it did not go as you meant, or you turned to
    another path;
  - something you were trying to find out came out one way or another;
  - you are stopping, handing off, or stuck: where the work is, what fails,
    and where it can be picked up.
Routine actions are not history. A few entries across one or several
context compactions is typical.

Record what you did, what you meant to find out, and what you observed,
with the conditions (version, machine, input, settings) and the evidence
attached. \"On WebRTC 1.2 over a 3G profile, packet loss measured 35% (log
attached)\" stays true. \"WebRTC is not viable here\" may not; do not write
it. Do not write verdicts such as \"the cause is\", \"does not work\", \"done\",
or \"the right approach\". Questions you could not answer are welcome.
Do not write relative time such as \"now\", \"currently\", \"recently\", or
\"yesterday\"; each entry is stamped with its time, so write dates and
versions. Quote the user's words verbatim. Cite code as path@commit or
attach the lines; the next reader may be on another machine.

Before starting a journal, look for one on the same subject and add to it,
even if what you saw contradicts it. Name a new journal after a thing,
never after a conclusion; keep its description to what it covers, not what
it found. Earlier entries record what someone saw under their conditions.
Do not defer to them because they are older or because another model wrote
them: test them, and add what you see. When your entry answers an earlier
one, cite that entry's id. Never change an entry, yours included; add a new
one. Journal files are read-only copies; write only through `fastctx journal`.";

/// The town's record as a resident reads it: the user's instructions for
/// all work in it, its machines, where things are, and who opened and last
/// changed it.
pub fn record(view: &MapView) -> String {
    record_of(&view.town, &view.record, &view.opened)
}

/// The record of `town` as a resident reads it, `opened` saying who opened
/// the town (`record`).
pub fn record_of(town: &TownName, record: &Record, opened: &Stamp) -> String {
    let mut text = format!("=== project {town} ({}), version {} of its record ===\n", record.name, record.version);
    // What the user wrote for whoever works here, as they wrote it (须知).
    let instructions = record.what.trim();
    if !instructions.is_empty() {
        text.push_str("The user's instructions for all work in this project; follow them:\n");
        text.push_str(instructions);
        text.push('\n');
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
    text.push_str(&format!("Opened by {} on {}, {}", writer(&opened.by, &opened.on), opened.on, day(opened.at)));
    if record.version > 1 {
        text.push_str(&format!(
            "; version {} by {} on {}, {}",
            record.version,
            writer(&record.by, &record.on),
            record.on,
            day(record.at)
        ));
    }
    text.push_str(".\n");
    text
}

/// The town's map: its record, then each journal and each document of its
/// canon in a line.
pub fn map(view: &MapView) -> String {
    let record = &view.record;
    let mut text = self::record(view);
    text.push('\n');
    if view.journals.is_empty() {
        text.push_str(&format!(
            "Journals: none yet. They are files in {}/journals; start one with `fastctx journal add`.\n",
            view.ledger
        ));
    } else {
        text.push_str(&format!(
            "Journals ({}, the latest written first; files in {}/journals; read one with `fastctx journal read JOURNAL`):\n",
            view.journals.len(),
            view.ledger
        ));
        for journal in &view.journals {
            text.push_str(&format!("  {}\n", journal_line(journal)));
        }
    }

    text.push_str(&format!("\nPages ({}; read one with `fastctx page read PAGE`):\n", view.pages.len()));
    for page in &view.pages {
        text.push_str(&format!("  {}\n", crate::pages::line(page)));
    }

    match (&record.canon, &view.canon) {
        (None, _) => text.push_str("\nDecisions settled with the user: none listed.\n"),
        (Some(canon), Some(Ok(heads))) if heads.is_empty() => text.push_str(&format!(
            "\nDecisions settled with the user: {} {}, which holds no documents.\n",
            canon.machine, canon.path
        )),
        (Some(canon), Some(Ok(heads))) => {
            text.push_str(&format!(
                "\nDecisions settled with the user, one document a line ({} {}):\n",
                canon.machine, canon.path
            ));
            for head in heads {
                let scope = if head.scope.is_empty() { String::new() } else { format!(" {}", head.scope) };
                text.push_str(&format!("  {}: {}.{scope}\n", head.file, head.title.trim_end_matches('.')));
            }
        }
        (Some(canon), Some(Err(why))) => text.push_str(&format!(
            "\nDecisions settled with the user: {} {}, not readable now: {why}\n",
            canon.machine, canon.path
        )),
        (Some(canon), None) => {
            text.push_str(&format!("\nDecisions settled with the user: {} {}\n", canon.machine, canon.path))
        }
    }
    text
}

/// What a resident living in a town is given of it. Journals, decisions
/// and the map are project memory, which is not wired in until it is
/// finished (`world-journals.md`): a resident is given the record alone.
pub fn brief(view: &MapView) -> String {
    record(view)
}

/// One journal in a line: its name, what it covers, and who last wrote.
pub fn journal_line(journal: &JournalSummary) -> String {
    let about = match &journal.description {
        Some(description) => description.text.clone(),
        None => "(no description)".to_string(),
    };
    let entries = match journal.entries {
        1 => "1 entry".to_string(),
        count => format!("{count} entries"),
    };
    let writers = match journal.writers {
        1 => "1 writer".to_string(),
        count => format!("{count} writers"),
    };
    let latest = match &journal.latest {
        Some(stamp) => format!("; latest {} by {}", minute(stamp.at), entry_writer(stamp)),
        None => String::new(),
    };
    format!("{}: {about} ({entries} by {writers}{latest})", journal.journal)
}

/// What one reading of a journal may take, in UTF-8 bytes: an answer's
/// budget less `run`'s own notes.
pub const JOURNAL_BYTES: usize = crate::run::window::bytes(crate::run::window::BUDGET - crate::run::window::RUN_NOTES);

/// A reading inside a World `run`, marked as `fastctx view` marks a file, so
/// the answer shows it as asked for rather than in a program's output window.
/// Control characters are written as `\xNN`, so the text cannot end the mark.
pub fn marked(text: &str) -> String {
    let mut marked = String::from(crate::run::terminal::VIEW_MARK);
    for line in text.split_inclusive('\n') {
        let (content, newline) = line.strip_suffix('\n').map_or((line, ""), |content| (content, "\n"));
        marked.push_str(&crate::text::visible(content));
        marked.push_str(newline);
    }
    marked.push_str(crate::run::terminal::VIEW_END);
    marked
}

/// The journal as one answer: a head note saying what is shown, what it
/// covers, then each entry with the World's stamp. `budget` is the most it
/// may hold, in UTF-8 bytes; the text of an entry that does not fit stops
/// where the budget does, saying where it goes on.
pub fn journal(view: &JournalView, budget: usize) -> String {
    let shown = view.entries.len();
    let last = view.first + shown.saturating_sub(1);
    let attachments: usize = view.entries.iter().map(|entry| entry.stamp.attachments.len()).sum();
    let mut facts = if shown == 0 {
        vec![format!("no entries of {}", view.total)]
    } else {
        vec![format!("entries {}-{last} of {}", view.first, view.total)]
    };
    facts.push(match view.writers {
        1 => "1 writer".to_string(),
        count => format!("{count} writers"),
    });
    if attachments > 0 {
        facts.push(format!("{attachments} attachments not shown"));
    }
    let mut text = format!("=== journal {} · {} ({}) ===\n", view.journal, view.town, facts.join("; "));
    match &view.description {
        Some(description) => text.push_str(&format!("About: {}\n", described(description))),
        None => text.push_str("About: (no description)\n"),
    }
    if view.first > 1
        && let Some(first) = view.entries.first()
    {
        text.push_str(&format!("Earlier entries: fastctx journal read {} --before {}\n", view.journal, first.stamp.id));
    }
    text.push_str(&format!("Each entry's files: {}/entries/<entry>/\n", view.directory));
    for entry in &view.entries {
        let block = entry_block(entry);
        let room = budget.saturating_sub(text.len());
        if block.len() <= room {
            text.push_str(&block);
        } else {
            // Only an entry larger than a whole answer gets here: it is the
            // one entry shown, and stops where the budget does.
            let kept = crate::answer::prefix(&block, room);
            let rest = block[kept.len()..].chars().count();
            text.push_str(kept);
            text.push_str(&format!(
                "\n… ({rest} more characters; the whole text is {}/entries/{}/text.md)\n",
                view.directory, entry.stamp.id
            ));
        }
    }
    text
}

fn entry_block(entry: &EntryView) -> String {
    let stamp = &entry.stamp;
    let mut text = format!("\n── {} · {} · {}\n", minute(stamp.at), entry_writer(stamp), stamp.id);
    if !stamp.cites.is_empty() {
        text.push_str(&format!("Cites: {}\n", ids(&stamp.cites)));
    }
    text.push_str(entry.text.trim_end());
    text.push('\n');
    if !stamp.attachments.is_empty() {
        let listed: Vec<String> = stamp.attachments.iter().map(attachment_line).collect();
        text.push_str(&format!("Attachments: {}\n", listed.join(" · ")));
    }
    if !entry.mentioned_by.is_empty() {
        text.push_str(&format!("Mentioned by: {}\n", ids(&entry.mentioned_by)));
    }
    text
}

fn attachment_line(attachment: &Attachment) -> String {
    let kind = match attachment.kind {
        AttachmentKind::Text => "text, ",
        AttachmentKind::Image => "image, ",
        AttachmentKind::Pdf => "PDF, ",
        AttachmentKind::Binary => "",
    };
    let size = size(attachment.bytes);
    match &attachment.kept {
        Kept::InEntry => format!("{} ({kind}{size})", attachment.name),
        Kept::OnMachine { location } => {
            format!("{} ({kind}{size}, kept on {} at {})", attachment.name, location.machine, location.path)
        }
    }
}

/// Who wrote an entry, with what program and model, on which machine.
pub fn entry_writer(stamp: &EntryStamp) -> String {
    let mut parts = Vec::new();
    match &stamp.by {
        Writer::Page { page } => parts.push(page.to_string()),
        Writer::User => parts.push(format!("the user on {}", stamp.machine)),
        Writer::Resident { address } => {
            if let Some(agent) = &stamp.agent {
                parts.push(program(&agent.agent, &agent.client, &agent.version));
            }
            parts.push(if address.machine == stamp.machine {
                format!("{} on {}", address.resident, stamp.machine)
            } else {
                format!("{} of {}, on {}", address.resident, address.machine, stamp.machine)
            });
            parts.push(match &stamp.model {
                Some(model) if model.self_reported => format!("model {} (self-reported)", model.name),
                Some(model) => format!("model {}", model.name),
                None => "model not known".to_string(),
            });
        }
        Writer::Session { agent } => {
            parts.push(format!("a {} session fastctx does not know, on {}", program(agent, "", ""), stamp.machine));
            if let Some(model) = &stamp.model {
                parts.push(format!("model {} (self-reported)", model.name));
            }
        }
    }
    parts.join(" · ")
}

/// Who wrote a version of something, for a line that says where already.
pub fn writer(by: &Writer, on: &MachineName) -> String {
    match by {
        Writer::Page { page } => page.to_string(),
        Writer::User => "the user".to_string(),
        Writer::Resident { address } if address.machine == *on => address.resident.to_string(),
        Writer::Resident { address } => address.tag(),
        Writer::Session { agent } => format!("a {} session fastctx does not know", program(agent, "", "")),
    }
}

/// A version of a record, in a line.
pub fn stamped(stamp: &Stamp) -> String {
    format!("{} on {}, {}", writer(&stamp.by, &stamp.on), stamp.on, minute(stamp.at))
}

pub fn record_stamp(record: &Record) -> Stamp {
    Stamp { by: record.by.clone(), on: record.on.clone(), at: record.at }
}

fn described(description: &Description) -> String {
    let edited = if description.version > 1 { "edited" } else { "written" };
    format!(
        "{} ({edited} {} by {} on {})",
        description.text,
        day(description.at),
        writer(&description.by, &description.on),
        description.on
    )
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

fn ids(ids: &[EntryId]) -> String {
    ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
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

/// A size as people read it: bytes, KB or MB, with one decimal under ten.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 { format!("{value:.1} {}", UNITS[unit]) } else { format!("{value:.0} {}", UNITS[unit]) }
}
