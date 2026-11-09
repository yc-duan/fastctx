//! Test-only evidence uses a checksum signature. It is intentionally not an
//! Ed25519 implementation or a usable credential; adapter cryptography is separate.
use super::*;
use fastctx_sync::proof::{CommittedFragment, HubKeyRotation, ProofVerifier, SignedAcceptanceFragment};
use fastctx_types::sync::{
    conversation_identity::{ItemKey, ItemRole, NativeId, NativeItemKey},
    conversation_timeline::{BodyAuthority, BodySnapshot, ItemIdentity, ProjectedBody},
    metadata::MetadataCodec,
    proof::{Bytes, ProofPurpose, Signature, SignedStatement, SigningKey},
};
use serde::Serialize;

pub(super) struct Verifier;
fn signature(purpose: ProofPurpose, bytes: &[u8]) -> Signature {
    let mut input = purpose.domain().as_bytes().to_vec();
    input.extend_from_slice(bytes);
    let digest = Digest::of(&input).bytes();
    let mut sig = [0; 64];
    sig[..32].copy_from_slice(&digest);
    sig[32..].copy_from_slice(&digest);
    Signature(sig)
}
fn sign(value: &impl Serialize, purpose: ProofPurpose) -> Result<SignedStatement> {
    let data = serde_json::to_vec(value)?;
    Ok(SignedStatement {
        purpose,
        signer: SigningKey([3; 32]),
        signature: signature(purpose, &data),
        data: Bytes(data),
    })
}
fn verify<T: serde::de::DeserializeOwned>(
    proof: &SignedStatement,
    purpose: ProofPurpose,
) -> std::result::Result<T, Error> {
    if proof.purpose != purpose
        || proof.signature != signature(purpose, &proof.data.0)
        || proof.signer != SigningKey([3; 32])
    {
        return Err(Error::Denied("invalid test evidence".into()));
    }
    serde_json::from_slice(&proof.data.0).map_err(|e| Error::Invalid(e.to_string()))
}
impl ProofVerifier for Verifier {
    fn source(&self, s: &SignedSourceBatch, _: &EpochDescriptor) -> std::result::Result<SourceBatch, Error> {
        verify(&s.proof, ProofPurpose::SourceBatchV1)
    }
    fn epoch(&self, s: &ConfirmedEpoch, i: &HubSigningIdentity) -> std::result::Result<EpochDescriptor, Error> {
        if i.key != s.proof.signer {
            return Err(Error::Denied("test hub key mismatch".into()));
        }
        verify(&s.proof, ProofPurpose::EpochHandoverV1)
    }
    fn accepted(&self, s: &SignedAcceptanceFragment, _: &Authority) -> std::result::Result<CommittedFragment, Error> {
        verify(&s.proof, ProofPurpose::AcceptedOperationV1)
    }
    fn rotation(&self, _: &SignedStatement, _: &HubSigningIdentity) -> std::result::Result<HubKeyRotation, Error> {
        Err(Error::Unavailable("unused in replica lifecycle".into()))
    }
}
pub(super) struct Fixture {
    pub hub: HubSigningIdentity,
    pub context: ReplicaContext,
    pub object: ObjectKey,
    pub begin: ScopeCursor,
    pub view: ViewDefinition,
    pub changes: ChangesPage,
    pub epoch: ConfirmedEpoch,
    pub stream: StreamPage,
    pub items: Vec<TimelineItem>,
    pub blocks: BTreeMap<Digest, Vec<u8>>,
    pub pages: BTreeMap<Digest, ManifestReply>,
}
#[derive(Default)]
struct Depot {
    blocks: BTreeMap<Digest, Vec<u8>>,
    pages: BTreeMap<Digest, ManifestReply>,
}
impl Depot {
    fn block(&mut self, bytes: Vec<u8>) -> ContentBlock {
        let block = ContentBlock {
            id: BlobId { domain: scope(), algorithm: DigestAlgorithm::Sha256, digest: Digest::of(&bytes) },
            decoded_bytes: bytes.len() as u32,
        };
        self.blocks.insert(block.id.digest.clone(), bytes);
        block
    }
    fn metadata(
        &mut self,
        codec: MetadataCodec,
        value: &impl Serialize,
        references: Vec<ContentRef>,
    ) -> Result<ContentRef> {
        let payload = self.block(serde_json::to_vec(value)?);
        let reply = source_ingest::metadata(codec, payload, references)?;
        self.pages.insert(reply.root.0.digest.clone(), reply.clone());
        Ok(ContentRef::Manifest { root: reply.root })
    }
    fn item(&mut self, conversation: &ObjectId, n: u8, revision: u64) -> Result<TimelineItem> {
        let id = ItemIdentity::Native(ItemKey {
            conversation: conversation.clone(),
            role: ItemRole::User,
            native: NativeItemKey::ClaudeMessage { uuid: NativeId::new(format!("message-{n}"))? },
        });
        let descriptor = fastctx_types::sync::conversation_timeline::ItemDisplayV1 {
            version: 1,
            kind: fastctx_types::sync::conversation_timeline::DisplayKind::User,
            tool: None,
            status: None,
            append: None,
            parts: Vec::new(),
            more: None,
        };
        let display = self.metadata(MetadataCodec::DisplayV1, &descriptor, Vec::new())?;
        let original = ContentRef::from(self.block(format!("message {n} revision {revision}").into_bytes()));
        let body = ProjectedBody {
            identity: id.clone(),
            primary: BodySnapshot { original, display, complete: true, text_part: None, text_bytes: ByteCount(0) },
            authority: BodyAuthority::HostFinal,
            world: None,
            host: None,
            run: None,
        };
        let references = body.owned_content().into_iter().cloned().collect();
        let root = self.metadata(MetadataCodec::BodyV1, &body, references)?;
        let object = id.id();
        Ok(TimelineItem {
            id: object.clone(),
            revision: Revision(revision),
            turn: ObjectId::from_bytes([91; 32]),
            parent: None,
            position: TimelinePosition {
                turn: Sequence(1),
                display_ancestry: vec![Sequence(n as u64)],
                accepted: Sequence(n as u64),
                item: object,
            },
            body: root,
            display: Box::new(descriptor),
        })
    }
}
impl Fixture {
    pub fn new() -> Result<Self> {
        let object = custody(92, "conversation.timeline").object;
        let stream = StreamId::of(&object, "conversation.timeline");
        let producer_epoch = ProducerEpoch::from_bytes([93; 32]);
        let descriptor = EpochDescriptor {
            scope: scope(),
            stream: stream.clone(),
            kind: "conversation.timeline".into(),
            schema: 1,
            epoch: producer_epoch.clone(),
            producer: Producer::Device { device: DeviceId::from_bytes([3; 32]) },
            predecessor: None,
            closed_through: None,
            operation: operation(94),
            control_sequence: Sequence(1),
            basis: fastctx_sync::proof::HandoverBasis::Initial { registration: operation(95) },
        };
        let epoch = ConfirmedEpoch { proof: sign(&descriptor, ProofPurpose::EpochHandoverV1)? };
        let hub = HubSigningIdentity {
            hub: scope().hub,
            key: SigningKey([3; 32]),
            revision: Revision(1),
            rotations: Vec::new(),
        };
        let mut depot = Depot::default();
        let first = depot.item(&object.object, 1, 1)?;
        let second = depot.item(&object.object, 2, 1)?;
        let revised = depot.item(&object.object, 1, 2)?;
        let mut records = Vec::new();
        for (i, item) in [first, second.clone(), revised.clone()].into_iter().enumerate() {
            let sequence = Sequence(i as u64 + 1);
            let operation = operation(100 + i as u8);
            let event = TimelineEvent {
                evidence: Evidence {
                    record: RecordRef { stream: stream.clone(), epoch: producer_epoch.clone(), sequence },
                    operation: operation.clone(),
                    accepted: sequence,
                },
                fact: TimelineFact::ItemRevision { change: TimelineChange::Put { item: item.clone() } },
            };
            let payload = depot.block(serde_json::to_vec(&event)?);
            records.push(SourceRecord {
                sequence,
                operation,
                kind: "conversation.timeline".into(),
                schema: 1,
                payload,
                roots: vec![item.body],
                causes: Vec::new(),
            });
        }
        let batch = SourceBatch {
            format: 1,
            scope: scope(),
            stream: stream.clone(),
            producer: descriptor.producer.clone(),
            epoch: producer_epoch.clone(),
            operation: operation(104),
            kind: descriptor.kind.clone(),
            schema: 1,
            after: Sequence(0),
            through: Sequence(3),
            records: records.clone(),
        };
        let signed = SignedSourceBatch { proof: sign(&batch, ProofPurpose::SourceBatchV1)? };
        let mut prefix = feed::stream_genesis(&epoch.proof.id());
        for record in &records {
            prefix = feed::stream_advance(&prefix, record)?.1;
        }
        let fence = StreamProgress::Confirmed {
            cursor: StreamCursor { scope: scope(), stream, epoch: producer_epoch.clone(), committed: Sequence(3) },
            epoch_proof: epoch.proof.id(),
            prefix_digest: prefix.clone(),
        };
        let page = StreamPage {
            fence,
            direction: Direction::Forward,
            records: records
                .into_iter()
                .map(|record| ProvenRecord { record, batch_proof: signed.proof.id() })
                .collect(),
            proofs: vec![signed],
            next: None,
        };
        let view = ViewDefinition {
            scope: scope(),
            acl_revision: Revision(1),
            projection_schema: 1,
            kinds: vec![KindVersion { kind: "conversation.timeline".into(), schema: 1 }],
        };
        let feed_epoch = FeedEpoch::from_bytes([96; 32]);
        let begin = ScopeCursor {
            scope: scope(),
            epoch: feed_epoch.clone(),
            acl_revision: Revision(1),
            view: Some(view.id()?),
            through: Sequence(0),
            prefix_digest: Some(feed::genesis(&scope(), &feed_epoch)),
        };
        let version = VersionRef { authority_epoch: AuthorityEpoch::from_bytes([97; 16]), revision: Revision(1) };
        let head = ObjectHead {
            key: object.clone(),
            kind: "conversation.timeline".into(),
            schema: 1,
            acl_revision: Revision(1),
            authority: Authority::Device { device: DeviceId::from_bytes([3; 32]), epoch: producer_epoch },
            evidence: HeadEvidence::Stream { epoch: epoch.proof.id(), through: Sequence(3), prefix_digest: prefix },
            revision: version.clone(),
            current_version: version,
            content: None,
            history: None,
            lifecycle: Lifecycle::Live,
            completeness: Completeness::MetadataOnly,
        };
        let change = CatalogChange { sequence: Sequence(1), operation: operation(98), head };
        let digest = feed::advance(begin.prefix_digest.as_ref().unwrap(), &change)?.1;
        let through = ScopeCursor { through: Sequence(1), prefix_digest: Some(digest.clone()), ..begin.clone() };
        let changes = ChangesPage {
            after: begin.clone(),
            through,
            entries: vec![FeedEntry::Changed { change: Box::new(change), prefix_digest: digest }],
            proofs: Vec::new(),
            more: false,
        };
        let context = ReplicaContext {
            source: Producer::Hub,
            holding: custody(92, "conversation.timeline").holding,
            source_uses: BTreeMap::new(),
        };
        Ok(Self {
            hub,
            context,
            object,
            begin,
            view,
            changes,
            epoch,
            stream: page,
            items: vec![revised, second],
            blocks: depot.blocks,
            pages: depot.pages,
        })
    }
}
