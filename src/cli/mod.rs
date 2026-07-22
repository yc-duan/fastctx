//! Command-line parsing, dual-mode TTY dispatch, and non-interactive control commands.

use crate::control::apply::{
    ApplyOptions, UnapplyOptions, commit_apply, commit_unapply, plan_apply, plan_unapply,
};
use crate::control::doctor;
use crate::control::i18n::{ALL_LANGUAGES, Language};
use crate::control::paths::ControlPaths;
use crate::control::settings::{self, Tier};
use crate::server::{FastCtxServer, ServerOptions};
use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Dual-mode fastctx entry point.
#[derive(Debug, Parser)]
#[command(
    name = "fastctx",
    version,
    about = "FastCtx — fast, context-efficient repository tools for AI agents.",
    long_about = "Run in a terminal for the control UI, or connect stdin/stdout pipes for the MCP server."
)]
pub struct Cli {
    /// Explicit control command; omission selects automatically from TTY state.
    #[command(subcommand)]
    command: Option<Command>,
}

/// All scriptable commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Force the stdio MCP server.
    Serve {
        /// Publish the five optional shell tools.
        #[arg(long)]
        enable_shell: bool,
        /// Deprecated compatibility flag; replace is always published.
        #[arg(long, hide = true)]
        enable_edit: bool,
    },
    /// Force the full-screen control terminal.
    Ui,
    /// Preview and apply the ChatGPT/Codex integration.
    Apply {
        /// Codex profile directory; overrides CODEX_HOME and the default.
        #[arg(long, value_name = "PATH")]
        codex_home: Option<PathBuf>,
        /// Host output tier; defaults to the saved selection.
        #[arg(long, value_enum)]
        tier: Option<Tier>,
        /// Accept the preview and any shared-limit conflict.
        #[arg(long)]
        yes: bool,
    },
    /// Preview and remove the ChatGPT/Codex integration.
    Unapply {
        /// Codex profile directory; overrides CODEX_HOME and the default.
        #[arg(long, value_name = "PATH")]
        codex_home: Option<PathBuf>,
        /// Accept the preview without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Run all local integration checks.
    #[command(visible_alias = "doctor")]
    Status {
        /// Codex profile directory; overrides CODEX_HOME and the default.
        #[arg(long, value_name = "PATH")]
        codex_home: Option<PathBuf>,
    },
    /// Set the TUI language.
    Lang {
        /// One of the 17 supported language codes.
        code: String,
    },
    /// List or terminate persistent background jobs.
    Jobs {
        #[command(subcommand)]
        command: Option<JobsCommand>,
    },
    /// Internal Unix detach bootstrap.
    #[cfg(unix)]
    #[command(hide = true)]
    JobBootstrap,
    /// Internal detached background-job supervisor.
    #[command(hide = true)]
    JobHost,
    /// Internal Unix process-group orphan guard.
    #[cfg(unix)]
    #[command(hide = true)]
    JobWatch { pid: u32, started: String },
    /// Internal updater helper copied outside the active installation.
    #[command(hide = true)]
    UpdateHelper {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        parent_pid: u32,
    },
}

/// Scriptable background-job operations.
#[derive(Debug, Subcommand)]
enum JobsCommand {
    /// Kill one background job's whole process tree.
    Kill {
        /// Job id returned by run_background or shown by `fastctx jobs`.
        job_id: String,
    },
}

/// Parses the current process arguments and executes the selected command.
pub async fn run() -> Result<ExitCode, String> {
    if let Some(request) = std::env::var_os(crate::update::UPDATE_FINALIZE_ENV) {
        unsafe {
            std::env::remove_var(crate::update::UPDATE_FINALIZE_ENV);
        }
        require_tty()?;
        let paths = ControlPaths::discover()?;
        let notice = crate::update::finalize_update(&paths, &PathBuf::from(request))?;
        return run_tui(
            paths,
            crate::update::StartupUpdate::None,
            Some(notice),
            None,
        );
    }
    if let Some(error) = std::env::var_os(crate::update::UPDATE_FAILURE_ENV) {
        unsafe {
            std::env::remove_var(crate::update::UPDATE_FAILURE_ENV);
        }
        require_tty()?;
        let paths = ControlPaths::discover()?;
        return run_tui(
            paths,
            crate::update::StartupUpdate::InstallFailed(error.to_string_lossy().into_owned()),
            None,
            None,
        );
    }
    run_cli(Cli::parse()).await
}

async fn run_cli(cli: Cli) -> Result<ExitCode, String> {
    let implicit_tui =
        cli.command.is_none() && io::stdin().is_terminal() && io::stdout().is_terminal();
    match cli.command {
        Some(Command::Serve {
            enable_shell,
            enable_edit: _,
        }) => run_server_with_options(ServerOptions { enable_shell }).await,
        Some(Command::Ui) => {
            require_tty()?;
            let paths = ControlPaths::discover()?;
            run_tui_with_check(paths)
        }
        Some(Command::Apply {
            codex_home,
            tier,
            yes,
        }) => run_apply(codex_home, tier, yes),
        Some(Command::Unapply { codex_home, yes }) => run_unapply(codex_home, yes),
        Some(Command::Status { codex_home }) => run_status(codex_home),
        Some(Command::Lang { code }) => run_lang(&code),
        Some(Command::Jobs { command }) => run_jobs(command),
        #[cfg(unix)]
        Some(Command::JobBootstrap) => {
            crate::shell::jobs::run_bootstrap_entry()?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::JobHost) => {
            crate::shell::jobs::run_host_entry()?;
            Ok(ExitCode::SUCCESS)
        }
        #[cfg(unix)]
        Some(Command::JobWatch { pid, started }) => {
            crate::shell::jobs::run_watchdog_entry(pid, started)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::UpdateHelper {
            request,
            parent_pid,
        }) => {
            let paths = ControlPaths::discover()?;
            crate::update::run_update_helper(&paths, &request, parent_pid)?;
            Ok(ExitCode::SUCCESS)
        }
        None if implicit_tui => {
            let paths = ControlPaths::discover()?;
            run_tui_with_check(paths)
        }
        None => run_server().await,
    }
}

fn run_tui_with_check(paths: ControlPaths) -> Result<ExitCode, String> {
    crate::update::cleanup_replaced_binaries(&paths);
    let startup_check = crate::update::spawn_update_check(paths.clone(), false);
    run_tui(
        paths,
        crate::update::StartupUpdate::None,
        None,
        Some(startup_check),
    )
}

fn run_tui(
    paths: ControlPaths,
    startup_update: crate::update::StartupUpdate,
    startup_notice: Option<crate::update::FinalizeNotice>,
    startup_check: Option<std::sync::mpsc::Receiver<crate::update::StartupUpdate>>,
) -> Result<ExitCode, String> {
    match crate::tui::run(paths.clone(), startup_update, startup_notice, startup_check)? {
        crate::tui::TuiOutcome::Exit => Ok(ExitCode::SUCCESS),
        crate::tui::TuiOutcome::Update(plan) => {
            let current_executable = std::env::current_exe()
                .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?;
            match crate::update::begin_update(&paths, *plan, &current_executable)? {
                crate::update::UpdateStart::Completed => Ok(ExitCode::SUCCESS),
                crate::update::UpdateStart::NpmLauncherWait => {
                    Ok(ExitCode::from(crate::update::NPM_LAUNCHER_WAIT_EXIT_CODE))
                }
            }
        }
    }
}

fn run_jobs(command: Option<JobsCommand>) -> Result<ExitCode, String> {
    let paths = ControlPaths::discover()?;
    match command {
        None => {
            let jobs = crate::shell::jobs::running_summaries(&paths)?;
            if jobs.is_empty() {
                println!("No running jobs.");
                return Ok(ExitCode::SUCCESS);
            }
            for (index, job) in jobs.iter().enumerate() {
                if index > 0 {
                    println!();
                }
                println!("{}  started {}", job.id, job.started_at);
                println!(
                    "  {} — {}",
                    one_line(&job.cwd),
                    truncate_command(&job.command)
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Some(JobsCommand::Kill { job_id }) => {
            println!("{}", crate::shell::jobs::kill_for_control(&paths, &job_id)?);
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn truncate_command(command: &str) -> String {
    let command = one_line(command);
    let mut characters = command.chars();
    let prefix = characters.by_ref().take(120).collect::<String>();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn one_line(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

/// Forces stdio MCP server startup for reuse by the entry point and doctor.
pub async fn run_server() -> Result<ExitCode, String> {
    run_server_with_options(ServerOptions::default()).await
}

/// Starts the single server with the requested optional tool groups.
pub async fn run_server_with_options(options: ServerOptions) -> Result<ExitCode, String> {
    let parent = crate::process_identity::parent_identity_from_environment()?;
    let stdin = crate::stdio_transport::DetachedStdin::start()?;
    run_server_with_io(options, parent, stdin, tokio::io::stdout()).await
}

async fn run_server_with_io<W>(
    options: ServerOptions,
    parent: Option<Option<crate::process_identity::ProcessIdentity>>,
    stdin: crate::stdio_transport::DetachedStdin,
    stdout: W,
) -> Result<ExitCode, String>
where
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let stdin_eof = stdin.eof_token();
    let stdin_read_error = stdin.read_error_receiver();
    let stdin_read_error_wait = wait_for_stdin_read_error(stdin_read_error.clone());
    tokio::pin!(stdin_read_error_wait);
    let service = match FastCtxServer::with_options(options)
        .serve((stdin, stdout))
        .await
    {
        Ok(service) => service,
        Err(error) => {
            return Err(stdin_read_error
                .borrow()
                .clone()
                .unwrap_or_else(|| format!("Cannot start the MCP server: {error}")));
        }
    };
    let cancellation = service.cancellation_token();
    let mut waiting = tokio::spawn(service.waiting());

    let monitor_stop = Arc::new(AtomicBool::new(false));
    let (parent_exit, monitor) = match parent {
        None => (None, None),
        Some(None) => {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let _ = sender.send(());
            (Some(receiver), None)
        }
        Some(Some(identity)) => {
            let stop = Arc::clone(&monitor_stop);
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let monitor = tokio::task::spawn_blocking(move || {
                if crate::process_identity::wait_for_identity_exit_until(&identity, &stop) {
                    let _ = sender.send(());
                }
            });
            (Some(receiver), Some(monitor))
        }
    };
    let parent_exit_future = async move {
        match parent_exit {
            Some(receiver) => match receiver.await {
                Ok(()) => {}
                // Monitor failure is not proof that the parent exited. Keep the server alive until
                // stdio EOF or another explicit shutdown signal instead of killing a live session.
                Err(_) => std::future::pending::<()>().await,
            },
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(parent_exit_future);

    let result = tokio::select! {
        result = &mut waiting => match stdin_read_error.borrow().clone() {
            Some(error) => Err(error),
            None => flatten_service_wait(result),
        },
        () = stdin_eof.cancelled() => {
            cancellation.cancel();
            wait_for_bounded_service_shutdown(&mut waiting).await
        }
        error = &mut stdin_read_error_wait => {
            cancellation.cancel();
            match wait_for_bounded_service_shutdown(&mut waiting).await {
                Ok(()) => Err(error),
                Err(shutdown_error) => Err(format!("{error}; {shutdown_error}")),
            }
        }
        () = &mut parent_exit_future => {
            cancellation.cancel();
            wait_for_bounded_service_shutdown(&mut waiting).await
        }
        () = wait_for_server_termination_signal() => {
            cancellation.cancel();
            wait_for_bounded_service_shutdown(&mut waiting).await
        }
    };
    monitor_stop.store(true, Ordering::Release);
    if let Some(monitor) = monitor {
        let _ = monitor.await;
    }
    result?;
    Ok(ExitCode::SUCCESS)
}

const SERVER_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

async fn wait_for_stdin_read_error(
    mut receiver: tokio::sync::watch::Receiver<Option<String>>,
) -> String {
    loop {
        if let Some(error) = receiver.borrow().clone() {
            return error;
        }
        if receiver.changed().await.is_err() {
            return std::future::pending::<String>().await;
        }
    }
}

type ServiceWaitResult =
    Result<Result<rmcp::service::QuitReason, tokio::task::JoinError>, tokio::task::JoinError>;

fn flatten_service_wait(result: ServiceWaitResult) -> Result<(), String> {
    match result {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) | Err(error) => {
            Err(format!("The MCP server stopped with an error: {error}"))
        }
    }
}

async fn wait_for_bounded_service_shutdown(
    waiting: &mut tokio::task::JoinHandle<
        Result<rmcp::service::QuitReason, tokio::task::JoinError>,
    >,
) -> Result<(), String> {
    match tokio::time::timeout(SERVER_SHUTDOWN_TIMEOUT, &mut *waiting).await {
        Ok(result) => flatten_service_wait(result),
        Err(_) => {
            // rmcp can keep waiting while an inherited stdin handle remains open; bounding only
            // this server waiter still leaves detached background-job supervisors independent.
            waiting.abort();
            let _ = waiting.await;
            Ok(())
        }
    }
}

#[cfg(unix)]
async fn wait_for_server_termination_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        return std::future::pending::<()>().await;
    };
    let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
        return std::future::pending::<()>().await;
    };
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_server_termination_signal() {
    std::future::pending::<()>().await
}

fn run_apply(
    codex_home: Option<PathBuf>,
    tier: Option<Tier>,
    yes: bool,
) -> Result<ExitCode, String> {
    let paths = ControlPaths::discover_with_codex_home(codex_home)?;
    let saved = settings::load(&paths)?;
    let plan = plan_apply(
        &paths,
        ApplyOptions {
            tier: tier.unwrap_or(saved.tier),
            tool_budgets: saved.tool_budgets,
            fastshell_enabled: saved.fastshell.enabled,
            current_executable: std::env::current_exe()
                .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?,
        },
    )?;
    print_preview("Apply preview", plan.preview());
    if plan.is_empty() {
        let receipt = commit_apply(plan, true)?;
        print_receipt(&receipt);
        return Ok(ExitCode::SUCCESS);
    }
    if !yes {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(
                "Apply requires confirmation in a terminal. Re-run with --yes after reviewing the preview."
                    .to_string(),
            );
        }
        if let Some(conflict) = plan.token_limit_conflict() {
            println!(
                "Shared setting warning: tool_output_token_limit is {}, requested {}.",
                conflict.current, conflict.requested
            );
            if !confirm("Change this shared ChatGPT/Codex setting?")? {
                println!("Cancelled. No files were written.");
                return Ok(ExitCode::SUCCESS);
            }
        }
        if !confirm("Apply these changes?")? {
            println!("Cancelled. No files were written.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let receipt = commit_apply(plan, yes || io::stdin().is_terminal())?;
    print_receipt(&receipt);
    Ok(ExitCode::SUCCESS)
}

fn run_unapply(codex_home: Option<PathBuf>, yes: bool) -> Result<ExitCode, String> {
    let paths = ControlPaths::discover_with_codex_home(codex_home)?;
    let plan = plan_unapply(
        &paths,
        UnapplyOptions {
            current_executable: std::env::current_exe()
                .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?,
        },
    )?;
    print_preview("Unapply preview", plan.preview());
    if plan.running_jobs() > 0 {
        println!(
            "  Stop      {} running background {} before removal",
            plan.running_jobs(),
            if plan.running_jobs() == 1 {
                "job"
            } else {
                "jobs"
            }
        );
    }
    println!(
        "  {:<9} {} running FastCtx {} (open ChatGPT/Codex sessions will lose FastCtx tools)",
        if plan.running_processes() == 0 {
            "Unchanged"
        } else {
            "Stop"
        },
        plan.running_processes(),
        if plan.running_processes() == 1 {
            "process"
        } else {
            "processes"
        }
    );
    if !yes && !plan.is_empty() {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(
                "Unapply requires confirmation in a terminal. Re-run with --yes after reviewing the preview."
                    .to_string(),
            );
        }
        if !confirm("Remove fastctx from ChatGPT/Codex?")? {
            println!("Cancelled. No files were written.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let receipt = commit_unapply(plan)?;
    print_receipt(&receipt);
    Ok(ExitCode::SUCCESS)
}

fn run_status(codex_home: Option<PathBuf>) -> Result<ExitCode, String> {
    use crate::control::doctor::DoctorCheckStatus;

    let paths = ControlPaths::discover_with_codex_home(codex_home)?;
    let report = doctor::run(&paths);
    for check in &report.checks {
        let label = match check.status {
            DoctorCheckStatus::Pass => "PASS",
            DoctorCheckStatus::Info => "INFO",
            DoctorCheckStatus::Fail => "FAIL",
        };
        println!("[{}] {}: {}", label, check.name, check.detail);
        if let Some(remedy) = &check.remedy {
            println!("       Next: {remedy}");
        }
    }
    Ok(if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn run_lang(code: &str) -> Result<ExitCode, String> {
    let language = Language::parse(code).ok_or_else(|| {
        format!(
            "Unsupported language code \"{code}\". Valid codes: {}.",
            ALL_LANGUAGES
                .iter()
                .map(|language| language.code())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let paths = ControlPaths::discover()?;
    let mut saved = settings::load(&paths)?;
    saved.language = Some(language.code().to_string());
    settings::save(&paths, &saved)?;
    println!(
        "TUI language set to {} ({}).",
        language.native_name(),
        language.code()
    );
    Ok(ExitCode::SUCCESS)
}

fn require_tty() -> Result<(), String> {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        Ok(())
    } else {
        Err("fastctx ui requires both stdin and stdout to be attached to a terminal.".to_string())
    }
}

fn confirm(question: &str) -> Result<bool, String> {
    print!("{question} [y/N] ");
    io::stdout()
        .flush()
        .map_err(|error| format!("Cannot write the confirmation prompt: {error}"))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("Cannot read the confirmation response: {error}"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn print_preview(title: &str, items: &[crate::control::apply::PreviewItem]) {
    use crate::control::apply::PreviewAction;
    println!("{title}:");
    let has_changes = items
        .iter()
        .any(|item| !matches!(item.action, PreviewAction::Unchanged));
    if !has_changes {
        println!("  No changes.");
    }
    for item in items {
        println!(
            "  {:<9} {}",
            item.action.as_str(),
            crate::paths::display_path(&item.path)
        );
        if matches!(item.action, PreviewAction::Keep) {
            println!("            the running binary cannot delete itself; clean it up manually");
        }
        for detail in &item.details {
            let mark = if detail.removed { "- " } else { "  " };
            println!("          {mark}{}", detail.text);
        }
    }
}

fn print_receipt(receipt: &crate::control::apply::OperationReceipt) {
    println!("Changed {} target(s).", receipt.changed_targets);
    for note in &receipt.notes {
        println!("{note}");
    }
}

#[cfg(test)]
mod tests {
    use super::{ServerOptions, run_server_with_io};
    use crate::stdio_transport::DetachedStdin;
    use std::io::{Cursor, Read};
    use std::time::Duration;

    struct BytesThenError {
        bytes: Cursor<Vec<u8>>,
    }

    impl Read for BytesThenError {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let read = self.bytes.read(output)?;
            if read == 0 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "injected established-session stdin failure",
                ))
            } else {
                Ok(read)
            }
        }
    }

    #[tokio::test]
    async fn established_server_reports_stdin_read_error() {
        let initialize = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "stdin-error-contract", "version": "1.0"}
            }
        }))
        .unwrap();
        let mut input = initialize;
        input.push(b'\n');
        let stdin = DetachedStdin::start_with_reader(BytesThenError {
            bytes: Cursor::new(input),
        });

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_server_with_io(ServerOptions::default(), None, stdin, tokio::io::sink()),
        )
        .await
        .expect("established server did not surface the stdin read error");

        assert_eq!(
            result.unwrap_err(),
            "Cannot read MCP stdin: injected established-session stdin failure"
        );
    }
}
