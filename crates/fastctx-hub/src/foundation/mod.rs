//! The data foundation inside the hub: its block store over hub.sqlite3, the
//! account directories it signs scope descriptions into, the service devices
//! sync with, and the worker that runs their durable tasks. The hub is the
//! authority here, so its own service keeps everything it describes.

mod conversations;
mod identity;
mod policy;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use fastctx_storage::Database;
use fastctx_sync::{
    authority::Service,
    blocks::SqliteBlocks,
    cursor::Holder,
    identity::{Digest, HubId, OperationId},
    proof::HubSigningIdentity,
    scheduler::{SqliteScheduler, Worker},
    store::{Error, control::AuthorityRun, directory::SqliteDirectory},
};

/// Codec and read buffers the content engine may hold at once.
const CONTENT_BUFFERS: usize = 16 * 1024 * 1024;
/// How long a claimed task stays this worker's before another may take it.
const LEASE: Duration = Duration::from_secs(60);

/// What exists only once the hub has begun its authority run.
struct Authority {
    directory: Arc<SqliteDirectory>,
    #[allow(dead_code, reason = "the asset sync transport's entry")]
    service: Service,
    #[allow(dead_code, reason = "the asset sync transport's advertisement")]
    identity: HubSigningIdentity,
}

pub(crate) struct Foundation {
    dir: PathBuf,
    hub: HubId,
    blocks: SqliteBlocks,
    /// Set by `begin`, before the hub accepts any write.
    authority: OnceLock<Authority>,
    /// Taken once by `run`.
    worker: Mutex<Option<Worker>>,
}

impl Foundation {
    /// Builds the engines on the hub's open database; nothing runs until `run`.
    pub(crate) fn open(dir: &Path, database: Database, hub: HubId, log: Arc<crate::log::Log>) -> anyhow::Result<Self> {
        let holder = Holder::Hub { hub: hub.clone() };
        let claims = claims(&hub);
        let blocks = SqliteBlocks::new(
            database.clone(),
            dir.join("sync").join("blocks"),
            holder.clone(),
            Arc::new(policy::HubPolicy),
            claims.clone(),
            CONTENT_BUFFERS,
        )?;
        let scheduler = SqliteScheduler::new(database, claims, holder, LEASE)?;
        let report: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line| log.line(line));
        let worker = Worker::new(scheduler, blocks.task_handlers(), report);
        Ok(Self { dir: dir.to_path_buf(), hub, blocks, authority: OnceLock::new(), worker: Mutex::new(Some(worker)) })
    }

    /// Loads or first creates the hub's service key, then begins this run's
    /// authority epoch. The hub signs nothing before this returns.
    pub(crate) async fn begin(&self) -> Result<(), Error> {
        let (signer, identity) = identity::load(&self.dir, &self.hub, &self.blocks).await?;
        let entropy: fastctx_sync::store::control::Entropy = Arc::new(fastctx_net::sign::random::<32>);
        let run = AuthorityRun::begin(&self.blocks, self.hub.clone(), entropy).await?;
        let directory = Arc::new(SqliteDirectory::new(self.blocks.clone(), run, signer));
        let service = Service::new(directory.clone());
        let _ = self.authority.set(Authority { directory, service, identity });
        Ok(())
    }

    /// Runs the durable tasks for as long as the hub does.
    pub(crate) async fn run(&self) {
        let Some(worker) = self.worker.lock().expect("never poisoned").take() else {
            return;
        };
        worker.run(std::future::pending()).await;
    }

    fn authority(&self) -> Result<&Authority, Error> {
        self.authority.get().ok_or_else(|| Error::Unavailable("the hub's authority run has not begun".into()))
    }
}

/// The owner this process claims tasks and content work as. It differs on
/// every start, so a restarted hub never takes its predecessor's leases.
fn claims(hub: &HubId) -> OperationId {
    let started =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    let seed = format!("fastctx-hub/foundation-claims/v1\0{hub}\0{}\0{started}", std::process::id());
    OperationId::from_bytes(Digest::of(seed.as_bytes()).bytes())
}
