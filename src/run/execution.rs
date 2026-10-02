//! Persistent execution and notification; the caller owns neither the child nor its pipes.

use super::{JobRecord, JobState, RunRequest, RunResult, WAIT, platform, record};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct Launch {
    request: RunRequest,
    id: String,
    record: PathBuf,
    output: PathBuf,
    executable: PathBuf,
}

/// Starts an independently owned job and waits at most fifteen seconds for completion.
/// Output assembly reads a fixed log snapshot; its I/O/encoding cost is additional to waiting.
pub fn execute(request: RunRequest) -> Result<RunResult, String> {
    execute_inner(request).map_err(|error| error.to_string())
}

fn execute_inner(request: RunRequest) -> io::Result<RunResult> {
    let deadline = Instant::now() + WAIT;
    if !request.cwd.is_absolute() || !request.directory.is_absolute() {
        return Err(io::Error::other(
            "run requires absolute cwd and directory paths",
        ));
    }
    if !request.cwd.is_dir() {
        return Err(io::Error::other("run cwd is not a directory"));
    }
    let (id, record_path, output) = record::reserve(&request.directory)?;
    let launch = Launch {
        request,
        id,
        record: record_path.clone(),
        output,
        executable: std::env::current_exe()?,
    };
    let request_path = record_path.with_extension("request");
    record::write(&request_path, &launch)?;
    let diagnostics = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(record_path.with_extension("supervisor.log"))?;
    platform::launch(&launch.executable, &request_path, &diagnostics)?;
    let job = loop {
        match record::read(&record_path) {
            Ok(job) => break job,
            Err(error) if error.kind() == io::ErrorKind::NotFound && Instant::now() < deadline => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(io::Error::other(format!(
                    "run {} has not published its start record within 15s; inspect {} (the launch is not cancelled)",
                    launch.id,
                    record_path.with_extension("supervisor.log").display()
                )));
            }
            Err(error) => return Err(error),
        }
        std::thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    };
    let mut output = LiveLog {
        file: File::open(&job.output_path)?,
        job,
        record: record_path.clone(),
        deadline,
        offset: 0,
        end: None,
    };
    let window = super::protocol::scan(&mut output)?;
    super::output::assemble_window(output.job, record_path, output.offset, window)
}

/// Streams the persisted log while execution continues. At the wait boundary the length
/// becomes immutable; later writes remain in the log for a subsequent reader.
struct LiveLog {
    file: File,
    job: JobRecord,
    record: PathBuf,
    deadline: Instant,
    offset: u64,
    end: Option<u64>,
}

impl LiveLog {
    fn observe(&mut self) -> io::Result<()> {
        self.job = record::read(&self.record)?;
        if self.job.state != JobState::Running || Instant::now() >= self.deadline {
            self.end = Some(self.file.metadata()?.len());
        }
        Ok(())
    }
}

impl Read for LiveLog {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if self.end.is_none() && Instant::now() >= self.deadline {
                self.observe()?;
            }
            let count = if let Some(end) = self.end {
                (&mut self.file).take(end - self.offset).read(buffer)?
            } else {
                self.file.read(buffer)?
            };
            self.offset += count as u64;
            if count > 0 || self.end.is_some() {
                return Ok(count);
            }
            self.observe()?;
            if self.end.is_none() {
                std::thread::sleep(
                    Duration::from_millis(10)
                        .min(self.deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }
}

pub(super) fn supervise(path: &Path) -> Result<(), String> {
    supervise_inner(path).map_err(|error| {
        let message = format!("run supervisor: {error}");
        let written = OpenOptions::new()
            .append(true)
            .open(path.with_extension("supervisor.log"))
            .and_then(|mut output| writeln!(output, "{message}"));
        match written {
            Ok(()) => message,
            Err(diagnostic) => {
                format!("{message}; cannot write supervisor diagnostic: {diagnostic}")
            }
        }
    })
}

fn supervise_inner(path: &Path) -> io::Result<()> {
    let launch: Launch = serde_json::from_reader(io::BufReader::new(File::open(path)?))?;
    let lock = record::lock(&launch.record)?;
    lock.try_lock_exclusive()?;
    fs::remove_file(path)?;
    let supervisor_pid = std::process::id();
    let mut job = JobRecord {
        id: launch.id.clone(),
        pid: None,
        supervisor_pid,
        supervisor_identity: platform::identity(supervisor_pid)?
            .ok_or_else(|| io::Error::other("cannot identify the running supervisor"))?,
        command: launch.request.command.clone(),
        cwd: launch.request.cwd.clone(),
        machine: launch.request.machine.clone(),
        started_at: record::now(),
        ended_at: None,
        output_path: launch.output.clone(),
        state: JobState::Running,
        exit_code: None,
        error: None,
        hook_error: None,
    };
    let result = run_command(&launch, &mut job);
    job.ended_at = Some(record::now());
    match result {
        Ok(code) => {
            job.state = JobState::Exited;
            job.exit_code = Some(code);
        }
        Err(error) => {
            job.state = JobState::Failed;
            job.error = Some(error.to_string());
        }
    }
    record::write(&launch.record, &job)?;
    if let Some(hook) = &launch.request.hook
        && let Err(error) = run_hook(&launch, hook)
    {
        job.hook_error = Some(error.to_string());
        record::write(&launch.record, &job)?;
        return Err(error);
    }
    Ok(())
}

fn bash_command(launch: &Launch) -> io::Result<Command> {
    let environment = crate::session::SessionEnvironment::capture().map_err(io::Error::other)?;
    let bash = crate::shell::bash::BashLocator::default()
        .resolve(&environment)
        .map_err(io::Error::other)?;
    let mut command = crate::process_policy::noninteractive_command(&bash);
    command
        .args(["--noprofile", "--norc"])
        .current_dir(&launch.request.cwd)
        .stdin(Stdio::null());
    // Apply overrides first; only the graphics capability and executable search prefix are
    // owned by run. No locale, editor, pager, or buffering policy rewrites caller values.
    command
        .envs(&launch.request.env)
        .env("AGENT_GRAPHICS", "kitty,iterm2");
    let mut directories = vec![
        launch
            .executable
            .parent()
            .ok_or_else(|| io::Error::other("executable has no parent"))?
            .to_path_buf(),
    ];
    #[cfg(windows)]
    {
        let usr_bin = bash
            .parent()
            .ok_or_else(|| io::Error::other("bash has no parent"))?;
        directories.push(usr_bin.to_path_buf());
        if let Some(root) = usr_bin.parent().and_then(Path::parent) {
            for arch in ["mingw64", "mingw32", "clangarm64"] {
                let bin = root.join(arch).join("bin");
                if bin.is_dir() {
                    directories.push(bin);
                }
            }
        }
    }
    let supplied_path = launch
        .request
        .env
        .iter()
        .find(|(key, _)| {
            if cfg!(windows) {
                key.eq_ignore_ascii_case("PATH")
            } else {
                key.as_str() == "PATH"
            }
        })
        .map(|(_, value)| std::ffi::OsString::from(value));
    if let Some(path) = supplied_path.or_else(|| std::env::var_os("PATH")) {
        directories.extend(std::env::split_paths(&path));
    }
    command.env(
        "PATH",
        std::env::join_paths(directories).map_err(io::Error::other)?,
    );
    Ok(command)
}

fn script(text: &str) -> io::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .prefix("fastctx-run-")
        .suffix(".sh")
        .tempfile()?;
    file.write_all(text.as_bytes())?;
    file.flush()?;
    Ok(file)
}

fn script_path(path: &Path) -> std::ffi::OsString {
    #[cfg(windows)]
    {
        path.to_string_lossy().replace('\\', "/").into()
    }
    #[cfg(not(windows))]
    {
        path.as_os_str().to_owned()
    }
}

fn run_command(launch: &Launch, job: &mut JobRecord) -> io::Result<i32> {
    let source = script(&launch.request.command)?;
    let (mut reader, writer) = io::pipe()?;
    let mut command = bash_command(launch)?;
    command
        .arg(script_path(source.path()))
        .stdout(writer.try_clone()?)
        .stderr(writer);
    platform::configure_bash(&mut command);
    let mut child = platform::spawn(&mut command)?;
    drop(command);
    job.pid = Some(child.id());
    record::write(&launch.record, job)?;
    let mut output = OpenOptions::new().append(true).open(&launch.output)?;
    // One pipe orders stdout and stderr. Copying never decodes, normalizes, or retains output.
    // EOF includes inherited writers, so background descendants can finish their output too.
    let copied = io::copy(&mut reader, &mut output);
    if let Err(error) = &copied {
        job.error = Some(format!(
            "output capture failed: {error}; subsequent output cannot be saved"
        ));
        record::write(&launch.record, job)?;
        // A full disk must not leave the writer blocked behind an unread pipe. The loss is
        // recorded before draining; the command keeps its own lifetime and no retry occurs.
        io::copy(&mut reader, &mut io::sink())?;
    }
    let synced = output.sync_all();
    let status = child.wait()?;
    job.exit_code = Some(platform::exit_code(status));
    copied?;
    synced?;
    Ok(platform::exit_code(status))
}

fn run_hook(launch: &Launch, hook: &str) -> io::Result<()> {
    let source = script(&format!("{hook} \"$1\"\n"))?;
    let path = launch.record.with_extension("hook.log");
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    let mut command = bash_command(launch)?;
    let status = command
        .arg(script_path(source.path()))
        .arg(&launch.record)
        .stdout(output.try_clone()?)
        .stderr(output)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "completion hook exited {}; output in {}",
            platform::exit_code(status),
            path.display()
        )));
    }
    Ok(())
}

/// Reconciles abandoned running records without sending notifications or restarting commands.
/// Live supervisors hold the record lock through final publication and their one hook attempt.
pub fn reconcile(directory: &Path) -> Result<Vec<JobRecord>, String> {
    reconcile_inner(directory).map_err(|error| error.to_string())
}

fn reconcile_inner(directory: &Path) -> io::Result<Vec<JobRecord>> {
    let mut changed = Vec::new();
    let entries = match fs::read_dir(directory.join("jobs")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(changed),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let lock = record::lock(&path)?;
        match lock.try_lock_exclusive() {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
        let mut job = record::read(&path)?;
        if job.state == JobState::Running
            && platform::identity(job.supervisor_pid)?.as_deref() != Some(&job.supervisor_identity)
        {
            job.state = JobState::Interrupted;
            job.ended_at = Some(record::now());
            job.error = Some("supervisor disappeared before recording completion; command exit status is unknown".into());
            record::write(&path, &job)?;
            changed.push(job);
        }
    }
    Ok(changed)
}
