//! `fastctx copy`: files and directories from one machine to another.

use std::process::ExitCode;

use clap::Args;
use fastctx_client::Client;
use fastctx_types::copy::{CopyReport, Place};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::{Address, Call, MachineName, Reply};

/// Copy a file or a directory from one machine of the World to another.
///
/// Each end is MACHINE:PATH, or a path on this machine. The destination
/// path is what the copy becomes; ending it with / copies into that
/// directory instead. Files already there with the same contents are not
/// sent again, so running the same copy again sends only what changed.
/// Links are copied as links. The bytes go between the machines, never
/// through the terminal.
#[derive(Args)]
pub struct CopyArgs {
    /// What to copy: MACHINE:PATH, or a path on this machine.
    from: String,
    /// Where to: MACHINE:PATH, or a path on this machine.
    to: String,
}

pub fn run(args: CopyArgs) -> anyhow::Result<ExitCode> {
    let (from, to) = (place(&args.from)?, place(&args.to)?);
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let you = std::env::var("FASTCTX_RESIDENT").ok().and_then(|address| address.parse::<Address>().ok());
    let reply = tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let mut client = Client::connect_or_start(&world, Peer::Command { resident: you }).await?;
        anyhow::Ok(client.call(Call::Copy { from, to }).await?)
    })?;
    match reply {
        Reply::Copied(report) => {
            print!("{}", render(&report));
            Ok(if report.problems.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        other => anyhow::bail!("the node answered something else: {other:?}"),
    }
}

/// `MACHINE:PATH`, or a path on this machine made absolute. A one-letter
/// prefix is a Windows drive, not a machine.
fn place(text: &str) -> anyhow::Result<Place> {
    if let Some((machine, path)) = text.split_once(':')
        && machine.len() >= 2
        && let Ok(machine) = MachineName::try_from(machine)
    {
        return Ok(Place { machine: Some(machine), path: path.to_string() });
    }
    let mut path = std::path::absolute(text)?.to_string_lossy().into_owned();
    if cfg!(windows) {
        path = path.replace('\\', "/");
    }
    // Absolute paths keep no trailing separator; it says "into".
    let into = text.ends_with('/') || (cfg!(windows) && text.ends_with('\\'));
    if into && !path.ends_with(['/', '\\']) {
        path.push('/');
    }
    Ok(Place { machine: None, path })
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

fn size(bytes: u64) -> String {
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
