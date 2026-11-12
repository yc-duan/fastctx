//! What a kind of record is (design §11): its number and schema, the stream
//! class it goes in and who holds it, its objects' roles, how it is encoded,
//! how long its frames are kept, how it is deleted, and — the rule that
//! makes it safe to take from anyone — who may change what of it. A kind's
//! records are written only through a `Writer` of that kind, which judges
//! each against the item as it stands here by the same rule the hub judges
//! it by when it applies the frame (`Kinds`, an `Authorizer`).

use super::{Authorizer, Commit, Committed, Entry, Header, KindRecord, Lease, Library, Op, Signer};
use crate::error::Error;
use crate::{ObjectRef, Record, Result, State};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// Who holds a kind's stream class (§2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holding {
    /// Every device whose accounts read the scope.
    Readers,
    /// The devices logged in to an account that reads the scope.
    Logged,
}

/// What deleting a record of the kind leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deletion {
    /// A tombstone, the item gone from reads.
    Tombstone,
    /// Nothing: the whole item goes with its window (§4.6).
    Whole,
}

/// Who acts: the device that orders the frame, in which scope and stream.
#[derive(Clone, Copy, Debug)]
pub struct Actor<'a> {
    pub device: [u8; 32],
    pub scope: &'a str,
    pub stream: &'a str,
}

/// A kind of record. Every item is required: a kind without one does not
/// compile.
pub trait Kind: Send + Sync + 'static {
    const KIND: u16;
    const SCHEMA: u16;
    /// The prefix of the streams its records go in.
    const STREAM: &'static str;
    const HOLDING: Holding;
    /// The roles its objects play (§2.4), in the order a record lists them.
    const ROLES: &'static [&'static str];
    /// How many frames of its streams are kept, if not all (§4.6).
    const RETENTION: Option<u64>;
    const DELETION: Deletion;
    type Value;
    fn encode(value: &Self::Value) -> Vec<u8>;
    fn decode(body: &[u8]) -> std::result::Result<Self::Value, String>;
    /// Whether `actor` may change the item from `before` to `after`: judged
    /// by what actually changes, never by who may see it (§11).
    fn authorize(actor: &Actor<'_>, before: Option<&Entry>, after: &KindRecord) -> std::result::Result<(), String>;
}

type Rule = fn(&Actor<'_>, Option<&Entry>, &KindRecord) -> std::result::Result<(), String>;

/// The kinds a hub takes records of, each judged by its own rule; a record
/// of a kind not here, or of a schema later than the one here, is refused.
#[derive(Default)]
pub struct Kinds {
    rules: HashMap<u16, (u16, Rule)>,
}

impl Kinds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<K: Kind>(mut self) -> Self {
        self.rules.insert(K::KIND, (K::SCHEMA, K::authorize));
        self
    }
}

impl Authorizer for Kinds {
    fn authorize(&self, frame: &Header, record: &KindRecord, before: Option<&Entry>) -> std::result::Result<(), String> {
        let Some((schema, rule)) = self.rules.get(&record.kind) else {
            return Err(format!("records of kind {} are not taken here", record.kind));
        };
        if record.schema > *schema {
            return Err(format!("a record of kind {} at schema {} is newer than this hub knows", record.kind, record.schema));
        }
        rule(&Actor { device: frame.device, scope: &frame.scope, stream: &frame.stream }, before, record)
    }
}

/// The only way a kind's records are written: into a stream of its class,
/// each judged here as the hub will judge it.
pub struct Writer<K: Kind> {
    library: Library,
    scope: String,
    stream: String,
    kind: PhantomData<K>,
}

impl<K: Kind> Writer<K> {
    pub fn new(library: &Library, scope: &str, stream: &str) -> Result<Self> {
        if !stream.starts_with(K::STREAM) {
            return Err(Error::Invalid(format!("records of kind {} go in streams under {}", K::KIND, K::STREAM)));
        }
        Ok(Self { library: library.clone(), scope: scope.to_string(), stream: stream.to_string(), kind: PhantomData })
    }

    /// Writes `value` as the item `item` at `order`, or deletes it when
    /// `value` is none; `objects` in the kind's role order, `leases` on
    /// them let go with the commit, `op` the operation it carries.
    #[allow(clippy::too_many_arguments)]
    pub fn write(
        &self,
        item: Vec<u8>,
        order: Vec<u8>,
        value: Option<&K::Value>,
        objects: Vec<ObjectRef>,
        leases: Vec<Lease>,
        op: Option<(Op, bool)>,
        signer: Arc<dyn Signer>,
    ) -> Result<Committed> {
        let state = match value {
            Some(value) => State::Is { body: K::encode(value), objects },
            None => State::Gone,
        };
        let record = KindRecord { kind: K::KIND, schema: K::SCHEMA, record: Record { item: item.clone(), order, state } };
        let before = self.library.item(&self.scope, &self.stream, &item)?;
        let actor = Actor { device: signer.device(), scope: &self.scope, stream: &self.stream };
        K::authorize(&actor, before.as_ref(), &record).map_err(Error::Invalid)?;
        let holding = self.library.holding(&self.scope)?.ok_or_else(|| Error::Invalid("the scope is not held here".into()))?;
        self.library.commit(
            Commit { scope: self.scope.clone(), stream: self.stream.clone(), records: vec![record], leases, durable: true, op, holding },
            signer,
        )
    }

    /// The item `item` as it stands here, decoded.
    pub fn read(&self, item: &[u8]) -> Result<Option<K::Value>> {
        let Some(entry) = self.library.item(&self.scope, &self.stream, item)? else { return Ok(None) };
        K::decode(&entry.body).map(Some).map_err(Error::Invalid)
    }
}
