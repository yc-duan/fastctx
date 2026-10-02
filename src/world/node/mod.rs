//! `fastctx node run`: the daemon that is this machine's presence in the World and, while it
//! runs, its FastCtx control center.

pub(crate) mod activity;
pub(crate) mod admin;
pub(crate) mod executor;
pub(crate) mod inventory;
mod logging;
pub(crate) mod process;
#[cfg(windows)]
mod process_windows;
pub(crate) mod service;
mod service_identity;
mod service_lifetime;

use crate::control::paths::ControlPaths;
use crate::world::WorldPaths;
use crate::world::client::WorldClient;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const STATUS_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) fn log(message: impl std::fmt::Display) {
    let line = format!("[{}] node: {message}", crate::world::now_rfc3339());
    eprintln!("{line}");
    logging::write(&line);
}

/// Runs the daemon until interrupted.
pub(crate) async fn run_daemon(paths: ControlPaths, service_host: bool) -> Result<(), String> {
    logging::open(&WorldPaths::from_control(&paths))?;
    let result = async {
        let parent = service_lifetime::capture(service_host)?;
        run_daemon_inner(paths, parent).await
    }
    .await;
    if let Err(error) = &result {
        log(error);
    }
    result
}

async fn run_daemon_inner(
    paths: ControlPaths,
    parent: Option<service_lifetime::ServiceParent>,
) -> Result<(), String> {
    let world_paths = WorldPaths::from_control(&paths);
    let Some(client) = WorldClient::open(world_paths.clone())? else {
        return Err(format!(
            "not_enrolled: this machine is not enrolled in a World ({} does not exist). Run 'fastctx node enroll <invite>' or 'fastctx world init'.",
            crate::paths::display_path(&world_paths.config)
        ));
    };
    world_paths.ensure()?;
    let captured = crate::session::SessionEnvironment::capture()?;
    let mut variables = captured
        .variables()
        .iter()
        .filter(|(name, _)| !crate::session::environment_name_eq(name, "HOME"))
        .cloned()
        .collect::<Vec<_>>();
    variables.push(("HOME".into(), paths.home.as_os_str().to_os_string()));
    let environment = crate::session::SessionEnvironment::new(paths.home.clone(), variables);
    let session = crate::session::SessionContext::from_environment(environment.clone())?;
    let executor = executor::Executor::new(Arc::clone(&client), Arc::clone(&session));
    let town = crate::town::start(
        client.name(),
        world_paths.dir.clone(),
        paths.codex_dir.clone(),
        Arc::new(|line: String| log(format!("town: {line}"))),
    )?;
    let engine_hosted = Arc::new(AtomicBool::new(false));
    let admin = admin::AdminServer::new(
        Arc::clone(&client),
        Arc::clone(&executor),
        Arc::clone(&engine_hosted),
        Arc::clone(&town),
    );
    log(format!(
        "\"{}\" starting (World {}, key {})",
        client.name(),
        client.config.read().world_id,
        client.identity.fingerprint()
    ));

    let session_task = tokio::spawn(crate::world::session::run(
        Arc::clone(&client),
        Arc::clone(&executor),
    ));
    let admin_endpoint = crate::runtime::node_admin_endpoint(&environment)?;
    let admin_task = tokio::spawn(Arc::clone(&admin).run(admin_endpoint, client.shutdown.clone()));
    let status_task = {
        let admin = Arc::clone(&admin);
        let client = Arc::clone(&client);
        tokio::spawn(async move {
            loop {
                write_status(&admin, &client);
                tokio::select! {
                    () = client.shutdown.cancelled() => return,
                    () = tokio::time::sleep(STATUS_INTERVAL) => {}
                }
            }
        })
    };
    let engine_task = {
        let client = Arc::clone(&client);
        let hosted = Arc::clone(&engine_hosted);
        let environment = environment.clone();
        tokio::spawn(async move {
            let options = crate::runtime::EngineHostOptions {
                environment,
                world: Some(Arc::clone(&client)),
                town: Some(town),
                shutdown: client.shutdown.clone(),
                take_over: true,
                idle_timeout: None,
                maintenance_interval: None,
            };
            hosted.store(true, Ordering::Relaxed);
            let result = crate::runtime::host_engine(options).await;
            hosted.store(false, Ordering::Relaxed);
            if let Err(error) = &result {
                log(format!("the control center could not be hosted: {error}"));
            }
            result
        })
    };

    let service_error = tokio::select! {
        () = wait_for_shutdown_signal() => { log("shutdown requested"); None },
        result = service_lifetime::wait(parent) => match result {
            Ok(()) => { log("service host stopped"); None },
            Err(error) => Some(error),
        },
        () = client.shutdown.cancelled() => None,
        result = engine_task => {
            if let Ok(Err(error)) = result {
                log(format!("control center stopped: {error}"));
            }
            None
        }
    };
    client.shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), session_task).await;
    admin_task.abort();
    status_task.abort();
    write_status(&admin, &client);
    let _ = std::fs::remove_file(&client.paths.status);
    service_error.map_or(Ok(()), Err)
}

fn write_status(admin: &admin::AdminServer, client: &WorldClient) {
    let status = admin.status();
    if let Ok(json) = serde_json::to_vec_pretty(&status) {
        let _ = crate::world::write_atomic(&client.paths.status, &json);
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(terminate) => terminate,
            Err(_) => return std::future::pending::<()>().await,
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(windows)]
    {
        async fn event(receiver: std::io::Result<tokio::signal::windows::CtrlClose>) {
            match receiver {
                Ok(mut receiver) => {
                    receiver.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        }
        // Closing the headless service host sends a close event, not Ctrl-C (2026-09-06).
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            () = event(tokio::signal::windows::ctrl_close()) => {}
        }
    }
}

/// Reads the status file the daemon maintains; `Ok(None)` when none exists.
pub(crate) fn read_status(paths: &WorldPaths) -> Result<Option<admin::NodeStatus>, String> {
    let Some(bytes) = crate::world::read_optional(&paths.status)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("Cannot parse the node status file: {error}"))
}
