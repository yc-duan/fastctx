//! `fastctx on`: what the World's shell function `on` calls to run part of
//! a script on another machine (`world-run.md`, 去别的机器).
//!
//! It collects everything to send before sending anything: the state the
//! function wrote, the command's words or the script, and its standard
//! input read to the end. Then the node sends the package and hands back
//! the job's standard output and standard error as they come, and this
//! command exits with the job's code. Leaving before the end stops the job.

use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use clap::Args;
use fastctx_client::{CallError, Client};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::on::{OnEnd, OnPackage, OnPiece, OnRequest};
use fastctx_types::run::EndedHow;
use fastctx_types::{Address, Call, JobId, MachineName, TownName};

/// What `on` exits with when the World could not carry the command through,
/// as ssh does (`world-run.md`).
const NOT_CARRIED: u8 = 255;

/// Run part of a script on another machine of the World: `on MACHINE
/// [COMMAND [ARG...]]` in the World's shell; without a command, standard
/// input is the script.
#[derive(Args)]
pub struct OnArgs {
    /// The state the shell function wrote; this command takes it over.
    #[arg(long, value_name = "FILE")]
    state: Option<PathBuf>,
    /// The state also holds the command's words, as `__fastctx_argv`.
    #[arg(long)]
    command: bool,
    /// The machine, as `fastctx nodes` names it.
    machine: Option<String>,
    /// A command and its words, when `fastctx on` is called directly.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    words: Vec<String>,
}

pub fn run(args: OnArgs) -> ExitCode {
    match on(args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("on: {error:#}");
            ExitCode::from(NOT_CARRIED)
        }
    }
}

fn on(args: OnArgs) -> anyhow::Result<ExitCode> {
    let Some(machine) = args.machine else {
        bail!(
            "usage: on MACHINE [COMMAND [ARG...]]; without a command, standard input is the script. `fastctx nodes` lists the machines"
        );
    };
    let machine = MachineName::try_from(machine.as_str())?;
    let resident: Address = std::env::var("FASTCTX_RESIDENT")
        .context("on runs part of a resident's script, in the World's shell (run)")?
        .parse()?;
    let town = std::env::var("FASTCTX_TOWN").ok().map(TownName::try_from).transpose()?;
    let job = std::env::var("FASTCTX_JOB").ok().map(JobId::try_from).transpose()?;
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let package = collect(&args.state, args.command, &args.words)?;
    let request = OnRequest { machine: machine.clone(), job, package: package.clone() };
    let call = Call::On { resident: resident.clone(), town, request };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let ended = runtime.block_on(async {
        let client = Client::connect(&world, Peer::Command { resident: Some(resident), session: None }).await?;
        let mut stream = client.on(call).await?;
        let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
        loop {
            match stream.next().await {
                Ok(Some(OnPiece::Started { machine: there, .. })) => {
                    // What went to another machine is there now; what stays
                    // here is the job's own input.
                    if std::env::var("FASTCTX_MACHINE").ok().as_deref() != Some(there.as_str()) {
                        forget(&package);
                    }
                }
                Ok(Some(OnPiece::Out { base64 })) => {
                    // A reader that stopped reading, as `| head` does, is a
                    // stop: leaving stops the job there.
                    if out.write_all(&STANDARD.decode(base64)?).and_then(|()| out.flush()).is_err() {
                        return Ok(Ended::ReaderLeft);
                    }
                }
                Ok(Some(OnPiece::Err { base64 })) => {
                    let _ = err.write_all(&STANDARD.decode(base64)?).and_then(|()| err.flush());
                }
                Ok(Some(OnPiece::Ended { end })) => return Ok(Ended::Job(end)),
                Ok(None) => bail!("the node ended the on without the job's end"),
                Err(CallError::Failed(failure)) => return Ok(Ended::Refused(failure.message)),
                Err(error) => return Err(error.into()),
            }
        }
    })?;
    Ok(match ended {
        Ended::Job(end) => {
            if let OnEnd::Otherwise { how, .. } = &end {
                eprintln!("on: on {machine}, {}", how_ended(how));
            }
            ExitCode::from(end.code() as u8)
        }
        // As bash reports a pipeline stage that SIGPIPE ended.
        Ended::ReaderLeft => ExitCode::from(141),
        Ended::Refused(why) => {
            eprintln!("on: {why}");
            ExitCode::from(NOT_CARRIED)
        }
    })
}

enum Ended {
    Job(OnEnd),
    ReaderLeft,
    Refused(String),
}

fn how_ended(how: &EndedHow) -> String {
    match how {
        EndedHow::Finished => "it finished".to_string(),
        EndedHow::Stopped => "it was stopped".to_string(),
        EndedHow::Interrupted { reason } => format!("it was interrupted: {reason}"),
        EndedHow::Failed { error } => format!("it could not run: {error}"),
    }
}

/// Writes the package beside the state: the state, the command's words or
/// the script, and standard input read to its end.
fn collect(state: &Option<PathBuf>, command: bool, words: &[String]) -> anyhow::Result<OnPackage> {
    let scratch = match std::env::var_os("FASTCTX_SCRATCH") {
        Some(scratch) => PathBuf::from(scratch),
        None => std::env::temp_dir(),
    };
    let directory = scratch.join(format!("on-{}", random()?));
    std::fs::create_dir_all(&directory).with_context(|| format!("making {}", directory.display()))?;
    let state_path = directory.join("state");
    match state {
        Some(state) => {
            std::fs::rename(state, &state_path)
                .or_else(|_| std::fs::copy(state, &state_path).map(|_| ()))
                .with_context(|| format!("taking over {}", state.display()))?;
            let _ = std::fs::remove_file(state);
        }
        None => std::fs::write(&state_path, b"")?,
    }
    if !words.is_empty() {
        let mut file = std::fs::OpenOptions::new().append(true).open(&state_path)?;
        writeln!(
            file,
            "declare -a __fastctx_argv=({})",
            words.iter().map(|word| quoted(word)).collect::<Vec<_>>().join(" ")
        )?;
    }
    let sends_words = command || !words.is_empty();
    let stdin = std::io::stdin();
    let read = |path: &Path| -> anyhow::Result<u64> {
        let mut file = std::fs::File::create(path)?;
        Ok(std::io::copy(&mut stdin.lock(), &mut file)?)
    };
    let (script, input) = if sends_words {
        // A terminal's input has no end to read to; the command gets none.
        if stdin.is_terminal() {
            (None, None)
        } else {
            let path = directory.join("input");
            let size = read(&path)?;
            (None, (size > 0).then(|| path.to_string_lossy().into_owned()))
        }
    } else {
        if stdin.is_terminal() {
            bail!(
                "no command, and no script on standard input: write on MACHINE COMMAND, or give the script as a here-document"
            );
        }
        let path = directory.join("script");
        if read(&path)? == 0 {
            bail!("no command, and the script on standard input is empty");
        }
        (Some(path.to_string_lossy().into_owned()), None)
    };
    Ok(OnPackage {
        state: state_path.to_string_lossy().into_owned(),
        script,
        input,
        writes_off: std::env::var("FASTCTX_WRITES").is_ok_and(|value| value == "off"),
    })
}

/// Removes a package another machine now holds.
fn forget(package: &OnPackage) {
    if let Some(directory) = Path::new(&package.state).parent() {
        let _ = std::fs::remove_dir_all(directory);
    }
}

/// A word as bash reads it back exactly: in single quotes, each single
/// quote written as `'\''`.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn random() -> anyhow::Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
