//! Source facts declare their direct content roots. Typed descriptor Metadata
//! pages retain nested parts; logical object and record links never grant custody.
use super::plan::record_operation;
use fastctx_sync::store::{Error, source_step::SourceStepTarget};
use fastctx_types::sync::{
    catalog::{RecordRef, SourceRecord},
    conversation_timeline::{BranchChange, KIND, PictureTarget, SCHEMA, TimelineFact},
    cursor::{Count, Sequence},
    identity::Digest,
    manifest::{ContentBlock, ContentRef},
    timeline::TimelineChange,
    transport_codec::bounded_json,
};

const FACT_BYTES: usize = 64 * 1024;

pub struct PreparedFact {
    payload: Vec<u8>,
    roots: Vec<ContentRef>,
}
impl PreparedFact {
    pub fn new(fact: &TimelineFact) -> Result<Self, Error> {
        let payload =
            bounded_json(fact, FACT_BYTES).map_err(|_| Error::Invalid("timeline fact exceeds 64 KiB".into()))?;
        let mut roots = Vec::with_capacity(2);
        let mut add = |root: &ContentRef| {
            if !roots.contains(root) {
                roots.push(root.clone());
            }
        };
        match fact {
            TimelineFact::Observed { observation } => {
                add(&observation.body.original);
                add(&observation.body.display);
            }
            TimelineFact::ItemRevision { change } => match change {
                TimelineChange::Put { item } => {
                    item.display.validate().map_err(Error::Invalid)?;
                    item.display.validate_scopes(&item.body.id().domain).map_err(Error::Invalid)?;
                    add(&item.body);
                    for root in item.display.owned_content() {
                        add(&root);
                    }
                }
                TimelineChange::SelectBranch { root, .. } => add(&ContentRef::Manifest { root: root.clone() }),
                TimelineChange::Retract { .. } => {}
            },
            TimelineFact::TextAppend { append } => add(&append.chunk),
            TimelineFact::Branch { change: BranchChange::Select { root, .. } } => {
                add(&ContentRef::Manifest { root: root.clone() })
            }
            TimelineFact::Run { run, .. } => {
                if let Some(summary) = &run.summary {
                    add(summary);
                }
            }
            TimelineFact::Image { picture } => {
                if let PictureTarget::Owned { content } = &picture.target {
                    add(content);
                }
            }
            TimelineFact::HostRewritten { .. }
            | TimelineFact::Branch { change: BranchChange::Membership { .. } }
            | TimelineFact::Alias { .. }
            | TimelineFact::Turn { .. }
            | TimelineFact::Placement { .. }
            | TimelineFact::Agent { .. } => {}
        }
        Ok(Self { payload, roots })
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    pub fn roots(&self) -> &[ContentRef] {
        &self.roots
    }
    /// Complete after uploading this payload through the bound source step. The
    /// canonical sequence and position must also be used when building Evidence.
    pub fn record(
        self,
        target: &SourceStepTarget,
        position: Count,
        sequence: Sequence,
        payload: ContentBlock,
        causes: Vec<RecordRef>,
    ) -> Result<SourceRecord, Error> {
        let scope = &target.target().custody.object.scope;
        let kind = target.target().custody.kind.registration();
        if kind.id != KIND
            || kind.schema != SCHEMA
            || &payload.id.domain != scope
            || payload.id.digest != Digest::of(&self.payload)
            || payload.decoded_bytes as usize != self.payload.len()
        {
            return Err(Error::Invalid("timeline fact differs from its source kind or uploaded payload".into()));
        }
        for root in &self.roots {
            root.validate().map_err(Error::Invalid)?;
            if &root.id().domain != scope {
                return Err(Error::Invalid("timeline fact owns content outside its scope".into()));
            }
        }
        Ok(SourceRecord {
            sequence,
            operation: record_operation(target.step(), position),
            kind: KIND.into(),
            schema: SCHEMA,
            payload,
            roots: self.roots,
            causes,
        })
    }
}
