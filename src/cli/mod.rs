//! Command-line parsing, dual-mode TTY dispatch, and non-interactive control commands.

use crate::control::apply::{
    ApplyOptions, UnapplyOptions, commit_apply, commit_unapply, plan_apply, plan_unapply,
    plan_unapply_all,
};
use crate::control::doctor;
use crate::control::i18n::{ALL_LANGUAGES, Language};
use crate::control::paths::ControlPaths;
use crate::control::settings::{self, Tier};
use crate::file_executor::GrepGlobExecutor;
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
    /// Preview and apply a host integration.
    Apply {
        /// Integration host (`codex`, `deepseek-harness`, or `dsh`).
        #[arg(long)]
        host: Option<String>,
        /// Codex profile directory; overrides CODEX_HOME and the default.
        #[arg(long, value_name = "PATH")]
        codex_home: Option<PathBuf>,
        /// DeepSeek Harness home; overrides DSH_HOME and the default.
        #[arg(long, value_name = "PATH")]
        dsh_home: Option<PathBuf>,
        /// Host output tier; defaults to the saved selection.
        #[arg(long, value_enum)]
        tier: Option<Tier>,
        /// Accept the preview and any shared-limit conflict.
        #[arg(long)]
        yes: bool,
    },
    /// Preview and remove one or all host integrations.
    Unapply {
        /// Integration host (`codex`, `deepseek-harness`, or `dsh`).
        #[arg(long)]
        host: Option<String>,
        /// Remove every connected host and shared FastCtx installation.
        #[arg(long)]
        all: bool,
        /// Codex profile directory; overrides CODEX_HOME and the default.
        #[arg(long, value_name = "PATH")]
        codex_home: Option<PathBuf>,
        /// DeepSeek Harness home; overrides DSH_HOME and the default.
        #[arg(long, value_name = "PATH")]
        dsh_home: Option<PathBuf>,
        /// Accept the preview without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Run all local integration checks.
    #[command(visible_alias = "doctor")]
    Status {
        /// Integration host (`codex`, `deepseek-harness`, or `dsh`).
        #[arg(long)]
        host: Option<String>,
        /// Show status for every host.
        #[arg(long)]
        all: bool,
        /// Codex profile directory; overrides CODEX_HOME and the default.
        #[arg(long, value_name = "PATH")]
        codex_home: Option<PathBuf>,
        /// DeepSeek Harness home; overrides DSH_HOME and the default.
        #[arg(long, value_name = "PATH")]
        dsh_home: Option<PathBuf>,
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
    /// Internal short-lived control-center detach bootstrap.
    #[command(hide = true)]
    RuntimeBootstrap,
    /// Internal per-user control center.
    #[command(hide = true)]
    RuntimeHost {
        /// Test-only idle override; production bootstraps always use ten minutes.
        #[arg(long, hide = true)]
        idle_timeout_ms: Option<u64>,
        /// Test-only maintenance override; production bootstraps use one minute.
        #[arg(long, hide = true)]
        maintenance_interval_ms: Option<u64>,
    },
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
            false,
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
            false,
        );
    }
    run_cli(Cli::parse()).await
}

async fn run_cli(cli: Cli) -> Result<ExitCode, String> {
    let implicit_tui =
        cli.command.is_none() && io::stdin().is_terminal() && io::stdout().is_terminal();
    let internal = is_internal_command(&cli.command);
    if !internal && let Ok(paths) = ControlPaths::discover() {
        crate::update::cleanup_replaced_binaries(&paths);
    }
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
            host,
            codex_home,
            dsh_home,
            tier,
            yes,
        }) => run_apply_host(host.as_deref(), codex_home, dsh_home, tier, yes),
        Some(Command::Unapply {
            host,
            all,
            codex_home,
            dsh_home,
            yes,
        }) => run_unapply_host(host.as_deref(), all, codex_home, dsh_home, yes),
        Some(Command::Status {
            host,
            all,
            codex_home,
            dsh_home,
        }) => run_status_host(host.as_deref(), all, codex_home, dsh_home),
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
        Some(Command::RuntimeBootstrap) => {
            crate::runtime::run_bootstrap_entry()?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::RuntimeHost {
            idle_timeout_ms,
            maintenance_interval_ms,
        }) => {
            crate::runtime::run_host_entry(idle_timeout_ms, maintenance_interval_ms).await?;
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

fn is_internal_command(command: &Option<Command>) -> bool {
    let common = matches!(
        command,
        Some(
            Command::JobHost
                | Command::RuntimeBootstrap
                | Command::RuntimeHost { .. }
                | Command::UpdateHelper { .. }
        )
    );
    #[cfg(unix)]
    {
        common
            || matches!(
                command,
                Some(Command::JobBootstrap | Command::JobWatch { .. })
            )
    }
    #[cfg(not(unix))]
    {
        common
    }
}

fn run_tui_with_check(paths: ControlPaths) -> Result<ExitCode, String> {
    crate::update::cleanup_replaced_binaries(&paths);
    run_tui(paths, crate::update::StartupUpdate::None, None, true)
}

fn run_tui(
    paths: ControlPaths,
    startup_update: crate::update::StartupUpdate,
    startup_notice: Option<crate::update::FinalizeNotice>,
    check_for_updates_at_startup: bool,
) -> Result<ExitCode, String> {
    match crate::tui::run(
        paths.clone(),
        startup_update,
        startup_notice,
        check_for_updates_at_startup,
    )? {
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
    let environment = crate::runtime::capture_proxy_environment()?;
    let parent = crate::process_identity::parent_identity_from_environment()?;
    match crate::runtime::connect_or_start(options, &environment).await {
        Ok(stream) => return crate::runtime::forward_stdio(stream, parent).await,
        Err(error) => eprintln!(
            "fastctx: control center unavailable ({error}); falling back to a full standalone MCP server."
        ),
    }
    let session = crate::session::SessionContext::from_environment(environment)?;
    let executor = load_search_executor(&session.control_paths)?;
    let stdin = crate::stdio_transport::DetachedStdin::start()?;
    run_server_with_io_and_executor(
        options,
        parent,
        stdin,
        tokio::io::stdout(),
        executor,
        session,
    )
    .await
}

fn load_search_executor(paths: &ControlPaths) -> Result<Arc<GrepGlobExecutor>, String> {
    let settings = settings::load(paths)?;
    let parallelism = settings.search_parallelism().map_err(|error| {
        format!(
            "Cannot start the MCP server with settings from {}: {error}. Repair the value and retry.",
            crate::paths::display_path(&paths.fastctx_config)
        )
    })?;
    Ok(Arc::new(GrepGlobExecutor::with_parallelism(
        parallelism.effective,
    )))
}

async fn run_server_with_io_and_executor<W>(
    options: ServerOptions,
    parent: Option<Option<crate::process_identity::ProcessIdentity>>,
    stdin: crate::stdio_transport::DetachedStdin,
    stdout: W,
    executor: Arc<GrepGlobExecutor>,
    session: Arc<crate::session::SessionContext>,
) -> Result<ExitCode, String>
where
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let stdin_eof = stdin.eof_token();
    let stdin_read_error = stdin.read_error_receiver();
    let stdin_read_error_wait = wait_for_stdin_read_error(stdin_read_error.clone());
    tokio::pin!(stdin_read_error_wait);
    let service = match FastCtxServer::with_session_and_runtime(
        options,
        session,
        crate::server::SharedRuntime::new(executor),
    )
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

fn host_kind(value: Option<&str>) -> Result<&'static str, String> {
    match value {
        None | Some("codex") => Ok("codex"),
        Some("dsh" | "deepseek-harness") => Ok("deepseek-harness"),
        Some(value) => Err(format!(
            "Unknown host \"{value}\". Use --host codex or --host deepseek-harness (alias: dsh)."
        )),
    }
}

fn validate_host_paths(
    host: &str,
    codex_home: &Option<PathBuf>,
    dsh_home: &Option<PathBuf>,
) -> Result<(), String> {
    if host == "codex" && dsh_home.is_some() {
        return Err("--dsh-home can only be used with --host deepseek-harness.".to_string());
    }
    if host == "deepseek-harness" && codex_home.is_some() {
        return Err("--codex-home can only be used with --host codex.".to_string());
    }
    Ok(())
}

fn run_apply_host(
    host: Option<&str>,
    codex_home: Option<PathBuf>,
    dsh_home: Option<PathBuf>,
    tier: Option<Tier>,
    yes: bool,
) -> Result<ExitCode, String> {
    let host = host_kind(host)?;
    validate_host_paths(host, &codex_home, &dsh_home)?;
    if host == "deepseek-harness" {
        return run_dsh_apply(dsh_home, tier, yes);
    }
    run_apply(codex_home, tier, yes)
}

fn run_dsh_apply(
    dsh_home: Option<PathBuf>,
    tier: Option<Tier>,
    yes: bool,
) -> Result<ExitCode, String> {
    let paths = ControlPaths::discover_with_hosts(None, dsh_home)?;
    let saved = settings::load(&paths)?;
    let plan = crate::control::dsh::plan_apply(
        &paths,
        crate::control::dsh::ApplyOptions {
            tier: tier.unwrap_or(saved.tier),
            tool_budgets: saved.tool_budgets,
            fastshell_enabled: saved.fastshell.enabled,
            current_executable: std::env::current_exe()
                .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?,
        },
    )?;
    println!("Apply preview (DeepSeek Harness)");
    println!("  Host       deepseek-harness");
    println!(
        "  DSH home   {} (source: {})",
        crate::paths::display_path(&paths.dsh_dir),
        paths.dsh_home_source.as_str()
    );
    println!(
        "  Patch      {}",
        crate::paths::display_path(&paths.dsh_patch)
    );
    println!(
        "  Timeout    {}ms",
        crate::control::dsh_config::TOOL_TIMEOUT_MS
    );
    println!("  Scope      Host-wide (all DeepSeek Harness profiles)");
    for change in plan.preview_changes() {
        println!(
            "  {:<9} {}",
            if change.is_changed() {
                "Change"
            } else {
                "Unchanged"
            },
            crate::paths::display_path(&change.target)
        );
    }
    if plan.running_jobs() > 0 {
        println!(
            "  Stop      {} running background job(s)",
            plan.running_jobs()
        );
    }
    if plan.running_processes() > 0 {
        println!(
            "  Stop      {} running FastCtx process(es)",
            plan.running_processes()
        );
    }
    if plan.is_empty() {
        crate::control::dsh::commit_apply(plan)?;
        println!("No changes were needed.");
        return Ok(ExitCode::SUCCESS);
    }
    if !yes {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err("Apply requires confirmation in a terminal. Re-run with --yes after reviewing the preview.".to_string());
        }
        if !confirm("Apply these changes to DeepSeek Harness?")? {
            println!("Cancelled. No files were written.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let changed = crate::control::dsh::commit_apply(plan)?;
    println!("Applied DeepSeek Harness integration ({changed} file target(s)).");
    Ok(ExitCode::SUCCESS)
}

fn run_apply(
    codex_home: Option<PathBuf>,
    tier: Option<Tier>,
    yes: bool,
) -> Result<ExitCode, String> {
    let paths = ControlPaths::discover_with_codex_home(codex_home)?;
    let startup = settings::load_for_startup(&paths)?;
    if startup.migration_notice {
        print_cli_migration_notice("This Apply will write them into Codex.");
    }
    let saved = startup.settings;
    let plan = plan_apply(
        &paths,
        ApplyOptions {
            tier: tier.unwrap_or(saved.tier),
            tool_budgets: saved.tool_budgets,
            output_guard_enabled: saved.output_guard.enabled,
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

fn run_unapply_host(
    host: Option<&str>,
    all: bool,
    codex_home: Option<PathBuf>,
    dsh_home: Option<PathBuf>,
    yes: bool,
) -> Result<ExitCode, String> {
    let parsed_host = host_kind(host)?;
    if all && (host.is_some() || codex_home.is_some() || dsh_home.is_some()) {
        return Err(
            "--all cannot be combined with --host, --codex-home, or --dsh-home.".to_string(),
        );
    }
    if !all {
        validate_host_paths(parsed_host, &codex_home, &dsh_home)?;
        if parsed_host == "deepseek-harness" {
            return run_dsh_unapply(dsh_home, yes);
        }
        return run_unapply(codex_home, yes);
    }
    let dsh_paths = all_host_paths_from_receipts()?;
    let saved = settings::load(&dsh_paths)?;
    let has_dsh = saved.integrations.deepseek_harness.is_some();
    let has_codex = saved.integrations.codex.is_some();
    if !yes && (has_dsh || has_codex) {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(
                "unapply --all requires confirmation in a terminal. Re-run with --yes after reviewing the affected hosts."
                    .to_string(),
            );
        }
        println!("Unapply all preview");
        println!(
            "  ChatGPT / Codex      {}",
            if has_codex { "Disconnect" } else { "Unchanged" }
        );
        println!(
            "  DeepSeek Harness     {}",
            if has_dsh { "Disconnect" } else { "Unchanged" }
        );
        println!("  Shared FastCtx data  Delete after the last connected host");
        if !confirm("Disconnect all FastCtx hosts and remove shared data?")? {
            println!("Cancelled. No files were written.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let current_executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?;
    let complete = plan_unapply_all(&dsh_paths, current_executable)?;
    if complete.is_empty() {
        println!("No host integrations were connected.");
        return Ok(ExitCode::SUCCESS);
    }
    let receipt = commit_unapply(complete)?;
    print_receipt(&receipt);
    Ok(ExitCode::SUCCESS)
}

fn run_dsh_unapply(dsh_home: Option<PathBuf>, yes: bool) -> Result<ExitCode, String> {
    let paths = ControlPaths::discover_with_hosts(None, dsh_home)?;
    let plan = crate::control::dsh::plan_unapply(
        &paths,
        std::env::current_exe()
            .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?,
    )?;
    if plan.is_empty() {
        println!("DeepSeek Harness is not connected.");
        return Ok(ExitCode::SUCCESS);
    }
    println!("Unapply preview (DeepSeek Harness)");
    for change in plan.preview_changes() {
        println!(
            "  {:<9} {}",
            if change.is_changed() {
                "Change"
            } else {
                "Unchanged"
            },
            crate::paths::display_path(&change.target)
        );
    }
    if plan.running_jobs() > 0 {
        println!(
            "  Stop      {} running background job(s)",
            plan.running_jobs()
        );
    }
    if plan.running_processes() > 0 {
        println!(
            "  Stop      {} running FastCtx process(es)",
            plan.running_processes()
        );
    }
    if !yes {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err("Unapply requires confirmation in a terminal. Re-run with --yes after reviewing the preview.".to_string());
        }
        if !confirm("Remove FastCtx from DeepSeek Harness?")? {
            println!("Cancelled. No files were written.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let changed = crate::control::dsh::commit_unapply(plan)?;
    println!("Removed DeepSeek Harness integration ({changed} file target(s)).");
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

fn run_status_host(
    host: Option<&str>,
    all: bool,
    codex_home: Option<PathBuf>,
    dsh_home: Option<PathBuf>,
) -> Result<ExitCode, String> {
    let parsed_host = host_kind(host)?;
    if all && (host.is_some() || codex_home.is_some() || dsh_home.is_some()) {
        return Err(
            "--all cannot be combined with --host, --codex-home, or --dsh-home.".to_string(),
        );
    }
    if all {
        let paths = all_host_paths_from_receipts()?;
        let (dsh_state, dsh_detail) = crate::control::dsh::status(&paths)?;
        println!(
            "[{}] DeepSeek Harness: {}",
            dsh_state.to_uppercase(),
            dsh_detail
        );
        let codex = run_status_with_paths(&paths);
        return Ok(if dsh_state == "connected" && codex == ExitCode::SUCCESS {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    validate_host_paths(parsed_host, &codex_home, &dsh_home)?;
    if parsed_host == "deepseek-harness" {
        let paths = ControlPaths::discover_with_hosts(None, dsh_home)?;
        let (state, detail) = crate::control::dsh::status(&paths)?;
        println!("[{}] DeepSeek Harness: {detail}", state.to_uppercase());
        return Ok(if state == "connected" {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    run_status(codex_home)
}

fn run_status(codex_home: Option<PathBuf>) -> Result<ExitCode, String> {
    let paths = ControlPaths::discover_with_codex_home(codex_home)?;
    Ok(run_status_with_paths(&paths))
}

fn run_status_with_paths(paths: &ControlPaths) -> ExitCode {
    use crate::control::doctor::DoctorCheckStatus;

    let report = doctor::run(paths);
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
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn all_host_paths_from_receipts() -> Result<ControlPaths, String> {
    let discovered = ControlPaths::discover_with_hosts(None, None)?;
    let saved = settings::load(&discovered)?;
    settings::paths_for_integrations(&discovered, &saved)
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
    let startup = settings::load_for_startup(&paths)?;
    if startup.migration_notice {
        print_cli_migration_notice("Run fastctx apply to write them into Codex.");
    }
    let mut saved = startup.settings;
    saved.language = Some(language.code().to_string());
    settings::save(&paths, &saved)?;
    println!(
        "TUI language set to {} ({}).",
        language.native_name(),
        language.code()
    );
    Ok(ExitCode::SUCCESS)
}

fn print_cli_migration_notice(next_step: &str) {
    println!(
        "FastCtx v{} updated the recommended per-tool output budgets in your settings. {next_step}",
        env!("CARGO_PKG_VERSION"),
    );
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
