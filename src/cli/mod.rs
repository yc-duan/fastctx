//! Command-line parsing, dual-mode TTY dispatch, and non-interactive control commands.

use crate::control::apply::{UnapplyOptions, commit_unapply, plan_unapply};
use crate::control::doctor;
use crate::control::i18n::{ALL_LANGUAGES, Language};
use crate::control::paths::ControlPaths;
use crate::control::settings::{self, Tier};
use crate::control::target_apply::{
    TargetApplyOptions, commit_target_apply, commit_target_disconnect, plan_target_apply,
    plan_target_disconnect,
};
use crate::control::targets::AgentTarget;
use crate::server::ServerOptions;
use crate::server_manifest::EnabledTools;
use clap::{Parser, Subcommand};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

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
        /// Comma-separated enabled tools; file tools are independent and shell tools are atomic.
        #[arg(long, value_name = "TOOL,...", conflicts_with = "enable_shell")]
        tools: Option<String>,
        /// Publish the five optional shell tools.
        #[arg(long, hide = true)]
        enable_shell: bool,
        /// Deprecated compatibility flag; replace is always published.
        #[arg(long, hide = true)]
        enable_edit: bool,
    },
    /// Force the full-screen control terminal.
    Ui,
    /// Preview and apply one agent integration.
    Apply {
        /// Agent target; defaults to Codex.
        #[arg(long, value_name = "ID")]
        target: Option<AgentTarget>,
        /// Comma-separated enabled tools; omission reuses the target preference.
        #[arg(long, value_name = "TOOL,...")]
        tools: Option<String>,
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
    /// Disconnect one target, or remove FastCtx completely when target is omitted.
    Unapply {
        /// Disconnect only this agent target.
        #[arg(long, value_name = "ID")]
        target: Option<AgentTarget>,
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
        /// Show detailed checks for one target after the shared report.
        #[arg(long, value_name = "ID")]
        target: Option<AgentTarget>,
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
    /// Run or manage a World hub on this machine.
    Hub {
        #[command(subcommand)]
        command: crate::world::cli::HubCommand,
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
            tools,
            enable_shell,
            enable_edit: _,
        }) => {
            let tools = match tools {
                Some(csv) => EnabledTools::from_csv(&csv)?,
                None if enable_shell => EnabledTools::all(),
                None => EnabledTools::files(),
            };
            run_server_with_options(ServerOptions { tools }).await
        }
        Some(Command::Ui) => {
            require_tty()?;
            let paths = ControlPaths::discover()?;
            run_tui_with_check(paths)
        }
        Some(Command::Apply {
            target,
            tools,
            codex_home,
            tier,
            yes,
        }) => run_apply(target, tools, codex_home, tier, yes),
        Some(Command::Unapply {
            target,
            codex_home,
            yes,
        }) => run_unapply(target, codex_home, yes),
        Some(Command::Status { target, codex_home }) => run_status(target, codex_home),
        Some(Command::Lang { code }) => run_lang(&code),
        Some(Command::Jobs { command }) => run_jobs(command),
        Some(Command::Hub { command }) => crate::world::cli::run_hub(command).await,
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
    crate::runtime::run_proxy_session(options, environment, parent).await
}

fn run_apply(
    target: Option<AgentTarget>,
    tools: Option<String>,
    codex_home: Option<PathBuf>,
    tier: Option<Tier>,
    yes: bool,
) -> Result<ExitCode, String> {
    let target = target.unwrap_or(AgentTarget::Codex);
    if target != AgentTarget::Codex && codex_home.is_some() {
        return Err("--codex-home is valid only with --target codex.".to_string());
    }
    let paths = ControlPaths::discover_with_codex_home(codex_home)?;
    let startup = settings::load_for_startup(&paths)?;
    if startup.migration_notice {
        print_cli_migration_notice("This Apply will record the selected target policy.");
    }
    let saved = startup.settings;
    let enabled_tools = match tools {
        Some(csv) => EnabledTools::from_csv(&csv)?,
        None => saved.selected_tools(target),
    };
    let plan = plan_target_apply(
        &paths,
        TargetApplyOptions {
            target,
            enabled_tools,
            tier: tier.unwrap_or(saved.tier),
            tool_budgets: saved.tool_budgets,
            output_guard_enabled: saved.output_guard.enabled,
            current_executable: std::env::current_exe()
                .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?,
        },
    )?;
    print_preview("Apply preview", plan.preview());
    if plan.is_empty() {
        let receipt = commit_target_apply(plan, true)?;
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
        if plan.needs_confirmation() {
            println!("Shared setting warning: Codex tool_output_token_limit will change.");
            if !confirm("Change this shared Codex setting?")? {
                println!("Cancelled. No files were written.");
                return Ok(ExitCode::SUCCESS);
            }
        }
        if !confirm("Apply these changes?")? {
            println!("Cancelled. No files were written.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let receipt = commit_target_apply(plan, yes || io::stdin().is_terminal())?;
    print_receipt(&receipt);
    Ok(ExitCode::SUCCESS)
}

fn run_unapply(
    target: Option<AgentTarget>,
    codex_home: Option<PathBuf>,
    yes: bool,
) -> Result<ExitCode, String> {
    if target.is_some_and(|target| target != AgentTarget::Codex) && codex_home.is_some() {
        return Err("--codex-home is valid only with --target codex.".to_string());
    }
    let paths = ControlPaths::discover_with_codex_home(codex_home)?;
    if let Some(target) = target {
        let plan = plan_target_disconnect(&paths, target)?;
        print_preview("Disconnect preview", plan.preview());
        if !yes && !plan.is_empty() {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                return Err(
                    "Disconnect requires confirmation in a terminal. Re-run with --yes after reviewing the preview."
                        .to_string(),
                );
            }
            if !confirm(&format!("Disconnect {}?", target.display_name()))? {
                println!("Cancelled. No files were written.");
                return Ok(ExitCode::SUCCESS);
            }
        }
        let receipt = commit_target_disconnect(plan)?;
        print_receipt(&receipt);
        return Ok(ExitCode::SUCCESS);
    }
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

fn run_status(
    target: Option<AgentTarget>,
    codex_home: Option<PathBuf>,
) -> Result<ExitCode, String> {
    use crate::control::doctor::DoctorCheckStatus;

    let paths = ControlPaths::discover_with_codex_home(codex_home)?;
    let settings = settings::load(&paths)?;
    for candidate in AgentTarget::ALL {
        let status = crate::control::target_status::inspect_target(&paths, &settings, candidate);
        let label = match status.state {
            crate::control::target_status::TargetConnectionState::NotConnected => "NOT CONNECTED",
            crate::control::target_status::TargetConnectionState::Connected => "CONNECTED",
            crate::control::target_status::TargetConnectionState::NeedsAttention => "ATTENTION",
            crate::control::target_status::TargetConnectionState::PermissionDenied => "NO ACCESS",
            crate::control::target_status::TargetConnectionState::Error => "ERROR",
        };
        println!(
            "[{label}] {}: {}",
            candidate.display_name(),
            status.enabled_tools.names().join(",")
        );
        if target == Some(candidate) {
            println!(
                "       Config: {}",
                crate::paths::display_path(&status.config_path)
            );
            println!(
                "       Guidance: {}",
                crate::paths::display_path(&status.guidance_path)
            );
            println!("       Budget: {} tokens", status.effective_budget);
            for fact in &status.facts {
                println!("       {fact}");
            }
        }
    }
    println!();
    let report = if target.is_some() {
        doctor::run(&paths)
    } else {
        doctor::run_with_connected_targets(&paths)
    };
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
    let target_report = target.map(|target| doctor::run_target(&paths, target));
    if let Some(target_report) = &target_report {
        println!();
        for check in &target_report.checks {
            let label = match check.status {
                DoctorCheckStatus::Pass => "PASS",
                DoctorCheckStatus::Info => "INFO",
                DoctorCheckStatus::Fail => "FAIL",
            };
            println!("[{label}] {}: {}", check.name, check.detail);
            if let Some(remedy) = &check.remedy {
                println!("       Next: {remedy}");
            }
        }
    }
    Ok(
        if report.passed() && target_report.as_ref().is_none_or(|report| report.passed()) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        },
    )
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
