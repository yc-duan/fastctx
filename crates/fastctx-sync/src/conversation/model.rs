use fastctx_types::sync::{
    conversation_timeline::*,
    cursor::{Revision, Sequence},
    identity::{HostGeneration, ObjectId, OperationId, SourceId},
    manifest::ContentRef,
    timeline::{TimelineChange, TimelineItem, TimelinePosition},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Versioned<T> {
    pub evidence: Evidence,
    pub value: T,
}
impl<T: PartialEq> Versioned<T> {
    pub(super) fn accepts(&self, new: &Self) -> Result<bool, Error> {
        if self.evidence.operation == new.evidence.operation && self != new {
            return Err(Error::ConflictingReplay);
        }
        if self.evidence.accepted == new.evidence.accepted && self != new {
            return Err(Error::ConflictingOrder);
        }
        Ok(new.evidence.accepted > self.evidence.accepted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemState {
    pub identity: ItemIdentity,
    pub created: Sequence,
    pub display_created: Sequence,
    pub last_append: Option<Versioned<TextAppend>>,
    pub bodies: BTreeMap<BodyAuthority, Versioned<BodySnapshot>>,
    pub placement: Option<Versioned<Placement>>,
    pub position: Option<TimelinePosition>,
    pub positioned: Option<Placement>,
    /// Canonical display parent; placement retains the original source evidence.
    pub display_parent: Option<ObjectId>,
    pub run: Option<Versioned<RunLink>>,
    pub revision: Revision,
    /// Last emitted descriptor, distinct from newer state still awaiting dependencies.
    pub published: Option<fastctx_types::sync::identity::Digest>,
    pub visible: bool,
    pub alias: Option<ObjectId>,
}
impl ItemState {
    pub fn new(identity: ItemIdentity, created: Sequence) -> Self {
        Self {
            identity,
            created,
            display_created: created,
            last_append: None,
            bodies: BTreeMap::new(),
            placement: None,
            position: None,
            positioned: None,
            display_parent: None,
            run: None,
            revision: Revision(0),
            published: None,
            visible: true,
            alias: None,
        }
    }
    pub fn id(&self) -> ObjectId {
        self.identity.id()
    }
    /// Bounded causes for the selected state, in addition to the triggering
    /// input record/alias receipt. These are logical proof links, not content
    /// dependencies or permission to install another object's bytes.
    pub fn causes(&self) -> Vec<fastctx_types::sync::catalog::RecordRef> {
        let mut causes = Vec::new();
        for evidence in self
            .bodies
            .values()
            .map(|v| &v.evidence)
            .chain(self.placement.iter().map(|v| &v.evidence))
            .chain(self.run.iter().map(|v| &v.evidence))
        {
            if !causes.contains(&evidence.record) {
                causes.push(evidence.record.clone());
            }
        }
        causes
    }

    pub fn body(&self) -> Option<ProjectedBody> {
        use fastctx_types::sync::conversation_identity::ItemRole;
        let world_first = matches!(
            self.identity.role(),
            ItemRole::User | ItemRole::TurnStarted | ItemRole::TurnEnded | ItemRole::System
        );
        let lanes = if world_first {
            [BodyAuthority::World, BodyAuthority::HostFinal, BodyAuthority::HostDraft, BodyAuthority::Legacy]
        } else {
            [BodyAuthority::HostFinal, BodyAuthority::World, BodyAuthority::HostDraft, BodyAuthority::Legacy]
        };
        let (authority, body) = lanes.into_iter().find_map(|lane| self.bodies.get(&lane).map(|body| (lane, body)))?;
        Some(ProjectedBody {
            identity: self.identity.clone(),
            primary: body.value.clone(),
            authority,
            world: self.bodies.get(&BodyAuthority::World).map(|v| v.value.display.clone()),
            host: self
                .bodies
                .get(&BodyAuthority::HostFinal)
                .or_else(|| self.bodies.get(&BodyAuthority::HostDraft))
                .map(|v| v.value.display.clone()),
            run: self.run.as_ref().map(|v| v.value.clone()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnState {
    pub declaration: TurnDeclaration,
    pub evidence: Evidence,
}

/// Only the matching turn and genuine display parent are needed to position an item.
#[derive(Default)]
pub struct Neighbors<'a> {
    pub turn: Option<&'a TurnState>,
    pub parent: Option<&'a ItemState>,
    /// Destination row when the requested parent has a retained alias.
    pub resolved_parent: Option<&'a ItemState>,
    /// The original gap anchor and its canonical destination after an alias.
    pub anchor: Option<&'a ItemState>,
    pub resolved_anchor: Option<&'a ItemState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedPut {
    pub id: ObjectId,
    pub revision: Revision,
    pub turn: Option<ObjectId>,
    pub parent: Option<ObjectId>,
    pub position: TimelinePosition,
    pub body: ProjectedBody,
}
impl PreparedPut {
    /// `body` must be the writer's stored representation of this descriptor,
    /// including its declared owned content closure, not a different payload.
    /// The writer verifies `display` against primary.display before this call.
    pub fn finish(self, body: ContentRef, display: ItemDisplayV1) -> Result<TimelineChange, Error> {
        body.validate().map_err(Error::InvalidContent)?;
        display.validate().map_err(Error::InvalidContent)?;
        display.validate_scopes(&body.id().domain).map_err(Error::InvalidContent)?;
        if body.id().domain != self.body.primary.display.id().domain {
            return Err(Error::InvalidContent("projected display crosses its body scope".into()));
        }
        Ok(TimelineChange::Put {
            item: TimelineItem {
                id: self.id,
                revision: self.revision,
                turn: self.turn,
                parent: self.parent,
                position: self.position,
                body,
                display: Box::new(display),
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dependency {
    Item(ObjectId),
    Placement(ObjectId),
    Turn(ObjectId),
    Parent(ObjectId),
    Anchor(ObjectId),
    Append { item: ObjectId, previous: OperationId },
    Generation { source: SourceId, previous: HostGeneration },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexEffect {
    /// Resolve waiting items by this turn; no transcript scan is necessary.
    TurnReady(ObjectId),
    /// Existing children are revisited by a paged parent index after relocation.
    RepositionChildren(ObjectId),
    Call {
        call: ObjectId,
        item: ObjectId,
    },
    Alias {
        from: ObjectId,
        to: ObjectId,
    },
    /// Persist with Alias. Page the source item's reverse relationship indexes,
    /// resolve effective destinations, and dirty/wake every affected item and
    /// pending dependency. This includes display children, call, picture and
    /// branch membership links. Keep original evidence; new index writes must
    /// resolve aliases too, so arrivals during the paged repair cannot escape it.
    RebindRelations {
        from: ObjectId,
        to: ObjectId,
    },
    Picture(Box<Versioned<PictureLink>>),
    Agent(Box<super::control::AgentState>),
}
#[derive(Clone, Debug, Default)]
pub struct Effects {
    /// Resolve this retained alias and re-key only the derived reducer input.
    /// The original source fact remains unchanged; a present target is not pending.
    pub redirect: Option<ObjectId>,
    pub puts: Vec<PreparedPut>,
    pub changes: Vec<TimelineChange>,
    pub indexes: Vec<IndexEffect>,
    pub pending: Vec<Dependency>,
}
#[derive(Clone, Debug)]
pub struct ItemReduction {
    pub state: ItemState,
    pub effects: Effects,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    WrongKey,
    WrongConversation,
    ConflictingReplay,
    ConflictingOrder,
    CreationChanged,
    CounterExhausted,
    InvalidContent(String),
    InvalidPlacement,
    Cycle,
    InvalidAlias,
    MissingAliasSource,
    FinalText,
    AppendLength,
    AppendPart,
    AppendBase,
    MissingRollbackReceipt,
    InvalidGeneration,
}
impl std::fmt::Display for Error {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "conversation projection: {self:?}")
    }
}
impl std::error::Error for Error {}

pub(super) fn next(revision: Revision) -> Result<Revision, Error> {
    revision.0.checked_add(1).map(Revision).ok_or(Error::CounterExhausted)
}
