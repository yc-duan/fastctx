//! `fastctx node run`: the daemon that is this machine's presence in the World and, while it
//! runs, its FastCtx control center.

pub(crate) mod admin;
pub(crate) mod executor;
pub(crate) mod inventory;
pub(crate) mod service;

use crate::control::paths::ControlPaths;
use crate::world::WorldPaths;
use crate::world::client::WorldClient;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const STATUS_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) fn log(message: impl std::fmt::Display) {
    eprintln!("[{}] node: {message}", crate::world::now_rfc3339());
}

/// Runs the daemon until interrupted.
pub(crate) async fn run_daemon(paths: ControlPaths) -> Result<(), String> {
    let world_paths = WorldPaths::from_control(&paths);
    let Some(client) = WorldClient::open(world_paths.clone())? else {
        return Err(format!(
            "not_enrolled: this machine is not enrolled in a World ({} does not exist). Run 'fastctx node enroll <invite>' or 'fastctx world init'.",
            crate::paths::display_path(&world_paths.config)
        ));
    };
    world_paths.ensure()?;
    let environment = crate::session::SessionEnvironment::capture()?;
    let session = crate::session::SessionContext::from_environment(environment.clone())?;
    let executor = executor::Executor::new(Arc::clone(&client), Arc::clone(&session));
    let engine_hosted = Arc::new(AtomicBool::new(false));
    let admin = admin::AdminServer::new(
        Arc::clone(&client),
        Arc::clone(&executor),
        Arc::clone(&engine_hosted),
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

    tokio::select! {
        () = wait_for_shutdown_signal() => log("shutdown requested"),
        () = client.shutdown.cancelled() => {}
        result = engine_task => {
            if let Ok(Err(error)) = result {
                log(format!("control center stopped: {error}"));
            }
        }
    }
    client.shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(3), session_task).await;
    admin_task.abort();
    status_task.abort();
    write_status(&admin, &client);
    let _ = std::fs::remove_file(&client.paths.status);
    Ok(())
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
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
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
