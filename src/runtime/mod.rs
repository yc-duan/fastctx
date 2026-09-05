//! Thin stdio proxy and the per-user, per-build FastCtx control center.

pub(crate) mod activity;
mod hosts;
mod journal;
pub(crate) mod local_ipc;
mod protocol;
mod session;
#[cfg(windows)]
mod windows_process;

use crate::control::paths::ControlPaths;
use crate::file_executor::GrepGlobExecutor;
use crate::process_identity::ProcessIdentity;
use crate::server::{FastCtxServer, ServerOptions, SharedRuntime};
use crate::session::{SessionContext, SessionEnvironment};
use fs2::FileExt;
use hosts::HostRegistry;
use local_ipc::{BoxedStream, Listener, LocalEndpoint};
use rmcp::ServiceExt;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::split;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

pub(crate) use session::run_proxy_session;

/// How long a proxy waits for the control center before falling back to an in-process engine.
pub(crate) const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_RETRY: Duration = Duration::from_millis(20);
const ACCEPT_RETRY: Duration = Duration::from_secs(1);
/// How long a control center with nothing left to serve waits before exiting.
///
/// This timer only starts once every host that used the control center has exited, so in normal
/// use it measures the gap between closing the last Codex window and reclaiming the runtime.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60);
const SERVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a node daemon waits for a retiring control center to release the endpoint.
const TAKEOVER_TIMEOUT: Duration = Duration::from_secs(15);
/// Backstop for a closing proxy waiting on answers the control center already owes. A session that
/// ends normally never reaches it: the control center closes the connection once it has answered.
const RESPONSE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long work in progress may keep running after its client stopped reading. Long enough for a
/// finished handler to write its answer, short enough that nothing outlives the session by much.
const INPUT_CLOSED_GRACE: Duration = Duration::from_millis(250);
/// Buffer between the proxy and an in-process engine. Both directions are pumped by independent
/// tasks on either side, so this only bounds how far ahead a writer may run.
const IN_PROCESS_BUFFER_BYTES: usize = 256 * 1024;
#[cfg(unix)]
const MAX_UNIX_SOCKET_PATH_BYTES: usize = 100;

/// Captures the thin proxy's native state without loading settings or any heavy executor.
pub(crate) fn capture_proxy_environment() -> Result<SessionEnvironment, String> {
    SessionEnvironment::capture()
}

/// Connects to the matching control center or starts exactly one.
pub(crate) async fn connect_or_start(
    options: ServerOptions,
    environment: &SessionEnvironment,
    host: Option<ProcessIdentity>,
) -> Result<BoxedStream, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the running fastctx binary: {error}"))?;
    let endpoint = endpoint_for(environment)?;
    crate::edit::private_storage::ensure_private_directory(
        endpoint.runtime_directory(),
        "control-center runtime",
    )?;

    if let Ok(mut stream) = local_ipc::connect(&endpoint).await {
        establish(&mut stream, options, environment.clone(), host.clone()).await?;
        return Ok(stream);
    }

    let startup_lock = crate::edit::private_storage::open_lock_file(
        &endpoint.startup_lock_path(),
        "control-center startup lock",
    )?;
    acquire_startup_lock(&startup_lock, &endpoint.startup_lock_path()).await?;

    if let Ok(mut stream) = local_ipc::connect(&endpoint).await {
        establish(&mut stream, options, environment.clone(), host.clone()).await?;
        return Ok(stream);
    }

    spawn_bootstrap(&executable, environment)?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        match local_ipc::connect(&endpoint).await {
            Ok(mut stream) => {
                establish(&mut stream, options, environment.clone(), host).await?;
                return Ok(stream);
            }
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(CONNECT_RETRY).await;
            }
            Err(error) => {
                return Err(format!(
                    "The FastCtx control center did not become ready within 10 seconds: {error}"
                ));
            }
        }
    }
}

/// Runs a control-center session inside the proxy itself.
///
/// This is the engine of last resort. It costs this session its own search executor instead of a
/// shared one, which is exactly the cost the shared control center exists to avoid — but a session
/// that cannot reach the shared runtime has only two remaining options, and losing the host's MCP
/// transport is the worse one.
async fn start_in_process(
    options: ServerOptions,
    environment: &SessionEnvironment,
    host: Option<ProcessIdentity>,
) -> Result<BoxedStream, String> {
    let (proxy_side, engine_side) = tokio::io::duplex(IN_PROCESS_BUFFER_BYTES);
    let state = HostState::new(None, false);
    let connection = state
        .activity
        .try_connection()
        .ok_or_else(|| "The in-process FastCtx engine refused its own session.".to_string())?;
    tokio::spawn(serve_connection(
        Box::new(engine_side) as BoxedStream,
        state,
        CancellationToken::new(),
        connection,
    ));
    let mut stream = Box::new(proxy_side) as BoxedStream;
    establish(&mut stream, options, environment.clone(), host).await?;
    Ok(stream)
}

async fn establish(
    stream: &mut BoxedStream,
    options: ServerOptions,
    environment: SessionEnvironment,
    host: Option<ProcessIdentity>,
) -> Result<(), String> {
    tokio::time::timeout(STARTUP_TIMEOUT, async {
        protocol::write_handshake(
            stream,
            &protocol::Handshake::new(options, environment, host),
        )
        .await?;
        protocol::read_handshake_response(stream).await
    })
    .await
    .map_err(|_| {
        "Timed out waiting for the FastCtx control center to accept the session handshake."
            .to_string()
    })?
}

async fn acquire_startup_lock(file: &File, path: &Path) -> Result<(), String> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                tokio::time::sleep(CONNECT_RETRY).await;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(
                    "Timed out waiting for another FastCtx proxy to finish starting the control center."
                        .to_string(),
                );
            }
            Err(error) => {
                return Err(format!(
                    "Cannot lock the control-center startup gate {}: {error}",
                    crate::paths::display_path(path)
                ));
            }
        }
    }
}

fn endpoint_for(environment: &SessionEnvironment) -> Result<LocalEndpoint, String> {
    endpoint_named(environment, "fastctx-engine")
}

/// The node daemon's local admin endpoint for the same user and build.
pub(crate) fn node_admin_endpoint(
    environment: &SessionEnvironment,
) -> Result<LocalEndpoint, String> {
    endpoint_named(environment, "fastctx-node")
}

fn endpoint_named(environment: &SessionEnvironment, prefix: &str) -> Result<LocalEndpoint, String> {
    let home = environment
        .var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| {
            environment
                .var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| {
            "Cannot determine the user home directory for the FastCtx control center. Set HOME or USERPROFILE and retry."
                .to_string()
        })?;
    let home_hash = short_hash(&endpoint_home_key(&home), 12);
    let build_id = effective_build_id(environment);
    let id = format!("{prefix}-{home_hash}-{build_id}");
    let preferred_runtime_directory = crate::edit::private_storage::control_center_directory();
    #[cfg(unix)]
    let runtime_directory = select_unix_runtime_directory(
        preferred_runtime_directory,
        crate::edit::private_storage::short_control_center_directory(),
        &id,
    )?;
    #[cfg(not(unix))]
    let runtime_directory = preferred_runtime_directory;
    Ok(LocalEndpoint::new(runtime_directory, id))
}

#[cfg(unix)]
fn select_unix_runtime_directory(
    preferred: std::path::PathBuf,
    fallback: std::path::PathBuf,
    id: &str,
) -> Result<std::path::PathBuf, String> {
    use std::os::unix::ffi::OsStrExt;

    let socket_length = |directory: &Path| {
        directory
            .join(format!("{id}.sock"))
            .as_os_str()
            .as_bytes()
            .len()
            .saturating_add(1)
    };
    let fits = |directory: &Path| socket_length(directory) <= MAX_UNIX_SOCKET_PATH_BYTES;
    // A server this user is already running wins over the directory this process would have
    // picked. `XDG_RUNTIME_DIR` is exported to login sessions and user services and missing
    // from cron jobs and from anything a system service starts, so a daemon and the shell that
    // looks for it can disagree about where the endpoint lives — the node daemon then answers
    // nobody, and two directories for one user mean two control centers, which is exactly what
    // "one control center per user" forbids. The endpoint name is the identity; the directory
    // is only where it was bound, so search every place this endpoint could already be serving
    // before binding a new one.
    let conventional = crate::edit::private_storage::conventional_control_center_directory();
    for candidate in [Some(&preferred), Some(&fallback), conventional.as_ref()]
        .into_iter()
        .flatten()
    {
        if fits(candidate.as_path()) && unix_endpoint_is_live(candidate.as_path(), id) {
            return Ok(candidate.to_path_buf());
        }
    }
    if fits(&preferred) {
        return Ok(preferred);
    }
    // Darwin's per-user temporary directory can already consume most of sockaddr_un::sun_path.
    // Keep the endpoint and both ownership locks together in an owner-only short directory.
    if fits(&fallback) {
        return Ok(fallback);
    }
    Err(format!(
        "The private control-center socket path is too long ({} bytes): {}",
        socket_length(&fallback),
        crate::paths::display_path(&fallback.join(format!("{id}.sock")))
    ))
}

/// Whether a server with this endpoint name is running out of `directory`.
///
/// Two signs, because the two kinds of server that bind here are not built the same. A control
/// center holds its instance lock for as long as it lives, so a lock that cannot be taken is a
/// live center. The node's admin channel takes no lock at all, so for it the only honest test is
/// the socket: a listener accepts the connection, while a socket file left behind by a dead
/// process refuses it. Testing only the lock would answer "nothing here" for every node endpoint
/// and send the search back to the directory this process would have picked on its own — which
/// is the disagreement the search exists to settle.
#[cfg(unix)]
fn unix_endpoint_is_live(directory: &Path, id: &str) -> bool {
    let lock = directory.join(format!("{id}.instance.lock"));
    if let Ok(file) = std::fs::File::open(&lock) {
        match file.try_lock_exclusive() {
            Ok(()) => {
                let _ = fs2::FileExt::unlock(&file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return true,
            Err(_) => {}
        }
    }
    std::os::unix::net::UnixStream::connect(directory.join(format!("{id}.sock"))).is_ok()
}

/// The bytes that identify a home directory in an endpoint name.
///
/// Shells hand the same directory over in different spellings — Git Bash `C:/Users/x`,
/// PowerShell `C:\Users\x`, a trailing separator from a profile script — and hashing the raw
/// value would give each spelling its own control center, so a node daemon started from one
/// shell would be invisible to a session started from another. The canonical path is the
/// directory's identity; the lexical form is the fallback when it cannot be resolved.
fn endpoint_home_key(home: &std::ffi::OsStr) -> Vec<u8> {
    match std::fs::canonicalize(home) {
        Ok(path) => crate::session::native_bytes(path.as_os_str()),
        Err(_) => crate::session::native_bytes(&lexical_home(home)),
    }
}

fn lexical_home(home: &std::ffi::OsStr) -> std::ffi::OsString {
    let Some(text) = home.to_str() else {
        return home.to_os_string();
    };
    let mut normalized: String = if cfg!(windows) {
        text.replace('/', "\\")
    } else {
        text.to_string()
    };
    let separator = if cfg!(windows) { '\\' } else { '/' };
    while normalized.len() > 1 && normalized.ends_with(separator) {
        normalized.pop();
    }
    if cfg!(windows) {
        let mut characters = normalized.chars();
        if let (Some(drive), Some(':')) = (characters.next(), characters.next())
            && drive.is_ascii_alphabetic()
        {
            normalized.replace_range(..1, &drive.to_ascii_uppercase().to_string());
        }
    }
    std::ffi::OsString::from(normalized)
}

fn short_hash(bytes: &[u8], characters: usize) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)[..characters].to_string()
}

fn effective_build_id(_environment: &SessionEnvironment) -> String {
    #[cfg(debug_assertions)]
    if let Ok(override_id) = _environment.var("FASTCTX_TEST_BUILD_ID")
        && !override_id.is_empty()
        && override_id.len() <= 32
        && override_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return override_id;
    }
    env!("FASTCTX_BUILD_ID").to_string()
}

fn spawn_bootstrap(executable: &Path, environment: &SessionEnvironment) -> Result<(), String> {
    let mut command = Command::new(executable);
    environment.configure_command(&mut command);
    command
        .arg("runtime-bootstrap")
        .current_dir(environment.cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::process_policy::noninteractive_creation_flags(0));
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Cannot start the FastCtx control-center bootstrap: {error}"))
}

/// Intermediate child that reparents the long-lived control center before the proxy returns.
pub(crate) fn run_bootstrap_entry() -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the control-center binary: {error}"))?;
    let environment = SessionEnvironment::capture()?;
    let mut command = Command::new(&executable);
    environment.configure_command(&mut command);
    command
        .arg("runtime-host")
        .current_dir(environment.cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: pre_exec performs only the async-signal-safe setsid syscall.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        };
        let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        match windows_process::spawn_without_inherited_handles(
            &executable,
            environment.cwd(),
            detached | CREATE_BREAKAWAY_FROM_JOB,
        ) {
            Ok(_) => Ok(()),
            Err(error) if error.raw_os_error() == Some(5) => {
                windows_process::spawn_without_inherited_handles(
                    &executable,
                    environment.cwd(),
                    detached,
                )
                .map_err(|error| format!("Cannot detach the FastCtx control center: {error}"))
            }
            Err(error) => Err(format!("Cannot detach the FastCtx control center: {error}")),
        }
    }
    #[cfg(not(windows))]
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Cannot detach the FastCtx control center: {error}"))
}

struct HostState {
    runtime: OnceCell<Arc<SharedRuntime>>,
    control_paths: OnceCell<ControlPaths>,
    activity: Arc<activity::RuntimeActivity>,
    hosts: Arc<HostRegistry>,
    /// The World client when the node daemon hosts this control center.
    world: Option<Arc<crate::world::client::WorldClient>>,
    /// Whether a `retire` handshake may shut this host down. The node daemon's engine is
    /// managed by the service manager and never steps aside on request.
    retirable: bool,
}

impl HostState {
    fn new(world: Option<Arc<crate::world::client::WorldClient>>, retirable: bool) -> Arc<Self> {
        Arc::new(Self {
            runtime: OnceCell::new(),
            control_paths: OnceCell::new(),
            activity: activity::RuntimeActivity::new(),
            hosts: Arc::new(HostRegistry::new()),
            world,
            retirable,
        })
    }

    async fn runtime_for(
        &self,
        session: &Arc<SessionContext>,
    ) -> Result<Arc<SharedRuntime>, String> {
        let runtime = self
            .runtime
            .get_or_try_init(|| async {
                let parallelism = session.settings.search_parallelism().map_err(|error| {
                    format!(
                        "Cannot start the MCP session with settings from {}: {error}. Repair the value and retry.",
                        crate::paths::display_path(&session.control_paths.fastctx_config)
                    )
                })?;
                let executor = Arc::new(GrepGlobExecutor::with_parallelism(parallelism.effective));
                Ok::<_, String>(SharedRuntime::with_activity(
                    executor,
                    Arc::clone(&self.activity),
                    self.world.clone(),
                ))
            })
            .await?;
        let _ = self.control_paths.set(session.control_paths.clone());
        Ok(Arc::clone(runtime))
    }
}

/// How a control center is hosted: as the ordinary detached process, or inside the node daemon.
pub(crate) struct EngineHostOptions {
    pub(crate) environment: SessionEnvironment,
    pub(crate) world: Option<Arc<crate::world::client::WorldClient>>,
    pub(crate) shutdown: CancellationToken,
    /// Ask a control center already holding the endpoint to retire, then take its place.
    pub(crate) take_over: bool,
    /// Exit after this much idleness; `None` never exits on idleness.
    pub(crate) idle_timeout: Option<Duration>,
    pub(crate) maintenance_interval: Option<Duration>,
}

/// Final detached control-center entry point.
pub(crate) async fn run_host_entry(
    idle_timeout_ms: Option<u64>,
    maintenance_interval_ms: Option<u64>,
) -> Result<(), String> {
    let environment = SessionEnvironment::capture()?;
    let idle_timeout = idle_timeout_ms
        .or_else(|| duration_override(&environment, "FASTCTX_TEST_RUNTIME_IDLE_MS"))
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_IDLE_TIMEOUT);
    let maintenance_interval = maintenance_interval_ms
        .or_else(|| duration_override(&environment, "FASTCTX_TEST_RUNTIME_MAINTENANCE_MS"))
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_MAINTENANCE_INTERVAL);
    host_engine(EngineHostOptions {
        environment,
        world: None,
        shutdown: CancellationToken::new(),
        take_over: false,
        idle_timeout: Some(idle_timeout),
        maintenance_interval: Some(maintenance_interval),
    })
    .await
}

/// Hosts the per-user control center on its endpoint until shutdown or idleness.
pub(crate) async fn host_engine(options: EngineHostOptions) -> Result<(), String> {
    let EngineHostOptions {
        environment,
        world,
        shutdown,
        take_over,
        idle_timeout,
        maintenance_interval,
    } = options;
    let endpoint = endpoint_for(&environment)?;
    crate::edit::private_storage::ensure_private_directory(
        endpoint.runtime_directory(),
        "control-center runtime",
    )?;
    let instance_lock = crate::edit::private_storage::open_lock_file(
        &endpoint.instance_lock_path(),
        "control-center instance lock",
    )?;
    // A takeover holds the startup gate so proxies that lose the retiring center wait for
    // this one instead of bootstrapping another plain control center in the gap.
    let startup_lock = if take_over {
        let lock = crate::edit::private_storage::open_lock_file(
            &endpoint.startup_lock_path(),
            "control-center startup lock",
        )?;
        acquire_startup_lock(&lock, &endpoint.startup_lock_path()).await?;
        Some(lock)
    } else {
        None
    };
    match instance_lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            if !take_over {
                return Ok(());
            }
            retire_existing(&endpoint, &environment).await?;
            let deadline = Instant::now() + TAKEOVER_TIMEOUT;
            loop {
                match instance_lock.try_lock_exclusive() {
                    Ok(()) => break,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(error) => {
                        return Err(format!(
                            "The running FastCtx control center did not release the endpoint within {} s: {error}",
                            TAKEOVER_TIMEOUT.as_secs()
                        ));
                    }
                }
            }
        }
        Err(error) => {
            return Err(format!(
                "Cannot lock the control-center instance gate: {error}"
            ));
        }
    }
    let mut listener = Listener::bind(&endpoint)?;
    drop(startup_lock);
    #[cfg(debug_assertions)]
    record_test_host_start(&environment);
    let state = HostState::new(world, !take_over);
    let (idle_candidate_tx, mut idle_candidate_rx) = tokio::sync::mpsc::channel(1);
    let monitor = idle_timeout.map(|idle_timeout| {
        tokio::spawn(monitor_idle(
            Arc::clone(&state),
            shutdown.clone(),
            idle_timeout,
            idle_candidate_tx,
        ))
    });
    let maintenance = tokio::spawn(monitor_maintenance(
        Arc::clone(&state),
        shutdown.clone(),
        maintenance_interval.unwrap_or(DEFAULT_MAINTENANCE_INTERVAL),
    ));
    let mut connections = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok(stream) => {
                        let Some(connection) = state.activity.try_connection() else {
                            drop(stream);
                            continue;
                        };
                        connections.spawn(serve_connection(
                            stream,
                            Arc::clone(&state),
                            shutdown.clone(),
                            connection,
                        ));
                    }
                    Err(error) => {
                        eprintln!("fastctx control center: {error}; retrying.");
                        tokio::select! {
                            () = shutdown.cancelled() => break,
                            () = tokio::time::sleep(ACCEPT_RETRY) => {}
                        }
                    }
                }
            }
            Some(()) = idle_candidate_rx.recv() => {
                if let Some(idle_timeout) = idle_timeout
                    && state.activity.try_begin_shutdown(idle_timeout)
                {
                    shutdown.cancel();
                    break;
                }
            }
            () = shutdown.cancelled() => break,
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    eprintln!("fastctx control center: connection task failed: {error}");
                }
            }
        }
    }

    shutdown.cancel();
    let deadline = tokio::time::sleep(SERVICE_SHUTDOWN_TIMEOUT);
    tokio::pin!(deadline);
    // Drain only while connections remain. The endpoint stays bound until this function returns,
    // and a proxy that connects during the wait is never accepted: it stalls, then degrades to a
    // standalone server. An always-enabled deadline branch would make every exit wait in full.
    while !connections.is_empty() {
        tokio::select! {
            _ = &mut deadline => {
                connections.abort_all();
                while connections.join_next().await.is_some() {}
                break;
            }
            result = connections.join_next() => {
                if result.is_none() {
                    break;
                }
            }
        }
    }
    if let Some(monitor) = monitor {
        monitor.abort();
        let _ = monitor.await;
    }
    maintenance.abort();
    let _ = maintenance.await;
    drop(instance_lock);
    Ok(())
}

/// Asks the control center holding the endpoint to shut down so a node daemon can host it.
async fn retire_existing(
    endpoint: &LocalEndpoint,
    environment: &SessionEnvironment,
) -> Result<(), String> {
    let mut stream = match local_ipc::connect(endpoint).await {
        Ok(stream) => stream,
        // The lock holder is not listening: it is starting or dying, and the lock wait covers both.
        Err(_) => return Ok(()),
    };
    tokio::time::timeout(STARTUP_TIMEOUT, async {
        protocol::write_handshake(
            &mut stream,
            &protocol::Handshake::retire(environment.clone()),
        )
        .await?;
        protocol::read_handshake_response(&mut stream).await
    })
    .await
    .map_err(|_| "Timed out asking the running FastCtx control center to step aside.".to_string())?
}

/// Reads a millisecond timer override for the control center's own loops.
///
/// Deliberately honoured in every profile. While this was `debug_assertions`-only, one release
/// test run left sixty-five control centers holding the production ten-minute timeout, which is
/// the very process pile-up this runtime exists to remove. Both timers only shorten the host's
/// own life, so a value from the environment cannot outlive or override a caller's session.
fn duration_override(environment: &SessionEnvironment, name: &str) -> Option<u64> {
    environment.var(name).ok()?.parse::<u64>().ok()
}

#[cfg(debug_assertions)]
fn record_test_host_start(environment: &SessionEnvironment) {
    use std::io::Write as _;
    use std::path::PathBuf;
    let Some(path) = environment.var_os("FASTCTX_TEST_RUNTIME_EVENT_LOG") else {
        return;
    };
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(PathBuf::from(path)) {
        let _ = writeln!(file, "START {}", std::process::id());
    }
}

async fn monitor_maintenance(
    state: Arc<HostState>,
    shutdown: CancellationToken,
    interval: Duration,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(interval) => {}
        }
        let Some(paths) = state.control_paths.get().cloned() else {
            continue;
        };
        match tokio::task::spawn_blocking(move || crate::shell::jobs::reap_history(&paths)).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                eprintln!("fastctx control center: periodic background-job cleanup failed: {error}")
            }
            Err(error) => eprintln!(
                "fastctx control center: periodic background-job cleanup task failed: {error}"
            ),
        }
    }
}

async fn serve_connection(
    mut stream: BoxedStream,
    state: Arc<HostState>,
    shutdown: CancellationToken,
    _connection: activity::ConnectionActivityGuard,
) {
    let handshake = match protocol::read_handshake(&mut stream).await {
        Ok(handshake) => handshake,
        Err(error) => {
            let _ = protocol::write_handshake_error(&mut stream, error).await;
            return;
        }
    };
    if handshake.retire {
        if state.retirable {
            let _ = protocol::write_handshake_success(&mut stream).await;
            shutdown.cancel();
        } else {
            let _ = protocol::write_handshake_error(
                &mut stream,
                "This control center is hosted by the FastCtx node service and does not step aside; stop the service instead."
                    .to_string(),
            )
            .await;
        }
        return;
    }
    let session = match SessionContext::from_environment(handshake.environment) {
        Ok(session) => session,
        Err(error) => {
            let _ = protocol::write_handshake_error(&mut stream, error).await;
            return;
        }
    };
    // Recorded before the session runs, so a host that connects once keeps the runtime warm for
    // every later conversation it opens, not only while this connection lasts.
    state.hosts.remember(handshake.host);
    let runtime = match state.runtime_for(&session).await {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = protocol::write_handshake_error(&mut stream, error).await;
            return;
        }
    };
    if protocol::write_handshake_success(&mut stream)
        .await
        .is_err()
    {
        return;
    }
    let (mut reader, writer) = split(stream);
    // Requests arrive framed so the proxy can mark the end of input; the MCP server reads the
    // plain stream that comes back out, and the sink closing is what it sees as EOF.
    let (requests, mut request_sink) = tokio::io::simplex(protocol::MAX_REQUEST_FRAME_BYTES);
    let mut unframe = tokio::spawn(async move {
        if let Err(error) = protocol::receive_requests(&mut reader, &mut request_sink).await {
            eprintln!("fastctx control center: {error}");
        }
        drop(request_sink);
    });
    let service = match FastCtxServer::with_session_and_runtime(handshake.options, session, runtime)
        .serve((requests, writer))
        .await
    {
        Ok(service) => service,
        Err(error) => {
            eprintln!("fastctx control center: cannot start MCP connection: {error}");
            unframe.abort();
            return;
        }
    };
    let cancellation = service.cancellation_token();
    let mut waiting = tokio::spawn(service.waiting());
    tokio::select! {
        _ = shutdown.cancelled() => {
            cancellation.cancel();
            end_service(&mut waiting).await;
        }
        _ = &mut unframe => {
            // The client is gone. Work that already finished still gets its answer written, but
            // an MCP server left holding the transport would keep running a request nobody can
            // read — the reason a closed stdin has to end foreground work rather than outlive it.
            if tokio::time::timeout(INPUT_CLOSED_GRACE, &mut waiting).await.is_err() {
                cancellation.cancel();
                end_service(&mut waiting).await;
            }
        }
        _ = &mut waiting => {}
    }
    unframe.abort();
}

/// Waits out a cancelled MCP service, then stops waiting.
async fn end_service<T>(waiting: &mut tokio::task::JoinHandle<T>) {
    if tokio::time::timeout(SERVICE_SHUTDOWN_TIMEOUT, &mut *waiting)
        .await
        .is_err()
    {
        waiting.abort();
        let _ = waiting.await;
    }
}

async fn monitor_idle(
    state: Arc<HostState>,
    shutdown: CancellationToken,
    idle_timeout: Duration,
    candidate: tokio::sync::mpsc::Sender<()>,
) {
    let interval = idle_timeout
        .div_f64(4.0)
        .clamp(Duration::from_millis(50), Duration::from_secs(30));
    // First instant of the current unbroken run of registry-scan failures, if any.
    let mut scan_failing_since: Option<Instant> = None;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(interval) => {}
        }
        if !state.activity.is_shutdown_eligible(idle_timeout) {
            scan_failing_since = None;
            continue;
        }
        // A host that is still running can open another conversation at any moment, and a host
        // whose stdio MCP server has died never gets it back. Staying resident is the cheap side
        // of that trade, so the idle timer does not even start while one of them is alive.
        let hosts = Arc::clone(&state.hosts);
        let hosts_alive = tokio::task::spawn_blocking(move || hosts.prune_and_check())
            .await
            .unwrap_or(true);
        if hosts_alive {
            scan_failing_since = None;
            continue;
        }
        let Some(paths) = state.control_paths.get().cloned() else {
            if candidate.send(()).await.is_err() {
                return;
            }
            continue;
        };
        let running = tokio::task::spawn_blocking(move || {
            crate::shell::jobs::running_summaries(&paths).map(|jobs| !jobs.is_empty())
        })
        .await
        .unwrap_or_else(|error| Err(format!("the inspection task failed: {error}")));
        match running {
            Ok(false) => {
                scan_failing_since = None;
                // A request may have arrived while the registry scan ran off-thread.
                if !state.activity.is_shutdown_eligible(idle_timeout) {
                    continue;
                }
                if candidate.send(()).await.is_err() {
                    return;
                }
            }
            Ok(true) => scan_failing_since = None,
            Err(error) => {
                eprintln!(
                    "fastctx control center: cannot inspect running jobs for idle shutdown: {error}"
                );
                // Fail open once scans have failed for a full idle window: one damaged registry
                // record fails every scan, and a host that insists on a clean scan before exiting
                // would never exit. Exiting is safe — job supervisors are detached processes and
                // the next connection bootstraps a fresh host that reads the same on-disk registry.
                let failing_since = *scan_failing_since.get_or_insert_with(Instant::now);
                if failing_since.elapsed() >= idle_timeout {
                    eprintln!(
                        "fastctx control center: registry scans kept failing for a full idle window; shutting down anyway."
                    );
                    if candidate.send(()).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::short_hash;

    #[test]
    fn endpoint_hashes_are_stable_and_separate_inputs() {
        assert_eq!(short_hash(b"home-a", 12), short_hash(b"home-a", 12));
        assert_ne!(short_hash(b"home-a", 12), short_hash(b"home-b", 12));
    }

    #[test]
    fn one_home_directory_has_one_endpoint_whatever_spelling_the_shell_uses() {
        use super::{endpoint_home_key, lexical_home};
        use std::ffi::OsStr;

        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().to_str().unwrap().to_string();
        let mut spellings = vec![real.clone(), format!("{real}{}", std::path::MAIN_SEPARATOR)];
        if cfg!(windows) {
            spellings.push(real.replace('\\', "/"));
            let mut lowered = real.clone();
            lowered.replace_range(..1, &real[..1].to_ascii_lowercase());
            spellings.push(lowered);
        }
        let keys = spellings
            .iter()
            .map(|spelling| endpoint_home_key(OsStr::new(spelling)))
            .collect::<Vec<_>>();
        assert!(keys.iter().all(|key| *key == keys[0]), "{spellings:?}");

        // A directory that does not exist still normalizes lexically.
        let missing = if cfg!(windows) {
            ("c:/no/such/dir/", "C:\\no\\such\\dir")
        } else {
            ("/no/such/dir/", "/no/such/dir")
        };
        assert_eq!(lexical_home(OsStr::new(missing.0)), OsStr::new(missing.1));
    }

    #[cfg(unix)]
    #[test]
    fn long_unix_runtime_paths_fall_back_to_the_short_private_directory() {
        use super::select_unix_runtime_directory;
        use std::path::PathBuf;

        let preferred = PathBuf::from("/").join("long".repeat(30));
        let fallback = PathBuf::from("/tmp/fastctx-engine-1000");
        let selected = select_unix_runtime_directory(
            preferred,
            fallback.clone(),
            "fastctx-engine-0123456789ab-0123456789abcdef",
        )
        .unwrap();
        assert_eq!(selected, fallback);
    }
}
