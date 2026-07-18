//! Diagnosable Status/Doctor checks and a real stdio MCP handshake.

use crate::control::agents;
use crate::control::codex_config::{self, ExpectedConfig};
use crate::control::paths::ControlPaths;
use crate::control::settings;
use crate::server::{FastCtxServer, ServerOptions};
use crate::server_manifest::{ToolContract, ToolManifest};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(4);
const MCP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
static CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Three-state result for one doctor check; Info does not affect the status exit code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DoctorCheckStatus {
    /// Configured and passing.
    Pass,
    /// Not yet applied or not currently applicable.
    Info,
    /// Existing configuration is damaged, drifted, or unavailable.
    Fail,
}

/// Result of one doctor check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorCheck {
    /// Stable English check name.
    pub name: &'static str,
    /// Pass, information, or failure state.
    pub status: DoctorCheckStatus,
    /// Current observation.
    pub detail: String,
    /// Recovery step after failure; empty for PASS and INFO.
    pub remedy: Option<String>,
}

impl DoctorCheck {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorCheckStatus::Pass,
            detail: detail.into(),
            remedy: None,
        }
    }

    fn info(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorCheckStatus::Info,
            detail: detail.into(),
            remedy: None,
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorCheckStatus::Fail,
            detail: detail.into(),
            remedy: Some(remedy.into()),
        }
    }
}

/// Ordered report of all doctor checks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DoctorReport {
    /// Checks in contract-table order.
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// Whether no check failed; INFO does not affect the exit code.
    pub fn passed(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != DoctorCheckStatus::Fail)
    }
}

/// Runs the complete status contract against the configured paths.
pub fn run(paths: &ControlPaths) -> DoctorReport {
    let mut checks = Vec::new();
    checks.push(check_profile(paths));

    let config_bytes = match std::fs::read(&paths.codex_config) {
        Ok(bytes) => {
            let check = match std::str::from_utf8(&bytes)
                .map_err(|error| error.to_string())
                .and_then(|source| {
                    toml_edit::DocumentMut::from_str(source)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }) {
                Ok(()) => DoctorCheck::pass(
                    "Codex config",
                    format!("Parsed {}", crate::paths::display_path(&paths.codex_config)),
                ),
                Err(error) => DoctorCheck::fail(
                    "Codex config",
                    format!(
                        "Cannot parse {}: {error}",
                        crate::paths::display_path(&paths.codex_config)
                    ),
                    "Repair config.toml manually, then run fastctx status again.",
                ),
            };
            checks.push(check);
            Some(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            checks.push(DoctorCheck::info(
                "Codex config",
                format!(
                    "{} does not exist yet; Apply will create it.",
                    crate::paths::display_path(&paths.codex_config)
                ),
            ));
            None
        }
        Err(error) => {
            checks.push(DoctorCheck::fail(
                "Codex config",
                format!(
                    "Cannot read {}: {error}",
                    crate::paths::display_path(&paths.codex_config)
                ),
                "Fix the path or permissions, then run fastctx status again.",
            ));
            None
        }
    };

    let settings = settings::load(paths);
    checks.push(match settings.as_ref() {
        Ok(settings) => check_drift(paths, Some(settings), config_bytes.as_deref()),
        Err(error) => DoctorCheck::fail(
            "Applied state",
            error.clone(),
            "Repair ~/.fastctx/config.toml or re-run Apply after moving the damaged file aside.",
        ),
    });
    checks.push(check_binary(
        paths,
        settings
            .as_ref()
            .ok()
            .and_then(|settings| settings.applied.as_ref()),
    ));
    checks.push(check_running_instances(paths));
    let saved_settings = settings.as_ref().ok();
    let applied = settings
        .as_ref()
        .ok()
        .and_then(|settings| settings.applied.as_ref());
    checks.push(check_mcp(&paths.installed_binary, applied));
    checks.push(check_agents(
        paths,
        applied.is_some(),
        applied.is_some_and(|record| record.fastshell_enabled),
    ));
    let fastshell_desired = saved_settings.is_some_and(|settings| settings.fastshell.enabled);
    let fastshell_applied = applied.is_some_and(|record| record.fastshell_enabled);
    checks.push(check_extension_state(
        "fastshell",
        fastshell_desired,
        fastshell_applied,
    ));
    if settings.is_ok() {
        checks.push(check_job_limits(paths));
    }
    checks.push(DoctorCheck::info(
        "Last update check",
        crate::update::last_check_status(paths).detail,
    ));
    DoctorReport { checks }
}

fn check_running_instances(paths: &ControlPaths) -> DoctorCheck {
    match crate::control::processes::installed_processes(&paths.fastctx_bin_dir) {
        Ok(processes) => {
            let pids = processes
                .iter()
                .filter(|process| process.identity.pid != std::process::id())
                .map(|process| process.identity.pid.to_string())
                .collect::<Vec<_>>();
            if pids.is_empty() {
                DoctorCheck::info(
                    "Running server instances",
                    "No other FastCtx process images are running from the managed bin directory.",
                )
            } else {
                DoctorCheck::pass(
                    "Running server instances",
                    format!(
                        "{} managed FastCtx process image(s) are running; PID(s): {}. This is informational and does not classify session health.",
                        pids.len(),
                        pids.join(", ")
                    ),
                )
            }
        }
        Err(error) => DoctorCheck::info(
            "Running server instances",
            format!("Running FastCtx process images could not be enumerated: {error}"),
        ),
    }
}

fn check_job_limits(paths: &ControlPaths) -> DoctorCheck {
    match settings::job_limit_status(paths) {
        Ok(status) => {
            let effective = format!(
                "Effective current-user limits: {} MiB retained job storage; {} running jobs; {} records per job_list page.",
                status.job_storage_limit_mib, status.max_running_jobs, status.job_list_limit
            );
            let mut invalid = Vec::new();
            if status.storage_limit_fell_back {
                invalid.push("fastshell.job_storage_limit_mib");
            }
            if status.running_limit_fell_back {
                invalid.push("fastshell.max_running_jobs");
            }
            if status.list_limit_fell_back {
                invalid.push("fastshell.job_list_limit");
            }
            if invalid.is_empty() {
                DoctorCheck::pass("Job limits", effective)
            } else {
                DoctorCheck::info(
                    "Job limits",
                    format!(
                        "Invalid {} value(s) fell back to safe defaults. {effective}",
                        invalid.join(", ")
                    ),
                )
            }
        }
        Err(error) => DoctorCheck::fail(
            "Job limits",
            error,
            "Repair ~/.fastctx/config.toml, then run fastctx status again.",
        ),
    }
}

fn check_profile(paths: &ControlPaths) -> DoctorCheck {
    match std::fs::metadata(&paths.codex_dir) {
        Ok(metadata) if metadata.is_dir() => DoctorCheck::pass(
            "Codex profile",
            format!(
                "Configuration root: {}",
                crate::paths::display_path(&paths.codex_dir)
            ),
        ),
        Ok(_) => DoctorCheck::fail(
            "Codex profile",
            format!(
                "{} exists but is not a directory.",
                crate::paths::display_path(&paths.codex_dir)
            ),
            "Move or remove that path so Apply can create the configuration directory.",
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DoctorCheck::info(
            "Codex profile",
            format!(
                "{} does not exist yet; Apply will create it.",
                crate::paths::display_path(&paths.codex_dir)
            ),
        ),
        Err(error) => DoctorCheck::fail(
            "Codex profile",
            format!(
                "Cannot inspect {}: {error}",
                crate::paths::display_path(&paths.codex_dir)
            ),
            "Fix the path or permissions, then run fastctx status again.",
        ),
    }
}

fn check_drift(
    paths: &ControlPaths,
    settings: Option<&settings::FastCtxSettings>,
    config: Option<&[u8]>,
) -> DoctorCheck {
    let Some(record) = settings.and_then(|settings| settings.applied.as_ref()) else {
        return DoctorCheck::info(
            "Applied state",
            "fastctx has not been applied in this profile. Run fastctx apply when ready.",
        );
    };
    let Some(config) = config else {
        return DoctorCheck::fail(
            "Applied state",
            "Codex config could not be inspected.",
            "Repair Codex config.toml, then re-apply.",
        );
    };
    let expected = ExpectedConfig {
        command: record.command.clone(),
        tier: record.tier,
        tool_budgets: record.tool_budgets,
        fastshell_enabled: record.fastshell_enabled,
    };
    let legacy_fastedit =
        settings.is_some_and(|settings| settings.fastedit.enabled) || record.fastedit_enabled;
    match codex_config::drift(config, &expected).and_then(|mut items| {
        items.extend(receipt_drift(paths, record)?);
        if legacy_fastedit {
            items.push("legacy fastedit configuration".to_string());
        }
        items.sort();
        items.dedup();
        Ok(items)
    }) {
        Ok(items) if items.is_empty() => DoctorCheck::pass(
            "Applied state",
            "Managed Codex settings match the Apply receipt.",
        ),
        Ok(items) => DoctorCheck::fail(
            "Applied state",
            format!("Drift detected: {}", items.join(", ")),
            "Run fastctx apply to preview and repair only the managed entries.",
        ),
        Err(error) => DoctorCheck::fail(
            "Applied state",
            error,
            "Repair Codex config.toml, then re-apply.",
        ),
    }
}

fn check_binary(paths: &ControlPaths, record: Option<&settings::AppliedRecord>) -> DoctorCheck {
    if paths.installed_binary.exists() && !paths.installed_binary.is_file() {
        return DoctorCheck::fail(
            "Installed binary",
            format!(
                "{} exists but is not a regular file.",
                crate::paths::display_path(&paths.installed_binary)
            ),
            "Move or remove that path, then run fastctx apply.",
        );
    }
    if !paths.installed_binary.is_file() {
        if record.is_none() {
            return DoctorCheck::info(
                "Installed binary",
                format!(
                    "{} is not installed yet; Apply will create it.",
                    crate::paths::display_path(&paths.installed_binary)
                ),
            );
        }
        return DoctorCheck::fail(
            "Installed binary",
            format!(
                "{} is missing.",
                crate::paths::display_path(&paths.installed_binary)
            ),
            "Run fastctx apply to install the stable binary.",
        );
    }
    if let Some(record) = record {
        match std::fs::read(&paths.installed_binary) {
            Ok(bytes) if sha256(&bytes) != record.binary_sha256 => {
                return DoctorCheck::fail(
                    "Installed binary",
                    "The installed binary content does not match the Apply receipt.",
                    "Run fastctx apply to refresh the stable binary.",
                );
            }
            Ok(_) => {}
            Err(error) => {
                return DoctorCheck::fail(
                    "Installed binary",
                    format!(
                        "Cannot read {}: {error}",
                        crate::paths::display_path(&paths.installed_binary)
                    ),
                    "Run fastctx apply to replace the stable binary.",
                );
            }
        }
    }
    let result = match run_output(
        crate::process_policy::noninteractive_command(&paths.installed_binary).arg("--version"),
        PROCESS_TIMEOUT,
    ) {
        Ok(output) if output.status.success() => {
            let actual = output_detail(&output);
            let expected = format!("fastctx {}", env!("CARGO_PKG_VERSION"));
            if actual == expected {
                DoctorCheck::pass("Installed binary", actual)
            } else {
                DoctorCheck::fail(
                    "Installed binary",
                    format!("Expected {expected}, got {actual}."),
                    "Run fastctx apply to refresh the stable binary.",
                )
            }
        }
        Ok(output) => DoctorCheck::fail(
            "Installed binary",
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
            "Run fastctx apply to replace the stable binary.",
        ),
        Err(error) => DoctorCheck::fail(
            "Installed binary",
            error,
            "Run fastctx apply to replace the stable binary.",
        ),
    };
    if record.is_none() && result.status == DoctorCheckStatus::Pass {
        DoctorCheck::info(
            "Installed binary",
            format!(
                "{} is runnable but is not owned by an Apply receipt; Apply will adopt and record it.",
                crate::paths::display_path(&paths.installed_binary)
            ),
        )
    } else {
        result
    }
}

fn receipt_drift(
    paths: &ControlPaths,
    record: &settings::AppliedRecord,
) -> Result<Vec<String>, String> {
    let mut drift = Vec::new();
    // Codex rewrites unowned config fields on startup, and users may edit outside our AGENTS block.
    // Status therefore validates those managed semantics separately instead of hashing whole files (2026-07-17).
    check_recorded_path(
        &paths.codex_config,
        &record.codex_config,
        "Codex config receipt",
        &mut drift,
    );
    check_recorded_path(
        &paths.codex_agents,
        &record.codex_agents,
        "AGENTS receipt",
        &mut drift,
    );
    if record.command != crate::paths::display_path(&paths.installed_binary) {
        drift.push("installed binary receipt path".to_string());
    }
    match std::fs::read(&paths.installed_binary) {
        Ok(bytes) if sha256(&bytes) != record.binary_sha256 => {
            drift.push("installed binary content".to_string())
        }
        Ok(_) => {}
        Err(error) => {
            return Err(format!(
                "Cannot read installed binary {} while checking the Apply receipt: {error}",
                crate::paths::display_path(&paths.installed_binary)
            ));
        }
    }
    Ok(drift)
}

fn check_recorded_path(
    path: &Path,
    record: &settings::ManagedFileRecord,
    label: &str,
    drift: &mut Vec<String>,
) {
    if record.path != crate::paths::display_path(path) {
        drift.push(format!("{label} path"));
    }
}

fn check_mcp(executable: &Path, applied: Option<&settings::AppliedRecord>) -> DoctorCheck {
    if !executable.is_file() && applied.is_none() {
        return DoctorCheck::info(
            "MCP handshake",
            "Not run before Apply installs the stable fastctx binary.",
        );
    }
    let options = applied.map_or_else(ServerOptions::default, |record| ServerOptions {
        enable_shell: record.fastshell_enabled,
    });
    match probe_mcp(executable, options) {
        Ok(()) => DoctorCheck::pass(
            "MCP handshake",
            format!(
                "initialize and tools/list returned {} tools with matching contract hashes.",
                ToolManifest::expected_names(options.enable_shell).len()
            ),
        ),
        Err(error) => DoctorCheck::fail(
            "MCP handshake",
            error,
            "Run fastctx apply, then retry status. If it still fails, run the configured fastctx serve command from a terminal to inspect the error.",
        ),
    }
}

fn check_extension_state(name: &'static str, desired: bool, applied: bool) -> DoctorCheck {
    match (desired, applied) {
        (false, false) => DoctorCheck::info(
            name,
            format!("{name} is disabled. Enable it in Config and run Apply to register it."),
        ),
        (true, false) => DoctorCheck::info(
            name,
            format!("{name} is enabled in Config and will be registered by the next Apply."),
        ),
        (false, true) => DoctorCheck::info(
            name,
            format!(
                "{name} is still applied but is disabled in Config; the next Apply will remove it."
            ),
        ),
        (true, true) => DoctorCheck::pass(name, format!("{name} is enabled and applied.")),
    }
}

fn check_agents(paths: &ControlPaths, applied: bool, fastshell_enabled: bool) -> DoctorCheck {
    match std::fs::read(&paths.codex_agents) {
        Ok(bytes) => match agents::has_exact_section_for(&bytes, fastshell_enabled) {
            Ok(true) if applied => DoctorCheck::pass(
                "AGENTS guidance",
                "The fastctx guidance block matches the current contract.",
            ),
            Ok(true) => DoctorCheck::info(
                "AGENTS guidance",
                "A current fastctx guidance block exists without an Apply receipt; Apply will adopt and record it.",
            ),
            Ok(false) if applied => DoctorCheck::fail(
                "AGENTS guidance",
                "The fastctx guidance block is missing or outdated.",
                "Run fastctx apply to refresh only the private marker block.",
            ),
            Ok(false) => DoctorCheck::info(
                "AGENTS guidance",
                format!(
                    "No managed guidance block is present in {}; Apply will add one without changing other content.",
                    crate::paths::display_path(&paths.codex_agents)
                ),
            ),
            Err(error) => DoctorCheck::fail(
                "AGENTS guidance",
                error,
                "Repair AGENTS.md as UTF-8 with one fastctx marker block, then re-apply.",
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !applied => {
            DoctorCheck::info(
                "AGENTS guidance",
                format!(
                    "{} does not exist yet; Apply will create it.",
                    crate::paths::display_path(&paths.codex_agents)
                ),
            )
        }
        Err(error) => DoctorCheck::fail(
            "AGENTS guidance",
            format!(
                "Cannot read {}: {error}",
                crate::paths::display_path(&paths.codex_agents)
            ),
            "Run fastctx apply to create the private guidance block.",
        ),
    }
}

/// Runs MCP initialize and tools/list through a real child process.
pub fn probe_mcp(executable: &Path, options: ServerOptions) -> Result<(), String> {
    let expected = FastCtxServer::with_options(options).tool_contracts();
    let mut arguments = vec!["serve"];
    if options.enable_shell {
        arguments.push("--enable-shell");
    }
    probe_mcp_server(executable, &arguments, &expected)
}

/// Probes one explicit server invocation and requires exact tool contracts.
pub fn probe_mcp_server(
    executable: &Path,
    arguments: &[&str],
    expected_contracts: &[ToolContract],
) -> Result<(), String> {
    let mut child = crate::process_policy::noninteractive_command(executable);
    child
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = child
        .spawn()
        .map_err(|error| format!("MCP spawn failed: {error}"))?;
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            terminate(&mut child);
            return Err("MCP spawn failed: stdin was not piped.".to_string());
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate(&mut child);
            return Err("MCP spawn failed: stdout was not piped.".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate(&mut child);
            return Err("MCP spawn failed: stderr was not piped.".to_string());
        }
    };
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if sender.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    break;
                }
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut stderr = stderr;
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let result = (|| {
        send_json(
            &mut stdin,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "fastctx-doctor", "version": env!("CARGO_PKG_VERSION")}
                }
            }),
        )?;
        let initialized = receive_response(&receiver, 1, "initialize")?;
        if initialized.get("error").is_some() {
            return Err(format!(
                "MCP handshake failed during initialize: {initialized}"
            ));
        }
        send_json(
            &mut stdin,
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        )?;
        send_json(
            &mut stdin,
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        )?;
        let listed = receive_response(&receiver, 2, "tools/list")?;
        if listed.get("error").is_some() {
            return Err(format!("MCP handshake failed during tools/list: {listed}"));
        }
        let tools = listed["result"]["tools"]
            .as_array()
            .ok_or_else(|| format!("MCP tools/list returned an invalid payload: {listed}"))?;
        let definitions = tools
            .iter()
            .cloned()
            .map(serde_json::from_value::<rmcp::model::Tool>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                format!("MCP tools/list returned an invalid tool definition: {error}")
            })?;
        let enable_shell = expected_contracts
            .iter()
            .any(|contract| contract.group == crate::server_manifest::ToolGroup::Shell);
        ToolManifest::validate(&definitions, enable_shell)
            .map_err(|error| format!("MCP tools/list manifest mismatch: {error}"))?;
        let actual = ToolManifest::contracts(&definitions)?
            .into_iter()
            .map(|contract| (contract.name, contract.hash))
            .collect::<BTreeMap<_, _>>();
        let expected = expected_contracts
            .iter()
            .map(|contract| (contract.name.clone(), contract.hash.clone()))
            .collect::<BTreeMap<_, _>>();
        if actual != expected {
            return Err(format!(
                "MCP tools/list contract hashes differ: expected {expected:?}, got {actual:?}."
            ));
        }
        Ok(())
    })();

    drop(stdin);
    if result.is_err() {
        terminate(&mut child);
    }
    let exit = wait_child(&mut child, PROCESS_TIMEOUT);
    let _ = join_with_timeout(reader, Duration::from_millis(500));
    let stderr = join_with_timeout(stderr_reader, Duration::from_millis(500)).unwrap_or_default();
    if let Err(error) = result {
        return Err(with_stderr(error, &stderr));
    }
    let status = exit.map_err(|error| with_stderr(error, &stderr))?;
    if !status.success() {
        return Err(with_stderr(
            format!("MCP server exited with {status}."),
            &stderr,
        ));
    }
    Ok(())
}

fn send_json(stdin: &mut impl Write, value: Value) -> Result<(), String> {
    writeln!(stdin, "{}", serde_json::to_string(&value).unwrap())
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("MCP handshake write failed: {error}"))
}

fn receive_response(
    receiver: &mpsc::Receiver<Result<String, String>>,
    expected_id: i64,
    stage: &str,
) -> Result<Value, String> {
    let deadline = Instant::now() + MCP_RESPONSE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("MCP handshake timed out during {stage}."));
        }
        let line = receiver
            .recv_timeout(remaining)
            .map_err(|error| format!("MCP handshake timed out during {stage}: {error}"))??;
        let value: Value = serde_json::from_str(&line).map_err(|error| {
            format!("MCP handshake returned invalid JSON during {stage}: {error}")
        })?;
        if value["id"].as_i64() == Some(expected_id) {
            return Ok(value);
        }
        if value.get("method").is_some() {
            continue;
        }
    }
}

fn run_output(command: &mut Command, timeout: Duration) -> Result<std::process::Output, String> {
    let mut stdout_capture = CommandCapture::create("stdout")?;
    let mut stderr_capture = CommandCapture::create("stderr")?;
    command
        .stdout(stdout_capture.stdio()?)
        .stderr(stderr_capture.stdio()?);
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start command: {error}"))?;
    let status = wait_child(&mut child, timeout);
    let stdout = stdout_capture.read_all()?;
    let stderr = stderr_capture.read_all()?;
    let status = status.map_err(|error| with_stderr(error, &String::from_utf8_lossy(&stderr)))?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

struct CommandCapture {
    path: PathBuf,
    file: Option<File>,
}

impl CommandCapture {
    fn create(label: &str) -> Result<Self, String> {
        for _ in 0..64 {
            let sequence = CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "fastctx-doctor-{}-{sequence}-{label}.tmp",
                std::process::id()
            ));
            match OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "cannot create command capture file {}: {error}",
                        crate::paths::display_path(&path)
                    ));
                }
            }
        }
        Err("cannot allocate a unique command capture file".to_string())
    }

    fn stdio(&self) -> Result<Stdio, String> {
        self.file
            .as_ref()
            .ok_or_else(|| "command capture file is closed".to_string())?
            .try_clone()
            .map(Stdio::from)
            .map_err(|error| format!("cannot clone command capture file: {error}"))
    }

    fn read_all(&mut self) -> Result<Vec<u8>, String> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| "command capture file is closed".to_string())?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| format!("cannot rewind command capture: {error}"))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read command capture: {error}"))?;
        Ok(bytes)
    }
}

impl Drop for CommandCapture {
    fn drop(&mut self) {
        self.file.take();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn wait_child(child: &mut Child, timeout: Duration) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "command timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                terminate(child);
                return Err(format!("cannot wait for command: {error}"));
            }
        }
    }
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn join_with_timeout<T: Send + 'static>(
    handle: thread::JoinHandle<T>,
    timeout: Duration,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !handle.is_finished() {
        return Err("reader thread did not finish before the cleanup deadline".to_string());
    }
    handle
        .join()
        .map_err(|_| "reader thread panicked".to_string())
}

fn output_detail(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    } else {
        stdout
    }
}

fn with_stderr(message: String, stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        message
    } else {
        format!("{message} Server stderr: {stderr}")
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::{DoctorCheckStatus, check_drift, check_extension_state, receipt_drift, run};
    use crate::control::codex_config::{self, ExpectedConfig};
    use crate::control::paths::ControlPaths;
    use crate::control::settings::{
        AppliedRecord, FastCtxSettings, ManagedFileRecord, Tier, ToolBudgets,
    };

    #[test]
    fn receipt_drift_ignores_unowned_file_bytes_but_detects_paths_and_binary() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        std::fs::create_dir_all(&paths.fastctx_bin_dir).unwrap();
        let config = b"config";
        let agents = b"agents";
        let binary = b"binary";
        std::fs::write(&paths.codex_config, config).unwrap();
        std::fs::write(&paths.codex_agents, agents).unwrap();
        std::fs::write(&paths.installed_binary, binary).unwrap();
        let managed = |path: &std::path::Path, bytes: &[u8]| ManagedFileRecord {
            path: crate::paths::display_path(path),
            original_existed: true,
            applied_sha256: super::sha256(bytes),
        };
        let mut record = AppliedRecord {
            applied_at_utc: "2026-07-12T00:00:00Z".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: crate::paths::display_path(&paths.installed_binary),
            tier: Tier::Standard,
            tool_output_token_limit: 10_000,
            previous_token_limit_present: false,
            previous_token_limit: None,
            fastctx_token_budget: 8_500,
            tool_budgets: ToolBudgets::default(),
            fastshell_enabled: false,
            fastedit_enabled: false,
            codex_dir_created: false,
            codex_config: managed(&paths.codex_config, config),
            codex_agents: managed(&paths.codex_agents, agents),
            codex_agents_inserted_separator: None,
            binary_sha256: super::sha256(binary),
        };
        assert!(receipt_drift(&paths, &record).unwrap().is_empty());

        let drifted_config = b"config\n# unrelated user comment";
        std::fs::write(&paths.codex_config, drifted_config).unwrap();
        std::fs::write(&paths.codex_agents, b"user edit").unwrap();
        assert!(
            receipt_drift(&paths, &record).unwrap().is_empty(),
            "whole-file edits outside FastCtx ownership are checked semantically elsewhere"
        );

        record.codex_config.path = "wrong/config".to_string();
        record.codex_agents.path = "wrong/agents".to_string();
        record.command = "wrong/path".to_string();
        std::fs::write(&paths.installed_binary, b"changed binary").unwrap();
        let drift = receipt_drift(&paths, &record).unwrap();
        assert!(drift.contains(&"Codex config receipt path".to_string()));
        assert!(drift.contains(&"AGENTS receipt path".to_string()));
        assert!(drift.contains(&"installed binary receipt path".to_string()));
        assert!(drift.contains(&"installed binary content".to_string()));
    }

    #[test]
    fn applied_state_ignores_host_rewrites_but_detects_managed_semantic_drift() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        std::fs::create_dir_all(&paths.fastctx_bin_dir).unwrap();
        let binary = b"binary";
        let agents = b"agents";
        std::fs::write(&paths.installed_binary, binary).unwrap();
        std::fs::write(&paths.codex_agents, agents).unwrap();
        let expected = ExpectedConfig {
            command: crate::paths::display_path(&paths.installed_binary),
            tier: Tier::Standard,
            tool_budgets: ToolBudgets::default(),
            fastshell_enabled: false,
        };
        let applied_config = codex_config::apply(
            b"# host-owned heading\n[desktop]\nintegrated_terminal_shell = \"powershell\"\n",
            &expected,
        )
        .unwrap()
        .bytes;
        let managed = |path: &std::path::Path, bytes: &[u8]| ManagedFileRecord {
            path: crate::paths::display_path(path),
            original_existed: true,
            applied_sha256: super::sha256(bytes),
        };
        let record = AppliedRecord {
            applied_at_utc: "2026-07-17T00:00:00Z".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: expected.command.clone(),
            tier: Tier::Standard,
            tool_output_token_limit: 10_000,
            previous_token_limit_present: false,
            previous_token_limit: None,
            fastctx_token_budget: 8_500,
            tool_budgets: ToolBudgets::default(),
            fastshell_enabled: false,
            fastedit_enabled: false,
            codex_dir_created: false,
            codex_config: managed(&paths.codex_config, &applied_config),
            codex_agents: managed(&paths.codex_agents, agents),
            codex_agents_inserted_separator: None,
            binary_sha256: super::sha256(binary),
        };
        let settings = FastCtxSettings {
            applied: Some(record),
            ..FastCtxSettings::default()
        };
        let mut host_rewritten = b"# normalized by Codex\nhost_runtime_epoch = 7\n".to_vec();
        host_rewritten.extend_from_slice(&applied_config);
        host_rewritten
            .extend_from_slice(b"\n[plugins.runtime]\nlast_refresh = \"2026-07-17T00:01:00Z\"\n");

        let status = check_drift(&paths, Some(&settings), Some(&host_rewritten));
        assert_eq!(status.status, DoctorCheckStatus::Pass, "{status:?}");

        let managed_drift_source = String::from_utf8(host_rewritten).unwrap();
        assert!(
            managed_drift_source.contains("FASTCTX_TOKEN_BUDGET = \"8500\""),
            "{managed_drift_source}"
        );
        let managed_drift = managed_drift_source
            .replace(
                "FASTCTX_TOKEN_BUDGET = \"8500\"",
                "FASTCTX_TOKEN_BUDGET = \"1234\"",
            )
            .into_bytes();
        let status = check_drift(&paths, Some(&settings), Some(&managed_drift));
        assert_eq!(status.status, DoctorCheckStatus::Fail, "{status:?}");
        assert!(
            status
                .detail
                .contains("mcp_servers.fastctx.env.FASTCTX_TOKEN_BUDGET"),
            "{status:?}"
        );
    }

    #[test]
    fn fresh_profile_is_informational_and_does_not_require_any_codex_installation() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let report = run(&paths);

        assert!(report.passed(), "{report:?}");
        let job_limits = report
            .checks
            .iter()
            .find(|check| check.name == "Job limits")
            .expect("fresh profiles still expose the effective current-user limits");
        assert_eq!(job_limits.status, DoctorCheckStatus::Pass);
        assert!(
            job_limits.detail.contains(
                "1024 MiB retained job storage; 128 running jobs; 20 records per job_list page"
            ),
            "{job_limits:?}"
        );
        assert!(
            report
                .checks
                .iter()
                .filter(|check| check.name != "Job limits")
                .all(|check| check.status == DoctorCheckStatus::Info),
            "{report:?}"
        );
        assert!(
            report
                .checks
                .iter()
                .all(|check| !check.name.contains("host"))
        );
        assert!(report.checks.iter().any(
            |check| check.name == "Codex profile" && check.detail.contains("Apply will create")
        ));
    }

    #[test]
    fn invalid_current_user_job_limits_are_reported_with_effective_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        std::fs::write(
            &paths.fastctx_config,
            b"schema_version = 1\n\n[fastshell]\njob_storage_limit_mib = 0\nmax_running_jobs = -2\njob_list_limit = 101\n",
        )
        .unwrap();

        let report = run(&paths);
        let job_limits = report
            .checks
            .iter()
            .find(|check| check.name == "Job limits")
            .unwrap();
        assert_eq!(job_limits.status, DoctorCheckStatus::Info);
        assert!(
            job_limits
                .detail
                .contains("fastshell.job_storage_limit_mib"),
            "{job_limits:?}"
        );
        assert!(
            job_limits.detail.contains("fastshell.max_running_jobs"),
            "{job_limits:?}"
        );
        assert!(
            job_limits.detail.contains("fastshell.job_list_limit"),
            "{job_limits:?}"
        );
        assert!(
            job_limits.detail.contains(
                "1024 MiB retained job storage; 128 running jobs; 20 records per job_list page"
            ),
            "{job_limits:?}"
        );
        assert!(report.passed(), "{report:?}");
    }

    #[test]
    fn a_non_directory_codex_profile_is_an_actionable_failure() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::write(&paths.codex_dir, b"not a directory").unwrap();

        let report = run(&paths);
        let profile = report
            .checks
            .iter()
            .find(|check| check.name == "Codex profile")
            .unwrap();
        assert_eq!(profile.status, DoctorCheckStatus::Fail);
        assert!(profile.detail.contains("not a directory"));
        assert!(!report.passed());
    }

    #[test]
    fn shell_extension_checks_cover_disabled_pending_stale_and_applied_states() {
        let cases = [
            (false, false, DoctorCheckStatus::Info, "is disabled"),
            (true, false, DoctorCheckStatus::Info, "next Apply"),
            (false, true, DoctorCheckStatus::Info, "will remove it"),
            (true, true, DoctorCheckStatus::Pass, "enabled and applied"),
        ];
        for (desired, applied, status, phrase) in cases {
            let check = check_extension_state("fastshell", desired, applied);
            assert_eq!(check.status, status);
            assert!(check.detail.contains(phrase), "{check:?}");
        }
    }
}
