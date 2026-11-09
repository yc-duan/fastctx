use super::super::{
    conversation_identity::{ImageSelector, ItemKey, ItemRole, NativeId, ProvisionalKey},
    cursor::ByteCount,
    identity::{ObjectId, ObjectKey},
    manifest::ContentRef,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "identity", content = "key", rename_all = "snake_case", deny_unknown_fields)]
pub enum ItemIdentity {
    Native(ItemKey),
    Provisional(ProvisionalKey),
}
impl ItemIdentity {
    pub fn id(&self) -> ObjectId {
        match self {
            Self::Native(key) => key.id(),
            Self::Provisional(key) => key.id(),
        }
    }
    pub fn conversation(&self) -> &ObjectId {
        match self {
            Self::Native(key) => &key.conversation,
            Self::Provisional(key) => &key.conversation,
        }
    }
    pub fn role(&self) -> ItemRole {
        match self {
            Self::Native(key) => key.role,
            Self::Provisional(key) => key.role,
        }
    }
}

/// Separate lanes keep authoritative World identity and final host text. A late
/// draft cannot replace final text. Within one lane, canonical accepted order wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyAuthority {
    Legacy,
    HostDraft,
    World,
    HostFinal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodySnapshot {
    /// Original host evidence. Public readers never parse this representation.
    pub original: ContentRef,
    /// The DisplayV1 page whose payload is the encoded ItemDisplayV1, at most
    /// 16 KiB. Timeline pages inline the decoded descriptor; part bodies and
    /// original evidence are fetched only on demand.
    pub display: ContentRef,
    pub complete: bool,
    /// The appendable channel inside the projected body, if it has one.
    pub text_part: Option<crate::session::Part>,
    /// Length of the appendable display text, not the source JSON or token count.
    pub text_bytes: ByteCount,
}

/// A small, authorized summary copied into this conversation may be owned here;
/// the job's output is always reached through the logical job object and its ACL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunLink {
    pub call: ObjectId,
    pub job: ObjectKey,
    pub summary: Option<ContentRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum PictureTarget {
    Owned { content: ContentRef },
    Object { object: ObjectKey },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PictureLink {
    pub item: ObjectId,
    pub selector: ImageSelector,
    pub target: PictureTarget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLink {
    pub parent: ObjectKey,
    pub child: ObjectKey,
    pub call: Option<ObjectId>,
    /// Missing fields add no evidence and never erase an existing value.
    pub name: Option<NativeId>,
    pub status: Option<NativeId>,
}

/// Bounded item descriptor. The writer stores this descriptor and its owned
/// content closure before constructing TimelineItem.body. No item contains a
/// full conversation, growing text vector, child transcript or job output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedBody {
    pub identity: ItemIdentity,
    pub primary: BodySnapshot,
    pub authority: BodyAuthority,
    pub world: Option<ContentRef>,
    pub host: Option<ContentRef>,
    pub run: Option<RunLink>,
}
impl ProjectedBody {
    /// Direct roots only. Parts are owned through the display page's closure,
    /// whose DisplayV1 and DisplayPartsV1 pages reference them exactly; the
    /// writer still verifies each Manifest part's total decoded length. Full
    /// replicas follow the whole closure. Lightweight replicas keep descriptors
    /// and overflow description pages without part bodies or original evidence.
    pub fn owned_content(&self) -> Vec<&ContentRef> {
        let mut refs = vec![&self.primary.original, &self.primary.display];
        refs.extend(self.world.iter());
        refs.extend(self.host.iter());
        refs.extend(self.run.iter().filter_map(|run| run.summary.as_ref()));
        refs
    }
}
