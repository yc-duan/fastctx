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

/// Look at files: numbered text, images, PDF pages, and Office content.
#[derive(Args)]
pub struct ViewArgs {
    /// Only lines FIRST-LAST (or one line), counting from 1, of every file.
    #[arg(long, value_name = "FIRST-LAST")]
    lines: Option<String>,
    /// Only characters FIRST-LAST of each line, counting from 1 as head notes do; for reading on in a line too long to show at once.
    #[arg(long, value_name = "FIRST-LAST", conflicts_with_all = ["pages", "master", "sheet", "sheet_index", "range", "image"])]
    chars: Option<String>,
    /// Only PDF pages or PowerPoint slides FIRST-LAST, counting from 1.
    #[arg(long, value_name = "FIRST-LAST")]
    pages: Option<String>,
    /// Only PowerPoint master and layout text; --lines selects within it.
    #[arg(long, conflicts_with_all = ["pages", "sheet", "sheet_index", "range", "image", "encoding"])]
    master: bool,
    /// Excel sheet name, matched literally (including numeric names).
    #[arg(long, value_name = "NAME", conflicts_with_all = ["sheet_index", "lines", "pages", "image", "encoding"])]
    sheet: Option<String>,
    /// Excel sheet index, counting from 1; useful for continuing a workbook.
    #[arg(long, value_name = "NUMBER", conflicts_with_all = ["sheet", "lines", "pages", "image", "encoding"])]
    sheet_index: Option<usize>,
    /// Excel cells in an inclusive A1 rectangle, such as B2:F40 or C7.
    #[arg(long, value_name = "A1:D20", conflicts_with_all = ["lines", "pages", "image", "encoding"])]
    range: Option<String>,
    /// Render PDF pages as images instead of showing their text layer.
    #[arg(long, conflicts_with_all = ["lines", "encoding"])]
    image: bool,
    /// Read text in this encoding, such as gbk or shift_jis; a list such as utf-8,gbk is tried line by line, for a log that mixes them.
    #[arg(long, value_name = "NAME")]
    encoding: Option<String>,
    /// Also look at the NUL-separated paths in FILE (`-` for standard input).
    #[arg(long, value_name = "FILE")]
    files0_from: Option<PathBuf>,
    /// Also look at a file the user attached in this conversation, by the id
    /// its message gives, wherever it is kept.
    #[arg(long = "attachment", value_name = "ID")]
    attachments: Vec<String>,
    /// The files, shown in this order.
    paths: Vec<PathBuf>,
}

pub fn run(args: ViewArgs) -> anyhow::Result<std::process::ExitCode> {
    let mut paths = args.paths;
    if let Some(list) = &args.files0_from {
        let mut bytes = Vec::new();
        if list.as_os_str() == "-" {
            std::io::stdin().read_to_end(&mut bytes)?;
        } else {
            let list = crate::paths::absolute("fastctx view", list)?;
            bytes = std::fs::read(&list).with_context(|| format!("reading {}", list.display()))?;
        }
        for name in bytes.split(|&b| b == 0).filter(|name| !name.is_empty()) {
            let name = std::str::from_utf8(name).context("a path in the list is not UTF-8")?;
            paths.push(PathBuf::from(name));
        }
    }
    paths.extend(attached(&args.attachments)?);
    if paths.is_empty() {
        bail!("name the files to look at, such as: fastctx view -- /path/to/file");
    }
    for path in &paths {
        crate::paths::absolute("fastctx view", path)?;
    }
    let lines = args.lines.as_deref().map(|text| parse_range(text, "lines")).transpose()?;
    let chars = args.chars.as_deref().map(|text| parse_range(text, "chars")).transpose()?;
    let pages = args.pages.as_deref().map(|text| parse_range(text, "pages")).transpose()?;
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    // Inside a World `run` the answer's first line already names the machine.
    let in_run = std::env::var_os("FASTCTX_MACHINE").is_some();
    let options = Options {
        lines,
        chars,
        pages,
        master: args.master,
        sheet: args.sheet,
        sheet_index: args.sheet_index,
        range: args.range.as_deref().map(str::parse).transpose()?,
        image: args.image,
        encoding: args.encoding,
        machine: if in_run { None } else { machine(&world) },
        in_run,
    };
    let began = crate::steps::began();
    let mut out = crate::output::out();
    let viewed = view(&paths, &options, &mut out)?;
    out.flush()?;
    drop(out);
    if let Some(began) = began {
        let mut pictures = viewed.pictures;
        pictures.truncate(fastctx_types::run_facts::STEP_ENTRIES);
        let paths = paths
            .iter()
            .take(fastctx_types::run_facts::STEP_ENTRIES)
            .map(|path| fastctx_types::run_facts::clipped(&path.to_string_lossy()))
            .collect();
        began.tell(fastctx_types::run_facts::StepDid::View { paths, pictures });
    }
    record(&world, viewed.seen)?;
    Ok(if viewed.failed { std::process::ExitCode::FAILURE } else { std::process::ExitCode::SUCCESS })
}

/// The files the user attached in this conversation, named by id, read from
/// wherever they are kept into this machine's scratch, to be looked at there.
fn attached(ids: &[String]) -> anyhow::Result<Vec<PathBuf>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let dir = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let world = fastctx_client::World::here()?;
    let scratch = match std::env::var_os("FASTCTX_SCRATCH") {
        Some(scratch) => PathBuf::from(scratch),
        None => std::env::temp_dir().join("fastctx-attachments"),
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let mut paths = Vec::new();
        for id in ids {
            let id = fastctx_types::NoticeId::try_from(id.as_str())
                .map_err(|_| anyhow::anyhow!("{id} is not an attachment id"))?;
            let mut client = Client::connect(&dir, fastctx_client::command_peer()?).await?;
            let fastctx_types::Reply::Uploaded(receipt) = client.call(Call::OwnAttachment { id: id.clone() }).await?
            else {
                bail!("the node answered something other than attachment {id}");
            };
            drop(client);
            let folder = scratch.join("attachments").join(id.as_str());
            std::fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
            let path = folder.join(file_name(&receipt.file.name));
            let mut file = std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
            let mut reading = world.read_attachment(fastctx_types::attachments::AttachmentSource::Upload(receipt));
            while let Some(read) = reading.next().await? {
                match read {
                    // The file changed while the read broke off: it begins again.
                    fastctx_client::file_read::Read::Opened(_) => {
                        file.set_len(0)?;
                        std::io::Seek::rewind(&mut file)?;
                    }
                    fastctx_client::file_read::Read::Bytes(bytes) => file.write_all(&bytes)?,
                }
            }
            file.flush()?;
            paths.push(path);
        }
        Ok(paths)
    })
}

/// The last part of an attached file's name, as a name for its copy here.
fn file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String =
        last.chars().map(|c| if c.is_control() || ":*?\"<>|".contains(c) { '_' } else { c }).collect();
    if cleaned.trim_matches('.').is_empty() { "attachment".into() } else { cleaned }
}

/// `FIRST-LAST` or `LINE`, counting from 1.
fn parse_range(text: &str, option: &str) -> anyhow::Result<(u64, u64)> {
    let (first, last) = text.split_once('-').unwrap_or((text, text));
    let first: u64 = first.trim().parse().with_context(|| format!("--{option} {text}: FIRST is not a number"))?;
    let last: u64 = last.trim().parse().with_context(|| format!("--{option} {text}: LAST is not a number"))?;
    if first == 0 || last < first {
        bail!("--{option} {text}: {option} count from 1 and LAST is not before FIRST");
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
