//! Resolve only World-owned native sessions through the node's foundation. The
//! canonical stream and its persisted epoch are owned by the shared source engine.
use crate::Node;
use fastctx_sync::{
    identity::{HostGeneration, ObjectId, Producer, ProducerEpoch, StreamId},
    store::{Error, access::HoldingUse, ingest::SourceTarget, sources::StreamOpening},
};
use fastctx_types::{
    ResidentId,
    sync::{
        conversation_identity::{NativeId, canonical_timeline_source, claude_transcript_source},
        conversation_timeline::KIND,
        identity::Digest,
    },
};

#[derive(Clone)]
pub struct Claim {
    pub conversation: ObjectId,
    pub session: NativeId,
    pub primary: SourceTarget,
    pub canonical: SourceTarget,
}
impl Claim {
    pub(crate) async fn open(node: &Node, resident: &ResidentId, session: &str) -> Result<Option<Self>, Error> {
        let Some((about, _)) = node.residents.about(resident) else { return Ok(None) };
        if !about.world_managed()
            || about.host.agent.as_str() != "claude"
            || node.residents.deleted_session(session).is_some()
            || !crate::logins::workbench::reads_host(node, &about)
        {
            return Ok(None);
        }
        let Some(foundation) = node.foundation.as_ref() else { return Ok(None) };
        let Some(hosted) = foundation.hosted(node, resident) else { return Ok(None) };
        let session = NativeId::new(session).map_err(|error| Error::Invalid(error.to_string()))?;
        foundation.open_scope(&hosted).await?;
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|error| Error::Unavailable(format!("choosing a source epoch: {error}")))?;
        let candidate = ProducerEpoch::from_bytes(bytes);
        let custody = foundation.source_custody(&hosted, candidate.clone())?;
        let HoldingUse::RuntimeSource { host } = &custody.purpose else {
            return Err(Error::Invalid("hosted source has no runtime provenance".into()));
        };
        let device = host.host.clone();
        let producer = Producer::Device { device: device.clone() };
        let epoch = foundation
            .sources()
            .open_stream(StreamOpening {
                object: hosted.timeline.clone(),
                kind: custody.kind,
                producer: producer.clone(),
                candidate,
            })
            .await?;
        let custody = foundation.source_custody(&hosted, epoch.clone())?;
        let canonical = SourceTarget {
            source: canonical_timeline_source(&hosted.conversation),
            stream: StreamId::of(&hosted.timeline, KIND),
            custody,
            epoch,
            producer,
        };
        let primary = SourceTarget {
            source: claude_transcript_source(&hosted.conversation, &device, &session),
            ..canonical.clone()
        };
        Ok(Some(Self { conversation: hosted.conversation, session, primary, canonical }))
    }
    /// A local first generation is stable even before the first file exists.
    /// Rewrites get a separate persisted generation through reconciliation.
    pub fn initial_generation(&self) -> HostGeneration {
        let mut bytes = b"fastctx.claude.initial-generation.v1\0".to_vec();
        bytes.extend_from_slice(self.primary.source.as_str().as_bytes());
        HostGeneration::from_bytes(Digest::of(&bytes).bytes())
    }
}
