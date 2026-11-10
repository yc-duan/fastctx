use super::*;
use fastctx_sync::{
    cursor::DurableAck,
    replicate::*,
    store::{Error, FutureResult},
    transfer::Message,
};
struct Paused {
    real: Arc<dyn Upstream>,
    marker: std::path::PathBuf,
}
impl Upstream for Paused {
    fn exchange(&self, request: Message) -> FutureResult<'_, Received<Message>> {
        Box::pin(async move {
            let source = matches!(request, Message::Source { .. });
            let answer = self.real.exchange(request).await?;
            if source
                && matches!(
                    answer.value(),
                    Message::SourceResult { result: fastctx_sync::store::authority::SourceAccepted::Accepted { .. } }
                )
            {
                std::fs::write(&self.marker, b"accepted response arrived before local receipt commit")
                    .map_err(|e| Error::Unavailable(e.to_string()))?;
                std::future::pending::<()>().await;
            }
            Ok(answer)
        })
    }
    fn fetch(&self, request: BlockRequest) -> FutureResult<'_, Received<Vec<u8>>> {
        self.real.fetch(request)
    }
    fn offer(&self, offer: BlockOffer) -> FutureResult<'_, DurableAck> {
        self.real.offer(offer)
    }
}
pub(super) async fn run(root: &Path, pause: bool) -> anyhow::Result<()> {
    let c = Config::load(root)?;
    let dir = root.join("node-0");
    let (db, blocks) = node::blocks(&dir, &c, 0)?;
    let (link, upstream) = node::connect(&dir, &c, 0, &blocks).await?;
    node::directory(&blocks, &c, 0).discover(upstream.as_ref()).await?;
    let sources = data::sources(&c, &blocks)?;
    let uploader = SourceUploader::new(sources.clone(), c.hub.clone())?;
    let observed = sources.clone();
    let queue = fastctx_sync::scheduler::SqliteScheduler::new(
        db,
        fastctx_sync::identity::OperationId::from_bytes([55; 32]),
        fastctx_sync::cursor::Holder::Device { device: c.device(0) },
        Duration::from_secs(60),
    )?;
    let mut handlers = blocks.task_handlers();
    handlers.push(Arc::new(sources));
    let worker = fastctx_sync::scheduler::Worker::new(queue, handlers, Arc::new(|line| eprintln!("{line}")));
    let worker = tokio::spawn(worker.run(std::future::pending()));
    let upstream: Arc<dyn Upstream> =
        if pause { Arc::new(Paused { real: upstream, marker: root.join("pause-a") }) } else { upstream };
    let began = std::time::Instant::now();
    let mut retries = 0;
    for _ in 0..4096 {
        match uploader.step(upstream.clone(), data::key(&c)).await {
            Ok(SourceUploadProgress::Idle(state)) if state.through.0 == 4 => {
                println!("source upload complete: {:.3}s retries={retries}", began.elapsed().as_secs_f64());
                use fastctx_sync::store::ingest::SourceIngest;
                for _ in 0..600 {
                    if matches!(
                        observed.progress(data::target(&c)).await?,
                        Some(fastctx_sync::store::ingest::StreamProgress::Confirmed { .. })
                    ) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                anyhow::ensure!(
                    matches!(
                        observed.progress(data::target(&c)).await?,
                        Some(fastctx_sync::store::ingest::StreamProgress::Confirmed { .. })
                    ),
                    "source prefix rebuild did not finish"
                );
                drop(upstream);
                worker.abort();
                link.stop();
                return Ok(());
            }
            Ok(SourceUploadProgress::Stopped(state)) => anyhow::bail!("source stopped: {:?}", state.stopped),
            Err(Error::Busy | Error::Unavailable(_) | Error::OutcomeUnknown { .. }) => {
                retries += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            value => {
                value?;
            }
        }
    }
    anyhow::bail!("upload did not reach its durable acknowledgement")
}
