//! `fastctx hub …`, `fastctx world …`, and `fastctx node …`: the World control commands.

use clap::Subcommand;
use std::path::PathBuf;
use std::process::ExitCode;

/// Hub-side commands, run on the machine that hosts the hub.
#[derive(Debug, Subcommand)]
pub enum HubCommand {
    /// Run the hub in the foreground until interrupted.
    Serve {
        /// Address to listen on; 443 looks like an ordinary web server (Linux needs `setcap`).
        #[arg(long, value_name = "HOST:PORT", default_value = "0.0.0.0:443")]
        listen: String,
        /// Hub data directory (database, key, certificate, status).
        #[arg(long, value_name = "DIR")]
        data: Option<PathBuf>,
        /// PEM certificate chain for a publicly trusted identity; with --key.
        #[arg(long, value_name = "FILE", requires = "key")]
        cert: Option<PathBuf>,
        /// PEM private key matching --cert.
        #[arg(long, value_name = "FILE", requires = "cert")]
        key: Option<PathBuf>,
        /// Serve plain HTTP for a reverse proxy or CDN that terminates TLS in front of the hub.
        #[arg(long, conflicts_with_all = ["cert", "key"])]
        behind_proxy: bool,
        /// Discard the unused bootstrap password and print a new one.
        #[arg(long)]
        reset_bootstrap: bool,
    },
    /// Show the running hub's status from its status file.
    Status {
        /// Hub data directory.
        #[arg(long, value_name = "DIR")]
        data: Option<PathBuf>,
    },
    /// Remove a member's admission. Key rotation completes on the next member that connects.
    Revoke {
        /// Member name.
        name: String,
        /// Hub data directory.
        #[arg(long, value_name = "DIR")]
        data: Option<PathBuf>,
    },
    /// Make the hub itself a member so a web AI can use the World through it.
    Join {
        /// Invite string from `fastctx world invite`.
        invite: String,
    },
}

fn hub_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let paths = crate::control::paths::ControlPaths::discover()?;
    Ok(paths.home.join(".fastctx-hub"))
}

pub async fn run_hub(command: HubCommand) -> Result<ExitCode, String> {
    match command {
        HubCommand::Serve {
            listen,
            data,
            cert,
            key,
            behind_proxy,
            reset_bootstrap,
        } => {
            let options = super::hub::HubOptions {
                listen,
                data: hub_data_dir(data)?,
                cert,
                key,
                behind_proxy,
                reset_bootstrap,
            };
            run_on_dedicated_runtime(super::hub::run(options)).await?;
            Ok(ExitCode::SUCCESS)
        }
        HubCommand::Status { data } => {
            let data = hub_data_dir(data)?;
            match super::hub::read_status(&data)? {
                None => {
                    println!(
                        "No hub is running from {} (no status file).",
                        crate::paths::display_path(&data)
                    );
                    Ok(ExitCode::FAILURE)
                }
                Some(status) => {
                    let live = super::hub::status_is_live(&status);
                    println!(
                        "{} hub {} (pid {}) on {}",
                        if live { "RUNNING" } else { "STALE" },
                        status.version,
                        status.pid,
                        status.listen
                    );
                    println!("World {}  hub key {}", status.world_id, status.hub_key);
                    println!("TLS: {}", status.tls);
                    println!("Binding: {}  started {}  status written {}", status.binding, status.started_at, status.written_at);
                    println!(
                        "Members: {} ({} online), open invites {}, events {}{}{}",
                        status.members.len(),
                        status.members.iter().filter(|member| member.state == "online").count(),
                        status.open_invites,
                        status.events,
                        if status.rotation_pending { ", key rotation pending" } else { "" },
                        if status.bootstrap_used { "" } else { ", waiting for the first member" }
                    );
                    for member in &status.members {
                        println!(
                            "  {:<32} {:<8} last seen {}  queued {}  fastctx {}",
                            member.name,
                            member.state,
                            if member.last_seen.is_empty() { "never" } else { &member.last_seen },
                            member.outbox,
                            if member.version.is_empty() { "?" } else { &member.version }
                        );
                    }
                    Ok(if live { ExitCode::SUCCESS } else { ExitCode::FAILURE })
                }
            }
        }
        HubCommand::Revoke { name, data } => {
            let data = hub_data_dir(data)?;
            println!("{}", super::hub::revoke_from_cli(&data, &name)?);
            Ok(ExitCode::SUCCESS)
        }
        HubCommand::Join { invite: _ } => Err(
            "fastctx hub join is not available in this version: the hub-as-member path ships with the web AI endpoint."
                .to_string(),
        ),
    }
}

/// Runs a long-lived server on its own multi-threaded runtime, outside the CLI's
/// single-threaded one, so store writes and TLS work never stall frame handling.
async fn run_on_dedicated_runtime<F>(future: F) -> Result<(), String>
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| format!("Cannot start the server runtime: {error}"))?;
        runtime.block_on(future)
    })
    .await
    .map_err(|error| format!("The server runtime stopped unexpectedly: {error}"))?
}
