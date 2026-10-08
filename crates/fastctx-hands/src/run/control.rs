//! Same-user local requests to a job's supervisor. A stop is complete only
//! after the tree is empty, both output streams have ended, and the final
//! record is published. The supervisor then closes the request connection;
//! the caller reads that record. No reply bytes need draining before the
//! supervisor exits, and a client that stops reading cannot keep it alive.
//! Disconnecting a requester never cancels an accepted stop.

use std::io;
use std::path::Path;
use std::time::Duration;

use fastctx_platform::ipc::{self, Connection, Endpoint, Listener};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::run::{JobRecord, StopRequest};
use fastctx_types::{JobId, ProcessIdentity};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use super::record;

#[derive(Serialize, Deserialize)]
struct Request {
    job: JobId,
    stop: StopRequest,
}

/// Stops a job through its supervisor and returns the durable final record.
/// `starting` is Launched::supervisor when called before the supervisor has
/// published its identity; otherwise pass None. A terminal job is returned
/// unchanged, including its original stopper. The node supplies attribution
/// in `request` after authenticating its own caller.
/// Inspect the returned end.how: an already finished job stays Finished,
/// and a failure while stopping is recorded as Failed, never Stopped.
///
/// No node or job-state lock may be held across this await: the supervisor
/// publishes JobChanged through the node while it completes the stop.
/// If completion closes the endpoint concurrently, the final disk record
/// is authoritative. Otherwise a broken connection is an error; never
/// resend a stop merely because its response was lost.
pub async fn stop(
    world: &WorldDir,
    path: &Path,
    starting: Option<&ProcessIdentity>,
    request: StopRequest,
) -> Result<JobRecord, Failure> {
    validate(&request)?;
    let mut job = record::read(path).map_err(failure)?;
    let mut connection = loop {
        if job.end.is_some() {
            return Ok(job);
        }
        let published = job.supervisor.is_some();
        let identity = job.supervisor.as_ref().or(starting).ok_or_else(|| {
            Failure::new(
                "the supervisor has not published its identity; pass Launched::supervisor when stopping during startup",
            )
        })?;
        let endpoint = Endpoint::for_supervisor(world.path(), identity.pid).map_err(failure)?;
        match ipc::connect(&endpoint).await {
            Ok(connection) => {
                if connection.server_process() != Some(identity) {
                    return Err(Failure::new("the stop endpoint is not held by this job's supervisor"));
                }
                break connection;
            }
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
                let alive = fastctx_platform::is_alive(identity).map_err(failure)?;
                job = record::read(path).map_err(failure)?;
                if job.end.is_some() {
                    return Ok(job);
                }
                if !alive {
                    return Err(Failure::new("the supervisor ended without finishing the job's record"));
                }
                // Binding precedes publication of the supervisor identity.
                // A published supervisor without an endpoint may be an old
                // binary; waiting for that live process would never finish.
                if published {
                    return Err(Failure::new(
                        "this supervisor is not accepting stop requests; its executable may predate job control",
                    ));
                }
                // Wait only for the known starter to publish its endpoint.
                tokio::time::sleep(Duration::from_millis(1)).await;
                job = record::read(path).map_err(failure)?;
            }
            Err(error) => return Err(failure(error)),
        }
    };
    let bytes = serde_json::to_vec(&Request { job: job.id, stop: request }).map_err(failure)?;
    let response = async {
        connection.send(&bytes).await?;
        connection.recv().await
    }
    .await;
    match response {
        Ok(Some(_)) => Err(Failure::new("the supervisor sent an unexpected stop response")),
        completed => {
            let final_record = record::read(path).map_err(failure)?;
            if final_record.end.is_some() {
                Ok(final_record)
            } else {
                let error = completed.err().unwrap_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof));
                Err(Failure::new(format!(
                    "the supervisor connection broke before answering the stop: {error}; read the job's record to learn its outcome"
                )))
            }
        }
    }
}

pub(super) struct Server {
    listener: Listener,
    reading: JoinSet<Option<(Connection, StopRequest)>>,
    job: JobId,
}

impl Server {
    pub fn bind(world: &WorldDir, job: &JobRecord) -> io::Result<Self> {
        let endpoint = Endpoint::for_supervisor(world.path(), std::process::id())?;
        Ok(Self { listener: Listener::bind(&endpoint)?, reading: JoinSet::new(), job: job.id.clone() })
    }

    /// Concurrent reads let an incomplete request coexist with real stops.
    /// Dropping the server cancels those reads when supervision ends.
    pub async fn next(&mut self) -> io::Result<(Connection, StopRequest)> {
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    match accepted {
                        Ok(connection) => {
                            self.reading.spawn(read(connection, self.job.clone()));
                        }
                        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                            eprintln!("refused supervisor connection: {error}");
                        }
                        Err(error) => return Err(error),
                    }
                }
                ready = self.reading.join_next(), if !self.reading.is_empty() => {
                    if let Some(request) = ready.expect("a pending reader").map_err(io::Error::other)? {
                        return Ok(request);
                    }
                }
            }
        }
    }
}

async fn read(mut connection: Connection, job: JobId) -> Option<(Connection, StopRequest)> {
    let parsed = async {
        let bytes = connection
            .recv()
            .await
            .map_err(failure)?
            .ok_or_else(|| Failure::new("the stop request ended before its frame"))?;
        let request: Request = serde_json::from_slice(&bytes).map_err(failure)?;
        if request.job != job {
            return Err(Failure::new("this supervisor owns a different job"));
        }
        validate(&request.stop)?;
        Ok(request.stop)
    }
    .await;
    match parsed {
        Ok(request) => Some((connection, request)),
        Err(error) => {
            eprintln!("refused stop request: {error}");
            None
        }
    }
}

fn validate(request: &StopRequest) -> Result<(), Failure> {
    if request.by.trim().is_empty() || request.reason.trim().is_empty() {
        return Err(Failure::new("a stop must name who requested it and why"));
    }
    Ok(())
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("stopping the job: {error}"))
}
