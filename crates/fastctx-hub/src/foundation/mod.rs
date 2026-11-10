//! The data foundation inside the hub: its block store over hub.sqlite3, the
//! account directories it signs scope descriptions into, the service devices
//! sync with, and the worker that runs their durable tasks. The hub is the
//! authority here, so its own service keeps everything it describes.

mod conversations;
mod identity;
mod policy;
mod sources;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use fastctx_storage::Database;
use fastctx_sync::{
    authority::Service,
    blocks::SqliteBlocks,
    cursor::Holder,
    identity::{Digest, HubId, OperationId},
    proof::HubSigningIdentity,
    scheduler::{SqliteScheduler, Wake, Worker},
    store::{Error, authority::SqliteAuthority, control::AuthorityRun, directory::SqliteDirectory},
};
use tokio::sync::OnceCell;

/// Codec and read buffers the content engine may hold at once.
const CONTENT_BUFFERS: usize = 32 * 1024 * 1024;
/// How long a claimed task stays this worker's before another may take it.
const LEASE: Duration = Duration::from_secs(60);

/// What exists only once the hub has begun its authority run.
struct Authority {
    directory: Arc<SqliteDirectory>,
    service: Service,
    identity: HubSigningIdentity,
    worker: Mutex<Option<Worker>>,
    wake: Wake,
}

pub(crate) struct Foundation {
    dir: PathBuf,
    hub: HubId,
    blocks: SqliteBlocks,
    /// Set by `begin`, before the hub accepts any write.
    authority: OnceCell<Authority>,
    scheduler: SqliteScheduler,
    report: Arc<dyn Fn(String) + Send + Sync>,
    sessions: fastctx_net::asset_transport::sessions::Sessions,
}

impl Foundation {
    pub(crate) fn asset_sessions(&self) -> &fastctx_net::asset_transport::sessions::Sessions {
        &self.sessions
    }
    pub(crate) fn advertisement(&self) -> Option<fastctx_net::sync_transport::Advertisement> {
        self.authority().ok().map(|a| fastctx_net::sync_transport::Advertisement::local(Some(a.identity.clone())))
    }
    pub(crate) async fn recheck_message(
        &self,
        peer: &fastctx_sync::authority::Peer,
        request: &fastctx_sync::transfer::Message,
    ) -> Result<(), Error> {
        self.authority()?.service.recheck_message(peer, request).await
    }
    pub(crate) async fn recheck(
        &self,
        peer: &fastctx_sync::authority::Peer,
        holding: &fastctx_sync::cursor::HoldingPeriod,
    ) -> Result<(), Error> {
        self.authority()?.service.recheck(peer, holding).await
    }
    pub(crate) async fn check_supply(
        &self,
        peer: &fastctx_sync::authority::Peer,
        request: &fastctx_sync::replicate::BlockRequest,
    ) -> Result<(), Error> {
        self.authority()?.service.check_supply(peer, request).await
    }
    pub(crate) async fn supply(
        &self,
        peer: &fastctx_sync::authority::Peer,
        request: fastctx_sync::replicate::BlockRequest,
    ) -> Result<fastctx_sync::replicate::Received<Vec<u8>>, Error> {
        self.authority()?.service.supply(peer, request).await
    }
    pub(crate) async fn check_source_block(
        &self,
        peer: &fastctx_sync::authority::Peer,
        object: &fastctx_sync::identity::ObjectKey,
        upload: &fastctx_sync::authority::PeerUpload,
    ) -> Result<(), Error> {
        self.authority()?.service.check_source_block(peer, object, upload).await
    }
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
        let sessions = fastctx_net::asset_transport::sessions::Sessions::new(blocks.buffer_budget());
        Ok(Self { dir: dir.to_path_buf(), hub, blocks, authority: OnceCell::new(), scheduler, report, sessions })
    }

    /// Loads or first creates the hub's service key, then begins this run's
    /// authority epoch. The hub signs nothing before this returns.
    pub(crate) async fn begin(&self) -> Result<(), Error> {
        self.authority
            .get_or_try_init(|| async {
                let (signer, identity) = identity::load(&self.dir, &self.hub, &self.blocks).await?;
                let entropy: fastctx_sync::store::control::Entropy = Arc::new(fastctx_net::sign::random::<32>);
                let run = AuthorityRun::begin(&self.blocks, self.hub.clone(), entropy).await?;
                let verifier = Arc::new(fastctx_net::sync_proof::IdentityVerifier::new(
                    self.hub.clone(),
                    [identity.key.clone()],
                    Arc::new(fastctx_sync::metrics::PlatformMetrics),
                ));
                // Composition installs real per-kind source policies with AccessState;
                // an empty set keeps source intake unavailable.
                let sources = Arc::new(SqliteAuthority::new(
                    self.blocks.clone(),
                    run.clone(),
                    signer.clone(),
                    verifier,
                    identity.clone(),
                    Vec::new(),
                    8 * 1024 * 1024,
                )?);
                let directory = Arc::new(SqliteDirectory::new(self.blocks.clone(), run, signer));
                let service = Service::new(directory.clone()).with_sources(sources.clone());
                let mut handlers = self.blocks.task_handlers();
                handlers.push(sources);
                let worker = Worker::new(self.scheduler.clone(), handlers, self.report.clone());
                Ok::<_, Error>(Authority {
                    directory,
                    service,
                    identity,
                    wake: worker.wake(),
                    worker: Mutex::new(Some(worker)),
                })
            })
            .await
            .map(|_| ())
    }

    /// Runs the durable tasks after the authority has begun.
    pub(crate) async fn run(&self) {
        let Ok(authority) = self.authority() else { return };
        let worker = authority.worker.lock().expect("never poisoned").take();
        let Some(worker) = worker else { return };
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
