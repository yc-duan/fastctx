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
use fastctx_types::access::Written;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::on::{OnEnd, OnPackage, OnPiece, OnRequest, OnSource, PIPE_LINE_MAX};
use fastctx_types::run::EndedHow;
use fastctx_types::{Address, Call, JobId, TownName};

/// What `on` exits with when the World could not carry the command through,
/// as ssh does (`world-run.md`).
const NOT_CARRIED: u8 = 255;

/// How `on` is written in the World's shell, which is what a reader of its
/// help needs; `fastctx on` itself is what the shell function calls.
pub const USAGE: &str = "on MACHINE COMMAND [ARG...]
       on MACHINE <<'EOF'
       ...the script...
       EOF";

pub const AFTER_HELP: &str = "The command's words are sent exactly as this shell expanded them. As with ssh, standard input is the command's when a command follows, and the script to run there, read only by that machine's bash, when none does.
The part sent there sees your script's variables, functions and shell options; PATH, HOME, LANG and the machine's other settings are its own. Its output and exit code work like any command's, in pipelines and `if` too. A script quoted as one word is refused: use a here-document.
If the connection drops, it goes on there and its end still reaches you. It exits 255 when it could not reach that machine.";

/// Run part of a script on another machine of the World, as the World's
/// shell writes it (`USAGE`).
#[derive(Args)]
pub struct OnArgs {
    /// The shell running the `on` function, which this follower ends with:
    /// its process id, or on Windows its native one.
    #[arg(long, hide = true)]
    shell_parent: Option<u32>,
    /// The state the shell function wrote; this command takes it over.
    #[arg(long, value_name = "FILE", hide = true)]
    state: Option<PathBuf>,
    /// The command line the shell function quoted from the command's words.
    #[arg(long, value_name = "FILE", hide = true)]
    command: Option<PathBuf>,
    /// The script pipes another `on`'s output into this one, and that
    /// output may have gone straight on: what comes is then where it is.
    #[arg(long, hide = true)]
    pipe_in: bool,
    /// The script pipes this one's output into an `on` to this machine.
    #[arg(long, value_name = "MACHINE", hide = true)]
    pipe_to: Option<String>,
    /// The machine, as `fastctx nodes` names it; the usage says it.
    #[arg(hide = true)]
    machine: Option<String>,
    /// A command and its words; without them, standard input is the script.
    #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
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
    if let Some(parent) = args.shell_parent {
        fastctx_platform::parent::follow_shell(parent)?;
    }
    let Some(machine) = args.machine else {
        bail!(
            "usage: on MACHINE COMMAND [ARG...], or on MACHINE with the script on standard input. `fastctx nodes` lists the machines"
        );
    };
    let written = Written::parse(machine.as_str())?;
    let machine = written.name.clone();
    let resident: Address =
        std::env::var("FASTCTX_RESIDENT").context("on works only inside a script run by the run tool")?.parse()?;
    let town = std::env::var("FASTCTX_TOWN").ok().map(TownName::try_from).transpose()?;
    let job = std::env::var("FASTCTX_JOB").ok().map(JobId::try_from).transpose()?;
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let package = collect(&args.state, args.command.as_deref(), &args.words, args.pipe_in)?;
    let request = OnRequest {
        machine: machine.clone(),
        hub: written.hub.clone(),
        receipt: true,
        process: Some(fastctx_platform::current_identity()?),
        job,
        package: package.clone(),
        pipe_to: args.pipe_to.as_deref().map(Written::parse).transpose()?,
    };
    let call = Call::On { resident: resident.clone(), town, request };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let ended = runtime.block_on(async {
        let client = Client::connect(&world, Peer::Command { resident: Some(resident), session: None }).await?;
        let here = client.machine.clone();
        let mut stream = client.on(call).await?;
        let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
        let mut started = None;
        loop {
            match stream.next().await {
                Ok(Some(OnPiece::Started { machine: there, job })) => {
                    started = Some(job);
                    // What went to another machine is there now; what stays
                    // here is the job's own input.
                    if here != there {
                        forget(&package);
                    }
                }
                // Its output stays there for the `on` reading it, which is
                // told where.
                Ok(Some(OnPiece::Piped { source })) => {
                    if out.write_all(source.line().as_bytes()).and_then(|()| out.flush()).is_err() {
                        stream.stop().await?;
                        return Ok(Ended::ReaderLeft);
                    }
                }
                Ok(Some(OnPiece::Out { base64 })) => {
                    // A reader that stopped reading, as `| head` does, is a
                    // stop: leaving stops the job there.
                    if out.write_all(&STANDARD.decode(base64)?).and_then(|()| out.flush()).is_err() {
                        stream.stop().await?;
                        return Ok(Ended::ReaderLeft);
                    }
                }
                Ok(Some(OnPiece::Err { base64 })) => {
                    if err.write_all(&STANDARD.decode(base64)?).and_then(|()| err.flush()).is_err() {
                        stream.stop().await?;
                        return Ok(Ended::ReaderLeft);
                    }
                }
                Ok(Some(OnPiece::Ended { end })) => {
                    // The authoritative result has already arrived. Receipt transport
                    // failure cannot change the command's known exit status.
                    if let Err(error) = stream.taken().await {
                        eprintln!("on: keeping the completion receipt was not confirmed: {error}");
                    }
                    return Ok(Ended::Job(end, started));
                }
                Ok(None) => bail!("the node ended the on without the job's end"),
                Err(CallError::Failed(failure)) if failure.not_run => return Ok(Ended::Refused(failure.message)),
                Err(CallError::Failed(failure)) => return Ok(Ended::Lost(failure.message)),
                Err(error) => return Err(error.into()),
            }
        }
    })?;
    Ok(match ended {
        Ended::Job(end, job) => {
            match &end {
                OnEnd::Otherwise { how, .. } => eprintln!("on: on {machine}, {}", how_ended(how)),
                // A subshell's background processes are its caller's to
                // know about: the command ended, they did not.
                OnEnd::Exited { remaining, .. } if *remaining > 0 => {
                    let processes = if *remaining == 1 {
                        "1 process it started".to_string()
                    } else {
                        format!("{remaining} processes it started")
                    };
                    let stop =
                        job.map(|job| format!("; `fastctx stop {job}@{machine}` stops them")).unwrap_or_default();
                    eprintln!("on: on {machine}, the command exited, but {processes} still run there{stop}");
                }
                OnEnd::Exited { .. } => {}
            }
            ExitCode::from(end.code() as u8)
        }
        // As bash reports a pipeline stage that SIGPIPE ended.
        Ended::ReaderLeft => ExitCode::from(141),
        Ended::Refused(why) => {
            eprintln!("on: not run on {machine}: {why}");
            ExitCode::from(NOT_CARRIED)
        }
        Ended::Lost(why) => {
            eprintln!("on: {why}");
            ExitCode::from(NOT_CARRIED)
        }
    })
}

enum Ended {
    /// It ended there, as the job that started.
    Job(OnEnd, Option<JobId>),
    ReaderLeft,
    /// It never started there.
    Refused(String),
    /// It started there, and then its end could not be had.
    Lost(String),
}

fn how_ended(how: &EndedHow) -> String {
    match how {
        EndedHow::Finished => "it finished".to_string(),
        EndedHow::Stopped => "it was stopped".to_string(),
        EndedHow::Interrupted { reason } => format!("it was interrupted: {reason}"),
        EndedHow::Failed { error } => format!("it failed: {error}"),
    }
}

/// Writes the package beside the state: the state, the command's words or
/// the script, and standard input read to its end, or where it is kept when
/// it is the output of an `on` that went straight on.
fn collect(
    state: &Option<PathBuf>,
    command: Option<&Path>,
    words: &[String],
    pipe_in: bool,
) -> anyhow::Result<OnPackage> {
    let scratch = match std::env::var_os("FASTCTX_JOB_TEMP") {
        Some(scratch) => PathBuf::from(scratch),
        None => fastctx_platform::temp_dir().context("finding the temporary directory")?,
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
    // A command's words become one command line that bash there reads back
    // into exactly these words: quoted by the sending bash (`%q`), or here.
    let command_line = match command {
        Some(path) => {
            let line = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            let _ = std::fs::remove_file(path);
            Some(line)
        }
        None if !words.is_empty() => Some(words.iter().map(|word| quoted(word)).collect::<Vec<_>>().join(" ")),
        None => None,
    };
    let stdin = std::io::stdin();
    let read = |path: &Path| -> anyhow::Result<u64> {
        let mut file = std::fs::File::create(path)?;
        Ok(std::io::copy(&mut stdin.lock(), &mut file)?)
    };
    let mut source = None;
    let (script, input) = if let Some(line) = command_line {
        let script = directory.join("script");
        // Bash's %q can end in an escaped space. Whitespace here belongs
        // to the quoted arguments and must survive byte for byte.
        std::fs::write(&script, format!("{line}\n"))?;
        // A terminal's input has no end to read to; the command gets none.
        let input = if stdin.is_terminal() {
            None
        } else {
            let path = directory.join("input");
            let size = read(&path)?;
            if pipe_in
                && size <= PIPE_LINE_MAX
                && let Some(found) = OnSource::read(&std::fs::read(&path)?)
            {
                std::fs::remove_file(&path)?;
                source = Some(found);
                None
            } else {
                (size > 0).then(|| path.to_string_lossy().into_owned())
            }
        };
        (script.to_string_lossy().into_owned(), input)
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
        (path.to_string_lossy().into_owned(), None)
    };
    Ok(OnPackage {
        state: state_path.to_string_lossy().into_owned(),
        script,
        input,
        source,
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
