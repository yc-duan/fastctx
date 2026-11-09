//! The unified data foundation on this World's database: the content, source,
//! timeline and scope engines, the worker that runs their durable tasks, and
//! the device identity they sign and hold with. A World on no hub keeps its
//! conversations under its alone hub and opens their scopes itself.

mod conversations;
mod identity;
mod maintenance;
mod policy;
pub(crate) mod reads;
mod tasks;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use fastctx_storage::Database;
use fastctx_sync::{
    blocks::SqliteBlocks,
    cursor::Holder,
    identity::{AccountId, DeviceId, HubId},
    scheduler::{SqliteScheduler, Wake, Worker},
    store::{ingest_memory::IngestBudget, scopes::SqliteScopes, sources::SqliteSources, timelines::SqliteTimelines},
};
use fastctx_types::layout::WorldDir;

/// Codec and read buffers the content engine may hold at once.
const CONTENT_BUFFERS: usize = 16 * 1024 * 1024;
/// How long a claimed task stays this worker's before another may take it.
const LEASE: Duration = Duration::from_secs(60);

pub(crate) struct Foundation {
    blocks: SqliteBlocks,
    sources: SqliteSources,
    timelines: SqliteTimelines,
    scopes: SqliteScopes,
    alone: HubId,
    device: DeviceId,
    owner: AccountId,
    wake: Wake,
    /// Taken once by `run`.
    worker: Mutex<Option<Worker>>,
}

impl Foundation {
    /// Builds the engines on the World's open database; nothing runs until `run`.
    pub(crate) fn open(world: &WorldDir, database: &Database, log: Arc<crate::log::Log>) -> anyhow::Result<Self> {
        let identity = identity::Identity::load(world)?;
        let holder = Holder::Device { device: identity.device.clone() };
        let policy =
            Arc::new(policy::AlonePolicy::new(identity.alone.clone(), identity.device.clone(), identity.owner.clone()));
        let claims = identity::claims(&identity.device);
        let blocks = SqliteBlocks::new(
            database.clone(),
            world.path().join("sync").join("blocks"),
            holder.clone(),
            policy,
            claims.clone(),
            CONTENT_BUFFERS,
        )?;
        let memory = IngestBudget::for_machine(fastctx_platform::memory::physical().ok())?;
        let sources = SqliteSources::new(
            blocks.clone(),
            identity.signer,
            vec![Arc::new(fastctx_sync::conversation::source::TimelineAuthorizer)],
            memory,
        )?;
        let timelines = SqliteTimelines::new(blocks.clone());
        let scopes = SqliteScopes::new(blocks.clone(), identity.alone.clone(), identity.device.clone());
        let scheduler = SqliteScheduler::new(database.clone(), claims, holder, LEASE)?;
        let report: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line| log.line(line));
        let worker = Worker::new(scheduler, tasks::handlers(&blocks, &timelines, &sources), report);
        Ok(Self {
            wake: worker.wake(),
            worker: Mutex::new(Some(worker)),
            blocks,
            sources,
            timelines,
            scopes,
            alone: identity.alone,
            device: identity.device,
            owner: identity.owner,
        })
    }

    /// Runs the durable tasks and the periodic upkeep for as long as the node does.
    pub(crate) async fn run(&self, log: Arc<crate::log::Log>) {
        let Some(worker) = self.worker.lock().expect("never poisoned").take() else {
            return;
        };
        fastctx_platform::task::spawn(maintenance::run(self.blocks.clone(), log));
        worker.run(std::future::pending()).await;
    }

    #[allow(dead_code, reason = "the conversation ingestion adapter's entry")]
    pub(crate) fn sources(&self) -> &SqliteSources {
        &self.sources
    }
    pub(crate) fn timelines(&self) -> &SqliteTimelines {
        &self.timelines
    }
    /// Wakes the task worker after a commit that made work due, such as an
    /// ingested step whose timeline now needs installing.
    #[allow(dead_code, reason = "the conversation ingestion adapter's entry")]
    pub(crate) fn wake(&self) {
        self.wake.notify();
    }
}
