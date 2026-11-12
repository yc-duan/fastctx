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

/// What a kind takes up from an old store as its records come in (§15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Import {
    /// Nothing: the kind kept nothing before.
    Nothing,
    /// Nothing: its records are made again from their sources.
    Remade,
    /// What its importer, by this name, takes up.
    From(&'static str),
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
    /// What it takes up from an old store (§15).
    const IMPORT: Import;
    type Value;
    fn encode(value: &Self::Value) -> Vec<u8>;
    fn decode(body: &[u8]) -> std::result::Result<Self::Value, String>;
    /// Whether `actor` may change the item from `before` to `after`: judged
    /// by what actually changes, never by who may see it (§11).
    fn authorize(actor: &Actor<'_>, before: Option<&Entry>, after: &KindRecord) -> std::result::Result<(), String>;
    /// The local subscription a change to one of its items in `stream` of
    /// `scope` is told under (§6.6).
    fn subscription(scope: &str, stream: &str) -> String;
}

type Judgement = fn(&Actor<'_>, Option<&Entry>, &KindRecord) -> std::result::Result<(), String>;

/// What a hub keeps of a kind to judge its records by.
struct Rule {
    schema: u16,
    stream: &'static str,
    roles: usize,
    judge: Judgement,
}

/// The kinds a hub takes records of, each judged by its own rule after what
/// every kind's records must be (in a stream of its class, its objects in
/// its roles). A record of a kind not here, or of a schema later than the
/// one here, is kept in its frame and judged once it is (`knows`).
#[derive(Default)]
pub struct Kinds {
    rules: HashMap<u16, Rule>,
}

impl Kinds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<K: Kind>(mut self) -> Self {
        self.rules.insert(K::KIND, Rule { schema: K::SCHEMA, stream: K::STREAM, roles: K::ROLES.len(), judge: K::authorize });
        self
    }
}

impl Authorizer for Kinds {
    fn authorize(&self, frame: &Header, record: &KindRecord, before: Option<&Entry>) -> std::result::Result<(), String> {
        let Some(rule) = self.rules.get(&record.kind) else {
            return Err(format!("records of kind {} are not taken here", record.kind));
        };
        if record.schema > rule.schema {
            return Err(format!("a record of kind {} at schema {} is newer than this hub knows", record.kind, record.schema));
        }
        if !frame.stream.starts_with(rule.stream) {
            return Err(format!("records of kind {} go in streams under {}", record.kind, rule.stream));
        }
        if let State::Is { objects, .. } = &record.record.state
            && objects.len() > rule.roles
        {
            return Err(format!("a record of kind {} refers to more objects than its kind has roles", record.kind));
        }
        (rule.judge)(&Actor { device: frame.device, scope: &frame.scope, stream: &frame.stream }, before, record)
    }

    fn knows(&self, kind: u16, schema: u16) -> bool {
        self.rules.get(&kind).is_some_and(|rule| schema <= rule.schema)
    }
}

/// One item a `Writer` writes among several: `value` none deletes it.
pub struct Write<K: Kind> {
    pub item: Vec<u8>,
    pub order: Vec<u8>,
    pub value: Option<K::Value>,
    /// In the kind's role order.
    pub objects: Vec<ObjectRef>,
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
        // The period first: what is judged is judged in it.
        let holding = self.library.holding(&self.scope)?.ok_or_else(|| Error::Invalid("the scope is not held here".into()))?;
        let state = match value {
            Some(value) => State::Is { body: K::encode(value), objects },
            None => State::Gone,
        };
        let record = KindRecord { kind: K::KIND, schema: K::SCHEMA, record: Record { item, order, state } };
        let judge = self.judge(signer.device());
        self.library.commit_judged(
            Commit { scope: self.scope.clone(), stream: self.stream.clone(), records: vec![record], leases, durable: true, op, holding },
            signer,
            judge,
        )
    }

    /// The kind's rule, judging a record in this scope and stream against its
    /// item as it stands in the commit's own transaction.
    fn judge(&self, device: [u8; 32]) -> super::commit::Judge {
        let (scope, stream) = (self.scope.clone(), self.stream.clone());
        Box::new(move |before, record| K::authorize(&Actor { device, scope: &scope, stream: &stream }, before, record))
    }

    /// Writes several items in one commit, each judged as `write` judges
    /// one; `leases` on their objects let go with it. A producer's commit
    /// waits for the disk only when `durable`.
    pub fn write_all(&self, writes: Vec<Write<K>>, leases: Vec<Lease>, durable: bool, signer: Arc<dyn Signer>) -> Result<Committed> {
        let holding = self.library.holding(&self.scope)?.ok_or_else(|| Error::Invalid("the scope is not held here".into()))?;
        let records = writes
            .into_iter()
            .map(|write| {
                let state = match &write.value {
                    Some(value) => State::Is { body: K::encode(value), objects: write.objects },
                    None => State::Gone,
                };
                KindRecord { kind: K::KIND, schema: K::SCHEMA, record: Record { item: write.item, order: write.order, state } }
            })
            .collect();
        let judge = self.judge(signer.device());
        self.library.commit_judged(
            Commit { scope: self.scope.clone(), stream: self.stream.clone(), records, leases, durable, op: None, holding },
            signer,
            judge,
        )
    }

    /// The item `item` as it stands here, decoded.
    pub fn read(&self, item: &[u8]) -> Result<Option<K::Value>> {
        let Some(entry) = self.library.item(&self.scope, &self.stream, item)? else { return Ok(None) };
        K::decode(&entry.body).map(Some).map_err(Error::Invalid)
    }
}
