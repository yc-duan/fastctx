//! `fastctx copy`: files and directories from one machine to another.

use std::process::ExitCode;

use clap::Args;
use fastctx_types::access::Written;
use fastctx_types::copy::{CopyReport, Place};
use fastctx_types::run_facts::{STEP_ENTRIES, StepDid, StepPlace, clipped};
use fastctx_types::{Call, MachineName, Reply};

/// The ends of a copy; what a copy does is said on the `copy` command.
#[derive(Args)]
pub struct CopyArgs {
    /// What to copy, then where to, each MACHINE:PATH or a path on this
    /// machine.
    #[arg(value_name = "SOURCE... DEST", num_args = 2.., required = true)]
    ends: Vec<String>,
}

/// Each source is copied on its own, as `cp` does: one that fails is said
/// and the rest still go, and the command fails if any did.
pub fn run(args: CopyArgs) -> anyhow::Result<ExitCode> {
    let (to, sources) = args.ends.split_last().expect("clap requires two ends");
    if sources.len() > 1 && !(to.ends_with('/') || (cfg!(windows) && to.ends_with('\\'))) {
        anyhow::bail!("with several sources, end the destination with / to copy them into that directory");
    }
    let to = place(to)?;
    let began = crate::steps::began();
    let mut told = Told::default();
    let mut failed = false;
    for source in sources {
        let copied = place(source).and_then(|from| crate::ask::call(Call::Copy { from, to: to.clone() }));
        match copied {
            Ok((Reply::Copied(report), _)) => {
                print!("{}", render(&report));
                failed |= !report.problems.is_empty();
                told.copied(report);
            }
            Ok((other, _)) => anyhow::bail!("the node answered something else: {other:?}"),
            Err(error) if error.is::<crate::paths::Said>() => {
                eprintln!("{error}");
                told.problems.push(error.to_string());
                failed = true;
            }
            Err(error) => {
                // Most failures already name the path they were about.
                let error = format!("{error:#}");
                let named = if error.contains(source.trim_end_matches(['/', '\\'])) { "" } else { source };
                let said = format!("{named}{}{error}", if named.is_empty() { "" } else { ": " });
                eprintln!("fastctx copy: {said}");
                told.problems.push(said);
                failed = true;
            }
        }
    }
    if let Some(began) = began
        && let Some(did) = told.did(&to)
    {
        began.tell(did);
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// What the copy's sources brought, as its step tells it.
#[derive(Default)]
struct Told {
    from: Vec<StepPlace>,
    to: Option<MachineName>,
    files: u64,
    bytes: u64,
    unchanged: u64,
    problems: Vec<String>,
}

impl Told {
    fn copied(&mut self, report: CopyReport) {
        self.from.push(StepPlace { machine: report.from_machine, path: report.from });
        self.to.get_or_insert(report.to_machine);
        self.files += report.files;
        self.bytes += report.bytes;
        self.unchanged += report.unchanged;
        self.problems.extend(report.problems);
    }

    fn did(self, to: &Place) -> Option<StepDid> {
        let machine = self.to.or_else(|| to.machine.clone()).or_else(crate::steps::here)?;
        let keep = |mut list: Vec<String>| {
            list.truncate(STEP_ENTRIES);
            list.iter().map(|text| clipped(text)).collect()
        };
        let mut from = self.from;
        from.truncate(STEP_ENTRIES);
        for place in &mut from {
            place.path = clipped(&place.path);
        }
        Some(StepDid::Copy {
            from,
            to: StepPlace { machine, path: clipped(&to.path) },
            files: self.files,
            bytes: self.bytes,
            unchanged: self.unchanged,
            problems: keep(self.problems),
        })
    }
}

/// `MACHINE:PATH`, or a path on this machine, each absolute. A prefix of
/// one English letter is a Windows drive, not a machine.
fn place(text: &str) -> anyhow::Result<Place> {
    if let Some((machine, path)) = text.split_once(':')
        && !(machine.len() == 1 && machine.as_bytes()[0].is_ascii_alphabetic())
        && let Ok(written) = Written::parse(machine)
    {
        let bytes = path.as_bytes();
        let rooted = bytes.first().is_some_and(|byte| matches!(byte, b'/' | b'\\'))
            || bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\');
        if !rooted {
            return Err(crate::paths::Said(format!(
                "fastctx copy {text}: fastctx copy takes absolute paths, and {path} on {machine} is not one."
            ))
            .into());
        }
        return Ok(Place { machine: Some(written.name), hub: written.hub, path: path.to_string() });
    }
    let mut path = crate::paths::absolute("fastctx copy", std::path::Path::new(text))?.to_string_lossy().into_owned();
    if cfg!(windows) {
        path = path.replace('\\', "/");
    }
    // Absolute paths keep no trailing separator; it says "into".
    let into = text.ends_with('/') || (cfg!(windows) && text.ends_with('\\'));
    if into && !path.ends_with(['/', '\\']) {
        path.push('/');
    }
    Ok(Place { machine: None, hub: None, path })
}

fn render(report: &CopyReport) -> String {
    let mut facts = vec![format!("{}, {}", count(report.files, "file", "files"), size(report.bytes))];
    if report.unchanged > 0 {
        facts.push(format!("{} already the same", report.unchanged));
    }
    if report.directories > 0 {
        facts.push(count(report.directories, "directory", "directories"));
    }
    if report.links > 0 {
        facts.push(count(report.links, "link", "links"));
    }
    if !report.problems.is_empty() {
        facts.push(format!("{} not copied", report.problems.len()));
    }
    let mut text = format!(
        "=== copy {}:{} -> {}:{} ({}) ===\n",
        report.from_machine,
        report.from,
        report.to_machine,
        report.to,
        facts.join("; ")
    );
    for problem in &report.problems {
        text.push_str(&format!("not copied: {problem}\n"));
    }
    text
}

fn count(n: u64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

pub(crate) fn size(bytes: u64) -> String {
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
    format!("{value:.1} {}", UNITS[unit])
}
