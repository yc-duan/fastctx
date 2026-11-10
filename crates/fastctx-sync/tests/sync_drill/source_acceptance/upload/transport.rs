//! Real source endpoints behind the same admitted JSON loopback as discovery.
use super::*;
use fastctx_sync::{
    replicate::{BlockOffer, BlockRequest, Received, Upstream},
    store::FutureResult,
};
use std::sync::Mutex;

pub(super) struct UploadLoopback {
    pub inner: Loopback,
    pub calls: AtomicUsize,
    pub offers: Mutex<Vec<OperationId>>,
    pub killed_reply: bool,
    pub latency: Duration,
    pub source_active: AtomicUsize,
    pub source_max: AtomicUsize,
    pub offer_active: AtomicUsize,
    pub offer_max: AtomicUsize,
    pub bytes: AtomicUsize,
    pub waiting_once: AtomicBool,
    pub partial_once: AtomicBool,
    pub audit_staging: AtomicBool,
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
fn active<'a>(count: &'a AtomicUsize, max: &AtomicUsize) -> Active<'a> {
    let n = count.fetch_add(1, Ordering::AcqRel) + 1;
    max.fetch_max(n, Ordering::AcqRel);
    Active(count)
}
impl UploadLoopback {
    pub fn new(service: Arc<Service>, budget: fastctx_sync::admission::ByteBudget, killed_reply: bool) -> Self {
        Self {
            inner: Loopback {
                service,
                budget,
                peer: Peer { device: DeviceId::from_bytes([3; 32]), accounts: vec![account().id] },
            },
            calls: AtomicUsize::new(0),
            offers: Mutex::new(Vec::new()),
            killed_reply,
            latency: Duration::ZERO,
            source_active: AtomicUsize::new(0),
            source_max: AtomicUsize::new(0),
            offer_active: AtomicUsize::new(0),
            offer_max: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            waiting_once: AtomicBool::new(false),
            partial_once: AtomicBool::new(false),
            audit_staging: AtomicBool::new(false),
        }
    }
    fn received(&self, message: Message) -> std::result::Result<Received<Message>, Error> {
        let bytes = serde_json::to_vec(&message).map_err(|e| Error::Invalid(e.to_string()))?;
        fastctx_sync::replicate::decode_message(&bytes, &self.inner.budget)
    }
}
impl Upstream for UploadLoopback {
    fn exchange(&self, request: Message) -> FutureResult<'_, Received<Message>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let source = matches!(request, Message::Source { .. });
            let _active = source.then(|| active(&self.source_active, &self.source_max));
            if let Message::Source { batch, .. } = &request
                && self.waiting_once.swap(false, Ordering::AcqRel)
            {
                return self.received(Message::SourceResult {
                    result: SourceAccepted::AwaitingEpoch { proof: batch.proof.id() },
                });
            }
            let answer = self.inner.exchange(request).await?;
            if self.killed_reply
                && source
                && matches!(answer.value(), Message::SourceResult { result: SourceAccepted::Accepted { .. } })
            {
                println!("source-upload-hub-accepted");
                std::io::stdout().flush().map_err(|e| Error::Unavailable(e.to_string()))?;
                loop {
                    std::thread::park();
                }
            }
            if source && !self.latency.is_zero() {
                tokio::time::sleep(self.latency).await;
            }
            if source
                && matches!(answer.value(), Message::SourceResult { result: SourceAccepted::Accepted { .. } })
                && self.partial_once.swap(false, Ordering::AcqRel)
            {
                let mut message = answer.value().clone();
                if let Message::SourceResult { result: SourceAccepted::Accepted { content_complete, .. } } =
                    &mut message
                {
                    *content_complete = false;
                }
                drop(answer);
                return self.received(message);
            }
            Ok(answer)
        })
    }
    fn fetch(&self, _: BlockRequest) -> FutureResult<'_, Received<Vec<u8>>> {
        Box::pin(async { Err(Error::Unavailable("upload-only loopback".into())) })
    }
    fn offer(&self, offer: BlockOffer) -> FutureResult<'_, fastctx_sync::cursor::DurableAck> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let _active = active(&self.offer_active, &self.offer_max);
            let fastctx_types::sync::session::UploadCause::Source { epoch } = offer.cause else {
                return Err(Error::Invalid("expected source upload cause".into()));
            };
            self.offers.lock().unwrap().push(offer.operation.clone());
            let upload = fastctx_types::sync::source_transfer::SourceTransfer {
                holding: offer.holding.clone(),
                object: offer.object.clone(),
                kind: offer.kind.clone(),
                epoch: epoch.clone(),
                operation: offer.operation.clone(),
            };
            let representation = serde_json::from_slice(
                &serde_json::to_vec(&offer.representation).map_err(|e| Error::Invalid(e.to_string()))?,
            )
            .map_err(|e| Error::Invalid(e.to_string()))?;
            let length = offer.encoded.len();
            let published = self
                .inner
                .service
                .put_source_block(
                    &self.inner.peer,
                    fastctx_sync::authority::PeerUpload {
                        holding: offer.holding,
                        kind: offer.kind.kind.clone(),
                        epoch,
                        operation: offer.operation,
                    },
                    representation,
                    offer.encoded,
                )
                .await?;
            self.bytes.fetch_add(length, Ordering::AcqRel);
            if self.audit_staging.swap(false, Ordering::AcqRel) {
                let content = ContentRef::from(published.content.clone());
                let reply = self
                    .inner
                    .exchange(Message::ProbeSource { upload: upload.clone(), content: content.clone() })
                    .await?;
                assert!(matches!(reply.value(), Message::SourcePresence { present: true, .. }));
                drop(reply);
                let mut other = upload.clone();
                other.operation = operation(250);
                let reply =
                    self.inner.exchange(Message::ProbeSource { upload: other, content: content.clone() }).await?;
                assert!(matches!(reply.value(), Message::SourcePresence { present: false, .. }));
                drop(reply);
                let mut other = upload;
                other.object.object = ObjectId::from_bytes([251; 32]);
                assert!(matches!(
                    self.inner.exchange(Message::ProbeSource { upload: other, content }).await,
                    Err(Error::Denied(_))
                ));
            }
            if !self.latency.is_zero() {
                tokio::time::sleep(self.latency).await;
            }
            Ok(fastctx_sync::cursor::DurableAck::Block {
                holding: published.pin.holding,
                object: offer.object,
                kind: offer.kind.kind,
                schema: offer.kind.schema,
                content: published.content,
            })
        })
    }
}
