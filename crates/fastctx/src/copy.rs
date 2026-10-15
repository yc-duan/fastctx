//! `fastctx copy`: files and directories from one machine to another.

use std::process::ExitCode;

use clap::Args;
use fastctx_types::access::Written;
use fastctx_types::copy::{CopyReport, Place};
use fastctx_types::{Call, Reply};

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
    let mut failed = false;
    for source in sources {
        let copied = place(source).and_then(|from| crate::ask::call(Call::Copy { from, to: to.clone() }));
        match copied {
            Ok((Reply::Copied(report), _)) => {
                print!("{}", render(&report));
                failed |= !report.problems.is_empty();
            }
            Ok((other, _)) => anyhow::bail!("the node answered something else: {other:?}"),
            Err(error) if error.is::<crate::paths::Said>() => {
                eprintln!("{error}");
                failed = true;
            }
            Err(error) => {
                // Most failures already name the path they were about.
                let error = format!("{error:#}");
                let named = if error.contains(source.trim_end_matches(['/', '\\'])) { "" } else { source };
                eprintln!("fastctx copy: {named}{}{error}", if named.is_empty() { "" } else { ": " });
                failed = true;
            }
        }
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
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
