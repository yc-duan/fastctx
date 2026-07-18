//! Detached-supervisor launch, output capture, tree ownership, and kill acknowledgement.

use super::identity::{identity_is_alive, process_identity, supervisor_isolation_warning};
use super::model::{
    CAPTURE_ERROR_FILE, CaptureErrorRecord, EXIT_FILE, ExitRecord, JOB_SCHEMA_VERSION, JobMeta,
    LaunchSpec, META_FILE, TerminationKind,
};
use super::spool::SpoolWriter;
use super::store::{
    clear_kill_request, kill_requested, read_json, request_kill, unix_nanos_now, utc_now,
    write_atomic_json,
};
use crate::paths::display_path;
use crate::shell::normalize::{NormalizedLine, StreamNormalizer};
use crate::shell::process::{exit_code, spawn_bash};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_POLL: Duration = Duration::from_millis(20);
const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

enum OutputEvent {
    Line(NormalizedLine),
    Failed(String),
    Finished,
}

/// Launches a detached supervisor and waits only until its process tree and immutable metadata exist.
pub(crate) fn launch_supervisor(
    executable: &std::path::Path,
    spec: &LaunchSpec,
) -> Result<(), String> {
    let encoded = serde_json::to_vec(spec)
        .map_err(|error| format!("Cannot encode the background job launch request: {error}"))?;
    let mut child = spawn_detached(executable)?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        "Cannot start the background job supervisor: its launch pipe was not created.".to_string()
    })?;
    stdin.write_all(&encoded).and_then(|()| stdin.flush()).map_err(|error| {
        let _ = child.kill();
        let _ = child.wait();
        format!("Cannot start the background job supervisor: cannot send its launch request ({error}).")
    })?;
    drop(stdin);
    let stdout = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        "Cannot start the background job supervisor: its readiness pipe was not created."
            .to_string()
    })?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout)
            .read_line(&mut line)
            .map(|_| line)
            .map_err(|error| error.to_string());
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(STARTUP_TIMEOUT) {
        Ok(Ok(line)) if line.trim_end() == "READY" => Ok(()),
        Ok(Ok(line)) if line.starts_with("ERROR\t") => {
            let encoded = line["ERROR\t".len()..].trim_end();
            let message =
                serde_json::from_str::<String>(encoded).unwrap_or_else(|_| encoded.to_string());
            let _ = child.wait();
            Err(message)
        }
        Ok(Ok(line)) => {
            abort_failed_launch(&mut child, spec);
            Err(format!(
                "Cannot start the background job supervisor: invalid readiness response {:?}.",
                line.trim_end()
            ))
        }
        Ok(Err(error)) => {
            abort_failed_launch(&mut child, spec);
            Err(format!(
                "Cannot start the background job supervisor: cannot read its readiness response ({error})."
            ))
        }
        Err(_) => {
            abort_failed_launch(&mut child, spec);
            Err("Cannot start the background job supervisor: it did not become ready within 10 seconds.".to_string())
        }
    }
}

fn spawn_detached(executable: &std::path::Path) -> Result<Child, String> {
    #[cfg(unix)]
    {
        Command::new(executable)
            .arg("job-bootstrap")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Cannot start the background job supervisor: {error}."))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        };

        fn command(executable: &std::path::Path, flags: u32) -> Command {
            let mut command = Command::new(executable);
            command
                .arg("job-host")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(crate::process_policy::noninteractive_creation_flags(flags));
            command
        }

        let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        match command(executable, detached | CREATE_BREAKAWAY_FROM_JOB).spawn() {
            Ok(child) => Ok(child),
            Err(error) if error.raw_os_error() == Some(5) => command(executable, detached)
                .spawn()
                .map_err(|error| format!("Cannot start the background job supervisor: {error}.")),
            Err(error) => Err(format!(
                "Cannot start the background job supervisor: {error}."
            )),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Command::new(executable)
            .arg("job-host")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Cannot start the background job supervisor: {error}."))
    }
}

fn abort_failed_launch(child: &mut Child, spec: &LaunchSpec) {
    let _ = child.kill();
    let _ = child.wait();
    let meta = read_json::<JobMeta>(&spec.job_dir.join(META_FILE), "job metadata")
        .ok()
        .flatten();
    let Some(meta) = meta else {
        return;
    };
    if !identity_is_alive(&meta.supervisor) {
        return;
    }
    let record = super::model::JobRecord {
        id: spec.job_id.clone(),
        directory: spec.job_dir.clone(),
        meta,
        status: super::model::JobStatus::Running,
        ended_sort_key: std::time::UNIX_EPOCH,
    };
    let _ = request_kill(&record);
    let deadline = Instant::now() + Duration::from_secs(6);
    while identity_is_alive(&record.meta.supervisor) && Instant::now() < deadline {
        std::thread::sleep(CONTROL_POLL);
    }
}

/// Unix intermediate process: creates a new session, starts the final host, and exits immediately.
#[cfg(unix)]
pub(crate) fn run_bootstrap() -> Result<(), String> {
    let mut launch = Vec::new();
    std::io::stdin()
        .read_to_end(&mut launch)
        .map_err(|error| format!("Cannot read the background job launch request: {error}"))?;
    if unsafe { libc::setsid() } < 0 {
        return Err(format!(
            "Cannot detach the background job supervisor session: {}",
            std::io::Error::last_os_error()
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the background job supervisor binary: {error}"))?;
    let mut child = Command::new(executable)
        .arg("job-host")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot start the background job supervisor: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Cannot create the final supervisor launch pipe.".to_string())?;
    stdin
        .write_all(&launch)
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("Cannot forward the background job launch request: {error}"))?;
    Ok(())
}

/// Final detached process entry point. Startup failures use the inherited readiness pipe.
pub(crate) fn run_job_host() -> Result<(), String> {
    let result = (|| {
        let mut source = Vec::new();
        std::io::stdin()
            .read_to_end(&mut source)
            .map_err(|error| format!("Cannot read the background job launch request: {error}"))?;
        let spec: LaunchSpec = serde_json::from_slice(&source)
            .map_err(|error| format!("Cannot parse the background job launch request: {error}"))?;
        supervise(spec)
    })();
    if let Err(error) = &result {
        write_startup_error(error);
    }
    result
}

fn supervise(spec: LaunchSpec) -> Result<(), String> {
    let mut process = spawn_bash(&spec.bash, &spec.command, &spec.cwd, spec.login_shell)
        .map_err(|error| format!("Cannot start the command: {error}."))?;
    let mut watchdog = match WatchdogGuard::arm(process.id()) {
        Ok(watchdog) => watchdog,
        Err(error) => {
            let _ = process.terminate_tree();
            return Err(error);
        }
    };
    let supervisor = process_identity(std::process::id()).ok_or_else(|| {
        let _ = process.terminate_tree();
        watchdog.disarm();
        "Cannot identify the background job supervisor process. The command was terminated."
            .to_string()
    })?;
    let meta = JobMeta {
        schema_version: JOB_SCHEMA_VERSION,
        command: spec.command,
        cwd: display_path(&spec.cwd),
        login_shell: spec.login_shell,
        supervisor,
        origin: spec.origin,
        started_at: utc_now(),
        started_at_unix_nanos: unix_nanos_now(),
        isolation_warning: supervisor_isolation_warning(),
    };
    if let Err(error) = write_atomic_json(&spec.job_dir.join(META_FILE), &meta) {
        let _ = process.terminate_tree();
        watchdog.disarm();
        return Err(format!(
            "Cannot start the background job supervisor: {error}"
        ));
    }

    let output = process.take_output();
    let (events, reader) = spawn_reader(output);
    if let Err(error) = write_ready() {
        match process.terminate_tree() {
            Ok(_) => watchdog.disarm(),
            Err(termination_error) => {
                return Err(format!(
                    "{error} Terminating the command tree also failed: {termination_error}."
                ));
            }
        }
        return Err(error);
    }

    let mut spool = Some(SpoolWriter::new(&spec.job_dir));
    let mut total_lines = 0_u64;
    let mut had_loss = false;
    let mut capture_error = None;
    let (status, termination) = loop {
        drain_output_events(
            &events,
            &spec.job_dir,
            &mut spool,
            &mut total_lines,
            &mut had_loss,
            &mut capture_error,
        );
        if let Some(writer) = spool.as_mut()
            && let Err(error) = writer.flush_if_idle()
        {
            capture_failure(
                &spec.job_dir,
                &mut spool,
                total_lines,
                error,
                &mut had_loss,
                &mut capture_error,
            );
        }

        match process.try_wait() {
            Ok(Some(status)) => {
                let code = exit_code(status);
                process.terminate_tree().map_err(|error| {
                    format!(
                        "The command exited, but its descendant process tree could not be terminated: {error}"
                    )
                })?;
                break (code, TerminationKind::Exited);
            }
            Ok(None) => {}
            Err(error) => {
                capture_failure(
                    &spec.job_dir,
                    &mut spool,
                    total_lines,
                    format!("cannot monitor the command process: {error}"),
                    &mut had_loss,
                    &mut capture_error,
                );
                let code = exit_code(process.terminate_tree().map_err(|termination_error| {
                    format!(
                        "Cannot monitor the command process ({error}) or terminate its process tree ({termination_error})."
                    )
                })?);
                break (code, TerminationKind::Exited);
            }
        }

        if kill_requested(&spec.job_dir) {
            match process.try_wait() {
                Ok(Some(status)) => {
                    clear_kill_request(&spec.job_dir);
                    let _ = process.terminate_tree();
                    break (exit_code(status), TerminationKind::Exited);
                }
                Ok(None) => {
                    if let Ok(status) = process.terminate_tree() {
                        clear_kill_request(&spec.job_dir);
                        break (exit_code(status), TerminationKind::Killed);
                    }
                }
                Err(error) => {
                    capture_failure(
                        &spec.job_dir,
                        &mut spool,
                        total_lines,
                        format!("cannot inspect the command before termination: {error}"),
                        &mut had_loss,
                        &mut capture_error,
                    );
                    if let Ok(status) = process.terminate_tree() {
                        clear_kill_request(&spec.job_dir);
                        break (exit_code(status), TerminationKind::Killed);
                    }
                }
            }
        }
        std::thread::sleep(CONTROL_POLL);
    };

    let deadline = Instant::now() + READER_JOIN_TIMEOUT;
    while Instant::now() < deadline {
        drain_output_events(
            &events,
            &spec.job_dir,
            &mut spool,
            &mut total_lines,
            &mut had_loss,
            &mut capture_error,
        );
        if reader.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if reader.is_finished() {
        let _ = reader.join();
    } else {
        capture_failure(
            &spec.job_dir,
            &mut spool,
            total_lines,
            "the output reader did not finish within 2 seconds after the process tree exited"
                .to_string(),
            &mut had_loss,
            &mut capture_error,
        );
    }
    drain_output_events(
        &events,
        &spec.job_dir,
        &mut spool,
        &mut total_lines,
        &mut had_loss,
        &mut capture_error,
    );
    if let Some(writer) = spool.as_mut() {
        total_lines = writer.total_lines();
        had_loss |= writer.had_loss();
        if let Err(error) = writer.finish() {
            capture_failure(
                &spec.job_dir,
                &mut spool,
                total_lines,
                error,
                &mut had_loss,
                &mut capture_error,
            );
        }
    }
    watchdog.disarm();
    let exit = ExitRecord {
        exit_code: status,
        total_lines,
        had_loss: had_loss || capture_error.is_some(),
        ended_at: utc_now(),
        ended_at_unix_nanos: unix_nanos_now(),
        termination,
        capture_error,
    };
    write_atomic_json(&spec.job_dir.join(EXIT_FILE), &exit)
        .map_err(|error| format!("Cannot write the background job exit record: {error}"))
}

fn spawn_reader(
    mut output: impl Read + Send + 'static,
) -> (mpsc::Receiver<OutputEvent>, std::thread::JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut normalizer = StreamNormalizer::new();
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let read = match output.read(&mut buffer) {
                Ok(read) => read,
                Err(error) => {
                    let _ = sender.send(OutputEvent::Failed(format!(
                        "cannot read the merged command output: {error}"
                    )));
                    return;
                }
            };
            if read == 0 {
                break;
            }
            let mut lines = Vec::new();
            normalizer.push(&buffer[..read], &mut lines);
            for line in lines {
                if sender.send(OutputEvent::Line(line)).is_err() {
                    return;
                }
            }
        }
        let mut lines = Vec::new();
        normalizer.finish(&mut lines);
        for line in lines {
            if sender.send(OutputEvent::Line(line)).is_err() {
                return;
            }
        }
        let _ = sender.send(OutputEvent::Finished);
    });
    (receiver, reader)
}

fn drain_output_events(
    events: &mpsc::Receiver<OutputEvent>,
    directory: &std::path::Path,
    spool: &mut Option<SpoolWriter>,
    total_lines: &mut u64,
    had_loss: &mut bool,
    capture_error: &mut Option<CaptureErrorRecord>,
) {
    while let Ok(event) = events.try_recv() {
        match event {
            OutputEvent::Line(line) => {
                if let Some(writer) = spool.as_mut() {
                    match writer.append(line) {
                        Ok(seq) => {
                            *total_lines = seq;
                            *had_loss |= writer.had_loss();
                        }
                        Err(error) => capture_failure(
                            directory,
                            spool,
                            *total_lines,
                            error,
                            had_loss,
                            capture_error,
                        ),
                    }
                }
            }
            OutputEvent::Failed(error) => capture_failure(
                directory,
                spool,
                *total_lines,
                error,
                had_loss,
                capture_error,
            ),
            OutputEvent::Finished => {}
        }
    }
}

fn capture_failure(
    directory: &std::path::Path,
    spool: &mut Option<SpoolWriter>,
    after_seq: u64,
    reason: String,
    had_loss: &mut bool,
    capture_error: &mut Option<CaptureErrorRecord>,
) {
    if let Some(writer) = spool.as_mut() {
        let _ = writer.finish();
    }
    *spool = None;
    *had_loss = true;
    if capture_error.is_some() {
        return;
    }
    let record = CaptureErrorRecord { after_seq, reason };
    capture_error.clone_from(&Some(record.clone()));
    let _ = write_atomic_json(&directory.join(CAPTURE_ERROR_FILE), &record);
}

fn write_ready() -> Result<(), String> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(b"READY\n")
        .and_then(|()| stdout.flush())
        .map_err(|error| {
            format!(
                "Cannot acknowledge background job supervisor startup: {error}. The command was terminated."
            )
        })
}

pub(super) fn write_startup_error(error: &str) {
    let encoded =
        serde_json::to_string(error).unwrap_or_else(|_| "\"unknown startup error\"".to_string());
    let _ = writeln!(std::io::stdout().lock(), "ERROR\t{encoded}");
}

#[cfg(unix)]
struct WatchdogGuard {
    writer: Option<std::io::PipeWriter>,
}

#[cfg(unix)]
impl WatchdogGuard {
    fn arm(process_pid: u32) -> Result<Self, String> {
        use std::os::unix::process::CommandExt;

        let identity = process_identity(process_pid).ok_or_else(|| {
            "Cannot identify the background command process for orphan protection.".to_string()
        })?;
        let (reader, writer) = std::io::pipe().map_err(|error| {
            format!("Cannot create the background command orphan-protection pipe: {error}")
        })?;
        let executable = std::env::current_exe()
            .map_err(|error| format!("Cannot locate the orphan-protection helper: {error}"))?;
        let mut command = Command::new(executable);
        command
            .arg("job-watch")
            .arg(identity.pid.to_string())
            .arg(&identity.started)
            .stdin(Stdio::from(reader))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn().map_err(|error| {
            format!("Cannot start the background command orphan-protection helper: {error}")
        })?;
        Ok(Self {
            writer: Some(writer),
        })
    }

    fn disarm(&mut self) {
        if let Some(mut writer) = self.writer.take() {
            let _ = writer.write_all(b"D");
        }
    }
}

#[cfg(not(unix))]
struct WatchdogGuard;

#[cfg(not(unix))]
impl WatchdogGuard {
    fn arm(_process_pid: u32) -> Result<Self, String> {
        Ok(Self)
    }

    fn disarm(&mut self) {}
}

/// Unix helper waits for a clean-disarm byte; EOF means the host died and the command group must die.
#[cfg(unix)]
pub(crate) fn run_watchdog(pid: u32, started: String) -> Result<(), String> {
    let mut byte = [0_u8; 1];
    let read = std::io::stdin()
        .read(&mut byte)
        .map_err(|error| format!("Cannot read the orphan-protection pipe: {error}"))?;
    if read == 0 {
        let identity = super::model::ProcessIdentity { pid, started };
        if identity_is_alive(&identity) {
            let process_group =
                i32::try_from(pid).map_err(|_| format!("Cannot address process group {pid}."))?;
            let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(format!(
                        "Cannot terminate orphaned process group {pid}: {error}"
                    ));
                }
            }
        }
    }
    Ok(())
}
