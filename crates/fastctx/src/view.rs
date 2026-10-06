//! `fastctx view`: the command around the hands' viewer.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context as _, bail};
use clap::Args;
use fastctx_client::Client;
use fastctx_hands::view::{Options, view};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::{Address, Call, TownName};

/// Look at files: text with line numbers, images as images.
#[derive(Args)]
pub struct ViewArgs {
    /// Only lines FIRST-LAST (or one line), counting from 1, of every file.
    #[arg(long, value_name = "FIRST-LAST")]
    lines: Option<String>,
    /// Read text in this encoding, such as gbk or shift_jis.
    #[arg(long, value_name = "NAME")]
    encoding: Option<String>,
    /// Also look at the NUL-separated paths in FILE (`-` for standard input).
    #[arg(long, value_name = "FILE")]
    files0_from: Option<PathBuf>,
    /// The files, shown in this order.
    paths: Vec<PathBuf>,
}

pub fn run(args: ViewArgs) -> anyhow::Result<()> {
    let mut paths = args.paths;
    if let Some(list) = &args.files0_from {
        let mut bytes = Vec::new();
        if list.as_os_str() == "-" {
            std::io::stdin().read_to_end(&mut bytes)?;
        } else {
            bytes = std::fs::read(list).with_context(|| format!("reading {}", list.display()))?;
        }
        for name in bytes.split(|&b| b == 0).filter(|name| !name.is_empty()) {
            let name = std::str::from_utf8(name).context("a path in the list is not UTF-8")?;
            paths.push(PathBuf::from(name));
        }
    }
    if paths.is_empty() {
        bail!("name the files to look at, such as: fastctx view -- /path/to/file");
    }
    let lines = args.lines.as_deref().map(parse_lines).transpose()?;
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    // Inside a World `run` the answer's first line already names the machine.
    let in_run = std::env::var_os("FASTCTX_MACHINE").is_some();
    let options =
        Options { lines, encoding: args.encoding, machine: if in_run { None } else { machine(&world) }, in_run };
    let mut out = std::io::stdout().lock();
    let seen = view(&paths, &options, &mut out)?;
    out.flush()?;
    drop(out);
    record(&world, seen)
}

/// `FIRST-LAST` or `LINE`, counting from 1.
fn parse_lines(text: &str) -> anyhow::Result<(u64, u64)> {
    let (first, last) = text.split_once('-').unwrap_or((text, text));
    let first: u64 = first.trim().parse().with_context(|| format!("--lines {text}: FIRST is not a number"))?;
    let last: u64 = last.trim().parse().with_context(|| format!("--lines {text}: LAST is not a number"))?;
    if first == 0 || last < first {
        bail!("--lines {text}: lines count from 1 and LAST is not before FIRST");
    }
    Ok((first, last))
}

/// The machine to name in head notes: the one this World is on.
fn machine(world: &WorldDir) -> Option<String> {
    let bytes = std::fs::read(world.machine_file()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("name")?.as_str().map(str::to_string)
}

/// In a resident's `run`, what it saw goes on its record, so a later
/// `write` can tell whether a file changed since.
fn record(world: &WorldDir, seen: Vec<fastctx_types::residents::Seen>) -> anyhow::Result<()> {
    let Ok(resident) = std::env::var("FASTCTX_RESIDENT") else {
        return Ok(());
    };
    if seen.is_empty() {
        return Ok(());
    }
    let resident: Address = resident.parse()?;
    let town = std::env::var("FASTCTX_TOWN").ok().map(TownName::try_from).transpose()?;
    let call = Call::Saw { owner: resident.clone(), town, seen };
    let recorded = tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let mut client = Client::connect(world, Peer::Command { resident: Some(resident), session: None }).await?;
        client.call(call).await?;
        anyhow::Ok(())
    });
    if let Err(error) = recorded {
        eprintln!("fastctx view: what you saw could not be recorded, so a later write cannot check it: {error:#}");
    }
    Ok(())
}
