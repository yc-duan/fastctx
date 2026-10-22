//! File-writing commands: literal stdin, with identity and writes owned by the node.

use std::io::Read as _;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, bail};
use clap::Args;
use fastctx_client::Client;
use fastctx_types::layout::WorldDir;
use fastctx_types::write::{ApplyPatchRequest, EditBlocksRequest, WriteOp, WriteRequest};
use fastctx_types::{Call, Reply, TownName};

#[derive(Args)]
pub struct EditArgs {
    /// Absolute path of the file to edit.
    path: PathBuf,
    /// Replace every occurrence of each SEARCH text.
    #[arg(long)]
    all: bool,
    /// Use this text encoding or ordered per-line list, as in view --encoding.
    #[arg(long, value_name = "NAME")]
    encoding: Option<String>,
}

#[derive(Args)]
pub struct WriteArgs {
    /// Absolute path of the file to create or replace.
    path: PathBuf,
    /// Leave out the final newline the here-doc adds.
    #[arg(short = 'n')]
    no_newline: bool,
    /// Use this text encoding, as in view --encoding.
    #[arg(long, value_name = "NAME")]
    encoding: Option<String>,
}

pub fn edit(args: EditArgs) -> anyhow::Result<ExitCode> {
    enabled()?;
    let file_path = crate::paths::absolute("edit", &args.path)?;
    execute(WriteOp::EditBlocks(EditBlocksRequest {
        file_path,
        input: input()?,
        replace_all: args.all,
        encoding: args.encoding,
    }))
}

pub fn write(args: WriteArgs) -> anyhow::Result<ExitCode> {
    enabled()?;
    let file_path = crate::paths::absolute("write", &args.path)?;
    let mut content = input()?;
    if args.no_newline && content.ends_with('\n') {
        content.pop();
    }
    execute(WriteOp::Write(WriteRequest { file_path, content, encoding: args.encoding }))
}

pub fn apply_patch() -> anyhow::Result<ExitCode> {
    enabled()?;
    execute(WriteOp::ApplyPatch(ApplyPatchRequest { input: input()? }))
}

fn enabled() -> anyhow::Result<()> {
    if std::env::var_os("FASTCTX_WRITES").as_deref() == Some(std::ffi::OsStr::new("off")) {
        bail!("edit and write are turned off on this machine; use your own file-writing tool");
    }
    Ok(())
}

fn input() -> anyhow::Result<String> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).context("read UTF-8 text from standard input")?;
    Ok(input)
}

fn execute(op: WriteOp) -> anyhow::Result<ExitCode> {
    let job = std::env::var("FASTCTX_JOB").ok().map(|job| job.parse()).transpose()?;
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let peer = fastctx_client::command_peer()?;
    // An invalid stamped identity must not fall through to a host-session hint.
    if let Some(address) = std::env::var_os("FASTCTX_RESIDENT") {
        address
            .to_str()
            .context("FASTCTX_RESIDENT must be UTF-8")?
            .parse::<fastctx_types::Address>()
            .context("invalid FASTCTX_RESIDENT")?;
    }
    let town = std::env::var_os("FASTCTX_TOWN")
        .map(|town| {
            TownName::try_from(town.to_str().context("FASTCTX_TOWN must be UTF-8")?).map_err(anyhow::Error::from)
        })
        .transpose()?;
    let reply = tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let mut client = Client::connect(&world, peer).await?;
        anyhow::Ok(client.call(Call::Write { job, resident: None, op, node: None, town }).await?)
    })?;
    match reply {
        Reply::Written { outcome: Ok(result) } => {
            println!("{}", result.text);
            Ok(ExitCode::SUCCESS)
        }
        Reply::Written { outcome: Err(error) } => {
            eprintln!("{error}");
            Ok(ExitCode::FAILURE)
        }
        _ => bail!("the node answered something other than a write"),
    }
}
