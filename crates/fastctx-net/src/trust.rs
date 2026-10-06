//! Who belongs to the World, and what each member may do, decided on each
//! machine for itself.
//!
//! The hub keeps the list of members and hands it out, but no machine takes
//! the hub's word for who is a member: a hub that could add a member could
//! speak as one. Every machine therefore checks the list itself, from one
//! root it learned outside the hub.
//!
//! The root is the founder, the first machine, whose key every machine
//! learns from the code it joined with. Every other member was invited by a
//! member: the inviter signed an endorsement of a one-time invitation key in
//! advance, and the joining machine, holding that key's seed from the code,
//! signed an admission naming itself and its own key. So each member's
//! record leads back to the founder through signatures the hub could not
//! have made. Each machine also remembers every name it has seen and the
//! key it had, and refuses a different key for a known name; and it refuses
//! both machines when one invitation admitted two, which happens only when
//! an invitation's seed leaked. A member's record also carries its sealing
//! key, which others encrypt to (`crate::seal`); a member of a World older
//! than sealing republishes its record with one, signed with the same key.
//!
//! What each member may do comes from the grants of members with
//! management rights (`crate::scope`). The founder is born with them; who
//! holds them later, and every member's scope, follow from the grants and
//! revocations taken in the order they were made. A member invited by a
//! member without management rights is that member's branch: it gets at
//! most its inviter's scope, never the right to invite, and it stands only
//! while its inviter does.
//!
//! A member that is lost is revoked: a statement that a key is no longer
//! trusted, signed by a member with management rights, by the member
//! itself, or by the member whose branch it is in. Revocations are kept
//! forever, and they name the key, not the name, so a revoked key is not
//! trusted under any name.
//!
//! A member is a machine, which runs a node and its residents, or a device,
//! such as a phone, which only carries the user's own words and looks. What
//! a member is was signed by its inviter, never said by the member itself.
//! A device invites nobody, manages nothing, and the only key it can revoke
//! is its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Bound;

use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ledger::TownId;
use crate::scope::{Grant, Scope};
use crate::seal::SealPublic;
use crate::sign::{Digest, Domain, PublicKey, Signed, sha256};
use crate::tree::{RelayOffer, Route, Upstream};

/// How far apart the clocks of the members and the hub are taken to be:
/// Kerberos's default tolerance for clocks kept by NTP. The hub takes an
/// invitation, grant or revocation only when it is dated within this of its
/// own clock; a ledger entry is judged by the standing its author had
/// within this after the time it is dated.
pub const CLOCK_TOLERANCE: jiff::SignedDuration = jiff::SignedDuration::from_mins(5);

/// Which World: one hub and the machines that joined it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorldId(String);

impl WorldId {
    pub fn generate() -> std::io::Result<Self> {
        let bytes = crate::sign::random::<8>()?;
        Ok(Self(format!("w-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>())))
    }

    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(format!("w-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
    }

    /// The eight bytes the id spells.
    pub fn bytes(&self) -> [u8; 8] {
        let mut bytes = [0_u8; 8];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.0[2 + index * 2..4 + index * 2], 16).expect("checked when made");
        }
        bytes
    }
}

impl TryFrom<String> for WorldId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let ok = text.len() == 18
            && text.starts_with("w-")
            && text[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if ok { Ok(Self(text)) } else { Err(format!("{text:?} is not a World id")) }
    }
}

impl From<WorldId> for String {
    fn from(id: WorldId) -> String {
        id.0
    }
}

impl fmt::Display for WorldId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for WorldId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// The hub's certificate, by the SHA-256 of its DER bytes. Machines reach
/// the hub by its IP address and accept exactly this certificate; no
/// certificate authority and no domain name are involved.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertPin(pub Digest);

impl CertPin {
    pub fn of(certificate_der: &[u8]) -> Self {
        Self(Digest(sha256(certificate_der)))
    }
}

impl fmt::Debug for CertPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CertPin({})", self.0)
    }
}

impl fmt::Display for CertPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// What a member is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Runs a node: residents live on it, and the World works on it.
    #[default]
    Machine,
    /// Carries the user's words and looks, and nothing runs on it.
    Device,
}

impl Kind {
    pub fn is_machine(&self) -> bool {
        *self == Kind::Machine
    }
}

/// An inviter's endorsement of whoever holds the invitation key `invite`,
/// signed by the inviter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endorsement {
    pub world: WorldId,
    pub inviter: MachineName,
    pub invite: PublicKey,
    /// After this the hub admits nobody with it.
    pub expires: Timestamp,
    /// The name the joining machine must take, when the inviter chose one.
    pub name: Option<MachineName>,
    /// What the invitation admits. Left out, as before devices, a machine.
    #[serde(default, skip_serializing_if = "Kind::is_machine")]
    pub kind: Kind,
    /// What the member may do until a grant says otherwise. Left out, as
    /// before scopes, everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// When the inviter made it: whether the inviter held management
    /// rights then decides whether the member is the inviter's branch.
    /// Left out, as before scopes, it counts as made when the World began,
    /// when only the founder held them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
}

/// The invitation's holder naming the machine it admits, signed with the
/// invitation key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admission {
    pub endorsement: Signed,
    pub name: MachineName,
    pub key: PublicKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Credential {
    /// The first machine; its key is the root every machine pins.
    Founder,
    Invited {
        admission: Signed,
    },
}

/// A machine's record of itself, signed with its own key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub world: WorldId,
    pub name: MachineName,
    pub key: PublicKey,
    pub credential: Credential,
    /// The key others seal what they send it to. Left out by a member that
    /// joined before sealing, until it republishes its record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal: Option<SealPublic>,
    /// Where it can be reached as a relay, while it is one
    /// (`crate::tree`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<RelayOffer>,
}

/// A statement that a key is no longer trusted, signed by a member allowed
/// to make it. A machine leaving the World revokes itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    pub world: WorldId,
    pub name: MachineName,
    pub key: PublicKey,
    pub by: MachineName,
    pub at: Timestamp,
    pub reason: String,
}

/// What a machine has accepted before and holds to: every name it has seen
/// with the key it had, and every revocation it has accepted, in the order
/// it accepted them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    pub names: BTreeMap<MachineName, PublicKey>,
    pub revocations: Vec<Signed>,
}

/// A trusted member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: MachineName,
    pub key: PublicKey,
    /// Its sealing key; `None` until it published one.
    pub seal: Option<SealPublic>,
    /// Where it is reached as a relay, while it is one.
    pub relay: Option<RelayOffer>,
    /// Who invited it; `None` for the founder.
    pub invited_by: Option<MachineName>,
    /// What its inviter admitted it as; the founder is a machine.
    pub kind: Kind,
    /// Whether it holds management rights.
    pub manage: bool,
    /// What it may do, all limits applied: management rights cover
    /// everything.
    pub scope: Scope,
    /// Whether it is the branch of an inviter without management rights.
    pub delegated: bool,
}

/// A member that was trusted once and is not any more: revoked, or cut off
/// with the branch it hung on. What it signed while it was trusted stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Former {
    pub key: PublicKey,
    pub kind: Kind,
    /// When it stopped being trusted, when that is known: what it says it
    /// signed after is not believed.
    pub until: Option<Timestamp>,
}

/// The members this machine trusts, as checked from the hub's list.
#[derive(Clone, Debug, Default)]
pub struct Table {
    trusted: BTreeMap<MachineName, Member>,
    /// Names that were members and are revoked, with who revoked them.
    revoked: BTreeMap<MachineName, Revocation>,
    /// Members that were trusted once, revoked or cut off.
    former: BTreeMap<MachineName, Former>,
    /// The latest grant taken for each member.
    grants: BTreeMap<MachineName, Grant>,
    /// The latest route taken for each member.
    routes: BTreeMap<MachineName, Route>,
    /// What each member was granted when, for what it could do at a time.
    history: History,
    /// Records in the hub's list that were refused, and why.
    pub refused: Vec<(String, String)>,
}

impl Table {
    pub fn get(&self, name: &MachineName) -> Option<&Member> {
        self.trusted.get(name)
    }

    pub fn by_key(&self, key: &PublicKey) -> Option<&Member> {
        self.trusted.values().find(|member| member.key == *key)
    }

    pub fn members(&self) -> impl Iterator<Item = &Member> {
        self.trusted.values()
    }

    /// The members that are machines: the ones the World asks and works on.
    pub fn machines(&self) -> impl Iterator<Item = &Member> {
        self.trusted.values().filter(|member| member.kind.is_machine())
    }

    pub fn managers(&self) -> impl Iterator<Item = &Member> {
        self.trusted.values().filter(|member| member.manage)
    }

    pub fn revoked(&self, name: &MachineName) -> Option<&Revocation> {
        self.revoked.get(name)
    }

    /// The members whose keys are revoked, with their revocations.
    pub fn revoked_members(&self) -> impl Iterator<Item = (&MachineName, &Revocation)> {
        self.revoked.iter()
    }

    /// The key of `name` for something it signed at `at`: a member now, or
    /// one that was then.
    pub fn key_at(&self, name: &MachineName, at: Timestamp) -> Option<PublicKey> {
        match (self.trusted.get(name), self.former.get(name)) {
            (Some(member), _) => Some(member.key),
            (None, Some(former)) if former.until.is_none_or(|until| at < until) => Some(former.key),
            _ => None,
        }
    }

    /// The key of `name`, a member now or once: for what it signed with no
    /// time of its own, which the hub took from it while it was a member.
    pub fn key_ever(&self, name: &MachineName) -> Option<PublicKey> {
        self.trusted.get(name).map(|member| member.key).or_else(|| self.former.get(name).map(|former| former.key))
    }

    /// The key of `name` for an entry it says it wrote at `at` in a
    /// ledger: a machine, trusted then. A device writes nothing in a
    /// ledger: what the user writes from one, a machine's node writes for
    /// them.
    pub fn author_key(&self, name: &MachineName, at: Timestamp) -> Result<PublicKey, String> {
        let kind = match (self.trusted.get(name), self.former.get(name)) {
            (Some(member), _) => member.kind,
            (None, Some(former)) => former.kind,
            (None, None) => return Err(format!("{name} is not a member, nor was it")),
        };
        if !kind.is_machine() {
            return Err(format!("{name} is a device, and a device writes nothing in the ledger"));
        }
        self.key_at(name, at).ok_or_else(|| format!("{name} wrote this after it stopped being a member"))
    }

    /// The latest route taken for `name`.
    pub fn route(&self, name: &MachineName) -> Option<&Route> {
        self.routes.get(name)
    }

    /// The upstreams `name` connects to, in the order it tries them.
    pub fn upstreams(&self, name: &MachineName) -> Vec<Upstream> {
        self.routes.get(name).map(|route| route.via.clone()).unwrap_or_else(Route::direct)
    }

    /// Whether the relay `relay` carries what `name` says: it is the relay
    /// itself, or `name`'s routes lead up through it. A relay that claims to
    /// carry a member any other way is not believed.
    pub fn carries(&self, relay: &MachineName, name: &MachineName) -> bool {
        let mut seen = BTreeSet::new();
        let mut below = vec![name.clone()];
        while let Some(member) = below.pop() {
            if member == *relay {
                return true;
            }
            if !seen.insert(member.clone()) {
                continue;
            }
            for upstream in self.upstreams(&member) {
                if let Upstream::Relay { machine } = upstream {
                    below.push(machine);
                }
            }
        }
        false
    }

    /// The latest grant taken for `name`.
    pub fn grant(&self, name: &MachineName) -> Option<&Grant> {
        self.grants.get(name)
    }

    /// Who may read `town`: the members with management rights, and those
    /// whose scope includes it. The World's own ledger is everyone's.
    pub fn readers(&self, town: &TownId) -> impl Iterator<Item = &Member> {
        // Reading needs no action: what a member may see of a project, it
        // may read.
        let world = *town == TownId::world();
        self.trusted.values().filter(move |member| world || member.manage || member.scope.projects.contains(town))
    }

    /// Whether `name` could write in `town` when it says it did, at `at`:
    /// it held management rights then, or its scope then included the town
    /// and writing.
    pub fn could_write(&self, name: &MachineName, town: &TownId, at: Timestamp) -> bool {
        self.around(at, |at| {
            self.history.scope_at(name, at, 0).is_some_and(|(scope, manage)| manage || scope.writes(town))
        })
    }

    /// Whether `name` may write in `town` now.
    pub fn may_write(&self, name: &MachineName, town: &TownId) -> bool {
        self.trusted.get(name).is_some_and(|member| {
            member.kind.is_machine() && (member.manage || *town == TownId::world() || member.scope.writes(town))
        })
    }

    /// Whether `name` held management rights when it says it wrote
    /// something, at `at`.
    pub fn managed_at(&self, name: &MachineName, at: Timestamp) -> bool {
        self.around(at, |at| self.history.held_at(name, at))
    }

    /// Whether `test` holds at some moment within `CLOCK_TOLERANCE` of `at`.
    /// What a member writes in a ledger is dated by its own clock, and a
    /// change to its standing by the clock of the member that signed it;
    /// either may run ahead of the other, and the hub took the entry by the
    /// standing the writer had. Being judged at any other moment is no help
    /// to a writer: it chooses the date anyway. Standing changes only at a
    /// change to the members, so those moments, and the window's start, are
    /// all there is to test.
    fn around(&self, at: Timestamp, test: impl Fn(Timestamp) -> bool) -> bool {
        let from = at.saturating_sub(CLOCK_TOLERANCE).unwrap_or(at);
        let until = at.saturating_add(CLOCK_TOLERANCE).unwrap_or(at);
        test(from)
            || test(at)
            || self.history.changes.range((Bound::Excluded(from), Bound::Included(until))).any(|change| test(*change))
    }

    /// Whether `by` may revoke `name`: a member with management rights may
    /// revoke anyone, a member itself, and a member the members of its own
    /// branch.
    pub fn may_revoke(&self, by: &MachineName, name: &MachineName) -> bool {
        let Some(signer) = self.trusted.get(by) else { return false };
        if by == name || signer.manage {
            return true;
        }
        signer.kind.is_machine()
            && self.trusted.get(name).is_some_and(|member| member.delegated && member.invited_by.as_ref() == Some(by))
    }

    /// Whether `name` may invite: with management rights, any scope; with
    /// the right to invite, within its own.
    pub fn may_invite(&self, name: &MachineName, scope: &Scope) -> Result<(), String> {
        let member = self.trusted.get(name).ok_or_else(|| format!("{name} is not a member"))?;
        if !member.kind.is_machine() {
            return Err(format!("{name} is a device, and a device invites nobody"));
        }
        if member.manage {
            return Ok(());
        }
        if !member.scope.invite {
            return Err(format!("{name} may not invite"));
        }
        if !scope.within(&member.scope) {
            return Err(format!("{name} may invite only within its own scope"));
        }
        Ok(())
    }

    /// Whether `by` may seal an entry's key once more for `to`: a member
    /// with management rights for anyone, an inviter for its own branch.
    pub fn may_share(&self, by: &MachineName, to: &MachineName) -> bool {
        let Some(signer) = self.trusted.get(by) else { return false };
        signer.manage
            || self.trusted.get(to).is_some_and(|member| member.delegated && member.invited_by.as_ref() == Some(by))
    }
}

/// Checks the hub's list of records, grants and revocations against the
/// founder's key and what this machine has accepted before; returns the
/// members to trust, and what to remember from now on.
pub fn check(
    world: &WorldId,
    founder: &PublicKey,
    records: &[Signed],
    revocations: &[Signed],
    grants: &[Signed],
    routes: &[Signed],
    known: &Known,
) -> (Table, Known) {
    let mut table = Table::default();
    let mut known = known.clone();

    let records = one_record_per_name(world, records, &known, &mut table.refused);

    // Keys this machine already knew to be revoked. A revoked key may still
    // sign endorsements; a machine first seen after the revocation was
    // accepted, invited by that key, is not admitted.
    let prior_revoked: BTreeSet<PublicKey> = known
        .revocations
        .iter()
        .filter_map(|signed| signed.claim::<Revocation>().ok())
        .map(|revocation| revocation.key)
        .collect();

    let mut accepted = accept_invited(world, founder, records, &known, &prior_revoked, &mut table.refused);

    // One invitation admits one machine; when it admitted more, its seed
    // leaked, and none of them is trusted.
    let mut by_invite: BTreeMap<PublicKey, Vec<MachineName>> = BTreeMap::new();
    for (name, joined) in &accepted {
        if let Some(invite) = &joined.invite {
            by_invite.entry(*invite).or_default().push(name.clone());
        }
    }
    for names in by_invite.values().filter(|names| names.len() > 1) {
        for name in names {
            accepted.remove(name);
            table.refused.push((name.to_string(), format!("its invitation also admitted {}", others(names, name))));
        }
    }

    for (name, joined) in &accepted {
        known.names.entry(name.clone()).or_insert(joined.record.key);
    }

    let grants = open_grants(world, grants, &accepted, &mut table.refused);
    let routes = open_routes(world, routes, &accepted, &mut table.refused);
    let revocations = open_revocations(world, revocations, &known, &accepted, &mut table.refused);
    let standing = replay(&accepted, &grants, &revocations, &routes, &mut table.refused);
    for (signed, _) in standing.revocations.values() {
        if !known.revocations.contains(signed) {
            known.revocations.push(signed.clone());
        }
    }

    let mut scopes: BTreeMap<MachineName, Result<(Scope, bool), String>> = BTreeMap::new();
    for name in accepted.keys() {
        if !standing.revocations.contains_key(&accepted[name].record.key) {
            // Kept in `scopes`, with the branch it hangs on worked out first.
            let _ = standing.scope(name, &accepted, &mut scopes);
        }
    }
    for (name, joined) in &accepted {
        if let Some((_, revocation)) = standing.revocations.get(&joined.record.key) {
            table.revoked.insert(name.clone(), revocation.clone());
            table
                .former
                .insert(name.clone(), Former { key: joined.record.key, kind: joined.kind, until: Some(revocation.at) });
            continue;
        }
        match scopes.remove(name) {
            Some(Ok((scope, delegated))) => {
                let member = Member {
                    name: name.clone(),
                    key: joined.record.key,
                    seal: joined.record.seal,
                    relay: joined.record.relay.clone(),
                    invited_by: joined.invited_by.clone(),
                    kind: joined.kind,
                    manage: standing.history.managers.contains(name),
                    scope,
                    delegated,
                };
                table.trusted.insert(name.clone(), member);
            }
            Some(Err(reason)) => {
                // Cut off with its branch: trusted until its inviter was
                // revoked, when that is when.
                let until = joined
                    .invited_by
                    .as_ref()
                    .and_then(|inviter| accepted.get(inviter))
                    .and_then(|inviter| standing.revocations.get(&inviter.record.key))
                    .map(|(_, revocation)| revocation.at);
                table.former.insert(name.clone(), Former { key: joined.record.key, kind: joined.kind, until });
                table.refused.push((name.to_string(), reason));
            }
            None => {}
        }
    }
    table.grants = standing.latest.into_iter().filter(|(name, _)| table.trusted.contains_key(name)).collect();
    table.routes = standing.routes.into_iter().filter(|(name, _)| table.trusted.contains_key(name)).collect();
    table.history = standing.history;
    (table, known)
}

/// One record per name: signed by the key it names, for this World, with
/// the key the name was first seen with. A member that republished its
/// record with a sealing key has two records with one key; the one with
/// the sealing key counts.
fn one_record_per_name(
    world: &WorldId,
    records: &[Signed],
    known: &Known,
    refused: &mut Vec<(String, String)>,
) -> BTreeMap<MachineName, Record> {
    let mut candidates: BTreeMap<MachineName, Vec<Record>> = BTreeMap::new();
    for signed in records {
        let record = match signed.claim::<Record>() {
            Ok(record) => record,
            Err(error) => {
                refused.push(("a record".to_string(), error.to_string()));
                continue;
            }
        };
        let name = record.name.to_string();
        match signed.open::<Record>(&record.key, Domain::Record) {
            Ok(_) if record.world != *world => refused.push((name, format!("it is for the World {}", record.world))),
            Ok(record) => candidates.entry(record.name.clone()).or_default().push(record),
            Err(error) => refused.push((name, error.to_string())),
        }
    }
    let mut chosen = BTreeMap::new();
    for (name, list) in candidates {
        let list: Vec<Record> = match known.names.get(&name) {
            Some(key) => {
                let (same, other): (Vec<Record>, Vec<Record>) = list.into_iter().partition(|record| record.key == *key);
                if !other.is_empty() {
                    refused.push((name.to_string(), "it has a different key than when it was first seen".to_string()));
                }
                same
            }
            None => list,
        };
        let keys: BTreeSet<PublicKey> = list.iter().map(|record| record.key).collect();
        if keys.len() > 1 {
            refused.push((name.to_string(), "the hub listed it with several keys".to_string()));
            continue;
        }
        let seals: BTreeSet<SealPublic> = list.iter().filter_map(|record| record.seal).collect();
        let credentials: Vec<&Credential> = list.iter().map(|record| &record.credential).collect();
        if credentials.windows(2).any(|pair| pair[0] != pair[1]) {
            refused.push((name.to_string(), "its records name different credentials".to_string()));
            continue;
        }
        let Some(mut record) = list.into_iter().next() else { continue };
        record.seal = match seals.len() {
            0 => None,
            1 => seals.into_iter().next(),
            _ => {
                refused.push((
                    name.to_string(),
                    "it published several sealing keys, so nothing can be sealed to it".to_string(),
                ));
                None
            }
        };
        chosen.insert(name, record);
    }
    chosen
}

/// A member whose record checked out, before scopes.
struct Joined {
    record: Record,
    invited_by: Option<MachineName>,
    invite: Option<PublicKey>,
    kind: Kind,
    endorsement: Option<Endorsement>,
}

/// Accepts the founder, then whoever was invited by an accepted member,
/// until nobody more can be accepted.
fn accept_invited(
    world: &WorldId,
    founder: &PublicKey,
    records: BTreeMap<MachineName, Record>,
    known: &Known,
    prior_revoked: &BTreeSet<PublicKey>,
    refused: &mut Vec<(String, String)>,
) -> BTreeMap<MachineName, Joined> {
    let mut accepted: BTreeMap<MachineName, Joined> = BTreeMap::new();
    let mut waiting: Vec<Record> = Vec::new();
    for record in records.into_values() {
        match &record.credential {
            Credential::Founder if record.key == *founder => {
                let joined = Joined { record, invited_by: None, invite: None, kind: Kind::Machine, endorsement: None };
                accepted.insert(joined.record.name.clone(), joined);
            }
            Credential::Founder => {
                refused.push((record.name.to_string(), "it claims to be the founder with another key".to_string()))
            }
            Credential::Invited { .. } => waiting.push(record),
        }
    }
    loop {
        let mut progressed = false;
        let mut still = Vec::new();
        for record in waiting {
            let Credential::Invited { admission } = &record.credential else {
                unreachable!("only invited records wait")
            };
            match admitted(world, &record, admission, &accepted) {
                Admitted::Yes(endorsement) => {
                    let inviter = &accepted[&endorsement.inviter];
                    if !inviter.kind.is_machine() {
                        refused.push((
                            record.name.to_string(),
                            format!("it was invited by {}, a device", endorsement.inviter),
                        ));
                    } else if !known.names.contains_key(&record.name) && prior_revoked.contains(&inviter.record.key) {
                        refused.push((
                            record.name.to_string(),
                            format!("it was invited by {0} after {0} was revoked", endorsement.inviter),
                        ));
                    } else {
                        let joined = Joined {
                            invited_by: Some(endorsement.inviter.clone()),
                            invite: Some(endorsement.invite),
                            kind: endorsement.kind,
                            endorsement: Some(*endorsement),
                            record,
                        };
                        accepted.insert(joined.record.name.clone(), joined);
                        progressed = true;
                    }
                }
                Admitted::NotYet => still.push(record),
                Admitted::No(reason) => refused.push((record.name.to_string(), reason)),
            }
        }
        waiting = still;
        if !progressed {
            break;
        }
    }
    for record in waiting {
        refused.push((record.name.to_string(), "its inviter is not a member".to_string()));
    }
    accepted
}

enum Admitted {
    Yes(Box<Endorsement>),
    /// The inviter has not been accepted yet; it may be later in this pass.
    NotYet,
    No(String),
}

fn admitted(
    world: &WorldId,
    record: &Record,
    admission: &Signed,
    accepted: &BTreeMap<MachineName, Joined>,
) -> Admitted {
    let claimed = match admission.claim::<Admission>() {
        Ok(admission) => admission,
        Err(error) => return Admitted::No(format!("its admission {error}")),
    };
    let endorsement = match claimed.endorsement.claim::<Endorsement>() {
        Ok(endorsement) => endorsement,
        Err(error) => return Admitted::No(format!("its endorsement {error}")),
    };
    let Some(inviter) = accepted.get(&endorsement.inviter) else {
        return Admitted::NotYet;
    };
    let endorsement = match claimed.endorsement.open::<Endorsement>(&inviter.record.key, Domain::Endorsement) {
        Ok(endorsement) => endorsement,
        Err(error) => return Admitted::No(format!("its endorsement by {}: {error}", inviter.record.name)),
    };
    if endorsement.world != *world {
        return Admitted::No(format!("it was invited into the World {}", endorsement.world));
    }
    let admission = match admission.open::<Admission>(&endorsement.invite, Domain::Admission) {
        Ok(admission) => admission,
        Err(error) => return Admitted::No(format!("its admission {error}")),
    };
    if admission.name != record.name || admission.key != record.key {
        return Admitted::No("its admission names another machine or key".to_string());
    }
    if let Some(name) = &endorsement.name
        && *name != record.name
    {
        return Admitted::No(format!("it was invited as {name}"));
    }
    Admitted::Yes(Box::new(endorsement))
}

/// The grants signed by the member they say they are by, for a member of
/// this World with the key they name.
fn open_grants(
    world: &WorldId,
    grants: &[Signed],
    accepted: &BTreeMap<MachineName, Joined>,
    refused: &mut Vec<(String, String)>,
) -> Vec<(Grant, [u8; 32])> {
    let mut opened = Vec::new();
    for signed in grants {
        let grant = match signed.claim::<Grant>() {
            Ok(grant) => grant,
            Err(error) => {
                refused.push(("a grant".to_string(), error.to_string()));
                continue;
            }
        };
        let label = format!("the grant {} for {} by {}", grant.version, grant.member, grant.by);
        let Some(signer) = accepted.get(&grant.by) else {
            refused.push((label, "its signer is not a member".to_string()));
            continue;
        };
        match signed.open::<Grant>(&signer.record.key, Domain::Grant) {
            Ok(grant) if grant.world != *world => refused.push((label, format!("it is for the World {}", grant.world))),
            Ok(grant) if accepted.get(&grant.member).is_none_or(|member| member.record.key != grant.key) => {
                refused.push((label, "it names a member that is not here with that key".to_string()))
            }
            Ok(grant) => opened.push((grant, sha256(&signed.data.0))),
            Err(error) => refused.push((label, error.to_string())),
        }
    }
    opened
}

/// The routes signed by the member they say they are by, for a member of
/// this World with the key they name.
fn open_routes(
    world: &WorldId,
    routes: &[Signed],
    accepted: &BTreeMap<MachineName, Joined>,
    refused: &mut Vec<(String, String)>,
) -> Vec<(Route, [u8; 32])> {
    let mut opened = Vec::new();
    for signed in routes {
        let route = match signed.claim::<Route>() {
            Ok(route) => route,
            Err(error) => {
                refused.push(("a route".to_string(), error.to_string()));
                continue;
            }
        };
        let label = format!("the route {} for {} by {}", route.version, route.member, route.by);
        let Some(signer) = accepted.get(&route.by) else {
            refused.push((label, "its signer is not a member".to_string()));
            continue;
        };
        match signed.open::<Route>(&signer.record.key, Domain::Route) {
            Ok(route) if route.world != *world => refused.push((label, format!("it is for the World {}", route.world))),
            Ok(route) if accepted.get(&route.member).is_none_or(|member| member.record.key != route.key) => {
                refused.push((label, "it names a member that is not here with that key".to_string()))
            }
            Ok(route) => opened.push((route, sha256(&signed.data.0))),
            Err(error) => refused.push((label, error.to_string())),
        }
    }
    opened
}

/// A revocation signed by the member it says it is by, of a member of this
/// World by the key it names; whether this machine accepted it before.
struct Opened {
    revocation: Revocation,
    signed: Signed,
    accepted_before: bool,
}

fn open_revocations(
    world: &WorldId,
    revocations: &[Signed],
    known: &Known,
    accepted: &BTreeMap<MachineName, Joined>,
    refused: &mut Vec<(String, String)>,
) -> Vec<Opened> {
    let mut opened = Vec::new();
    let mut seen = BTreeSet::new();
    for (signed, accepted_before) in
        known.revocations.iter().map(|signed| (signed, true)).chain(revocations.iter().map(|signed| (signed, false)))
    {
        if !seen.insert(signed.data.0.clone()) {
            continue;
        }
        let claimed = match signed.claim::<Revocation>() {
            Ok(revocation) => revocation,
            Err(error) => {
                refused.push(("a revocation".to_string(), error.to_string()));
                continue;
            }
        };
        let label = format!("the revocation of {} by {}", claimed.name, claimed.by);
        let Some(signer) = accepted.get(&claimed.by) else {
            // One accepted before stands even if its signer is no longer
            // listed: it named a key, and that key stays revoked.
            if accepted_before {
                opened.push(Opened { revocation: claimed, signed: signed.clone(), accepted_before });
            } else {
                refused.push((label, "its signer is not a member".to_string()));
            }
            continue;
        };
        match signed.open::<Revocation>(&signer.record.key, Domain::Revocation) {
            Ok(revocation) if revocation.world == *world => {
                opened.push(Opened { revocation, signed: signed.clone(), accepted_before })
            }
            Ok(revocation) => refused.push((label, format!("it is for the World {}", revocation.world))),
            Err(error) if accepted_before => {
                refused.push((label, error.to_string()));
            }
            Err(error) => refused.push((label, error.to_string())),
        }
    }
    opened
}

/// Who invited a member, the scope the invitation gave it, and when it was
/// made.
#[derive(Clone, Debug)]
struct Invited {
    by: Option<MachineName>,
    scope: Option<Scope>,
    at: Option<Timestamp>,
}

/// What each member was granted when, and when it held management rights.
#[derive(Clone, Debug, Default)]
struct History {
    managers: BTreeSet<MachineName>,
    /// When each member held management rights.
    held: BTreeMap<MachineName, Vec<(Timestamp, Option<Timestamp>)>>,
    /// Each member's grants taken, in order: from when, which scope.
    scopes: BTreeMap<MachineName, Vec<(Timestamp, Scope)>>,
    /// How each member was invited.
    invited: BTreeMap<MachineName, Invited>,
    /// When the grants and revocations taken were made.
    changes: BTreeSet<Timestamp>,
}

impl History {
    fn promote(&mut self, name: &MachineName, at: Timestamp) {
        if self.managers.insert(name.clone()) {
            self.held.entry(name.clone()).or_default().push((at, None));
        }
    }

    fn demote(&mut self, name: &MachineName, at: Timestamp) {
        if self.managers.remove(name)
            && let Some(last) = self.held.get_mut(name).and_then(|held| held.last_mut())
        {
            last.1 = Some(at);
        }
    }

    /// Whether `name` held management rights at `at`.
    fn held_at(&self, name: &MachineName, at: Timestamp) -> bool {
        self.held
            .get(name)
            .is_some_and(|held| held.iter().any(|(from, until)| *from <= at && until.is_none_or(|until| at < until)))
    }

    /// Whether `name` is the branch of its inviter: invited by a member
    /// that did not hold management rights when it made the invitation.
    /// An invitation from before scopes, which says no time, counts as made
    /// when the World began, when only the founder held them.
    fn delegated(&self, name: &MachineName) -> bool {
        match self.invited.get(name) {
            Some(Invited { by: Some(inviter), at, .. }) => !self.held_at(inviter, at.unwrap_or(Timestamp::MIN)),
            _ => false,
        }
    }

    /// What `name` could do at `at`, and whether it managed then; `None`
    /// when its branch did not allow it anything then. `depth` guards the
    /// walk up a branch, which the one level of delegation keeps short.
    fn scope_at(&self, name: &MachineName, at: Timestamp, depth: usize) -> Option<(Scope, bool)> {
        if self.held_at(name, at) {
            return Some((Scope::all(), true));
        }
        let Invited { by: inviter, scope: invited, .. } = self.invited.get(name)?;
        let base = self
            .scopes
            .get(name)
            .and_then(|scopes| scopes.iter().rev().find(|(from, _)| *from <= at))
            .map(|(_, scope)| scope.clone())
            .or_else(|| invited.clone())
            .unwrap_or_else(Scope::all);
        if !self.delegated(name) {
            return Some((base, false));
        }
        let inviter = inviter.as_ref()?;
        if depth > 2 {
            return None;
        }
        let (theirs, _) = self.scope_at(inviter, at, depth + 1)?;
        if !theirs.invite || self.delegated(inviter) {
            return None;
        }
        let mut scope = base.intersect(&theirs);
        scope.invite = false;
        Some((scope, false))
    }
}

/// Who holds management rights, what each member was granted, and which
/// revocations stand, from the grants and revocations taken in the order
/// they were made.
struct Standing {
    history: History,
    latest: BTreeMap<MachineName, Grant>,
    routes: BTreeMap<MachineName, Route>,
    /// The revocations that stand, by the key they revoke.
    revocations: BTreeMap<PublicKey, (Signed, Revocation)>,
}

enum Event<'a> {
    Revoked(&'a Opened),
    Granted(&'a Grant),
    Routed(&'a Route),
}

/// Replays grants, routes and revocations in the order they were made: a
/// revocation before a grant made at the same instant, a grant before a
/// route, and those made at one instant by the digest of their signed bytes,
/// so every machine reaches the same answer whatever order the hub lists
/// them in. A grant or a route counts when its signer held management rights
/// then and it is the next version for its member; a revocation counts when
/// its signer could revoke that member then, or when this machine accepted
/// it before.
fn replay(
    accepted: &BTreeMap<MachineName, Joined>,
    grants: &[(Grant, [u8; 32])],
    revocations: &[Opened],
    routes: &[(Route, [u8; 32])],
    refused: &mut Vec<(String, String)>,
) -> Standing {
    let mut history = History::default();
    for (name, joined) in accepted {
        let endorsement = joined.endorsement.as_ref();
        history.invited.insert(
            name.clone(),
            Invited {
                by: joined.invited_by.clone(),
                scope: endorsement.and_then(|endorsement| endorsement.scope.clone()),
                at: endorsement.and_then(|endorsement| endorsement.at),
            },
        );
        if joined.invited_by.is_none() {
            history.promote(name, Timestamp::MIN);
        }
    }
    let mut standing =
        Standing { history, latest: BTreeMap::new(), routes: BTreeMap::new(), revocations: BTreeMap::new() };
    let mut events: Vec<(Timestamp, u8, [u8; 32], Event)> = Vec::new();
    for opened in revocations {
        events.push((opened.revocation.at, 0, sha256(&opened.signed.data.0), Event::Revoked(opened)));
    }
    for (grant, digest) in grants {
        events.push((grant.at, 1, *digest, Event::Granted(grant)));
    }
    for (route, digest) in routes {
        events.push((route.at, 2, *digest, Event::Routed(route)));
    }
    events.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    let key_of = |name: &MachineName| accepted.get(name).map(|joined| joined.record.key);
    for (at, _, _, event) in events {
        match event {
            Event::Revoked(opened) => {
                let revocation = &opened.revocation;
                if standing.revocations.contains_key(&revocation.key) {
                    continue;
                }
                let signer_revoked = key_of(&revocation.by).is_some_and(|key| standing.revocations.contains_key(&key));
                let target = accepted.get(&revocation.name).filter(|joined| joined.record.key == revocation.key);
                let may = opened.accepted_before
                    || (!signer_revoked
                        && target.is_some()
                        && (standing.history.managers.contains(&revocation.by)
                            || revocation.by == revocation.name
                            || (target.is_some_and(|target| target.invited_by.as_ref() == Some(&revocation.by))
                                && standing.history.delegated(&revocation.name)
                                && !standing.history.managers.contains(&revocation.name)
                                && accepted.get(&revocation.by).is_some_and(|signer| signer.kind.is_machine()))));
                if !may {
                    refused.push((
                        format!("the revocation of {} by {}", revocation.name, revocation.by),
                        format!("{} could not revoke {} then", revocation.by, revocation.name),
                    ));
                    continue;
                }
                standing.revocations.insert(revocation.key, (opened.signed.clone(), revocation.clone()));
                if let Some(target) = target {
                    standing.history.demote(&target.record.name, at);
                }
                standing.history.changes.insert(at);
            }
            Event::Granted(grant) => {
                let signer_revoked = key_of(&grant.by).is_some_and(|key| standing.revocations.contains_key(&key));
                let member_revoked = key_of(&grant.member).is_some_and(|key| standing.revocations.contains_key(&key));
                let label = format!("the grant {} for {} by {}", grant.version, grant.member, grant.by);
                if signer_revoked || member_revoked || !standing.history.managers.contains(&grant.by) {
                    refused.push((label, format!("{} did not hold management rights then", grant.by)));
                    continue;
                }
                let next = standing.latest.get(&grant.member).map(|latest| latest.version + 1).unwrap_or(1);
                if grant.version != next {
                    refused.push((label, format!("the next grant for {} was version {next}", grant.member)));
                    continue;
                }
                let machine = accepted.get(&grant.member).is_some_and(|joined| joined.kind.is_machine());
                if grant.manage && machine {
                    standing.history.promote(&grant.member, at);
                } else {
                    standing.history.demote(&grant.member, at);
                }
                standing.history.scopes.entry(grant.member.clone()).or_default().push((at, grant.scope.clone()));
                standing.history.changes.insert(at);
                standing.latest.insert(grant.member.clone(), grant.clone());
            }
            Event::Routed(route) => {
                let signer_revoked = key_of(&route.by).is_some_and(|key| standing.revocations.contains_key(&key));
                let member_revoked = key_of(&route.member).is_some_and(|key| standing.revocations.contains_key(&key));
                let label = format!("the route {} for {} by {}", route.version, route.member, route.by);
                if signer_revoked || member_revoked || !standing.history.managers.contains(&route.by) {
                    refused.push((label, format!("{} did not hold management rights then", route.by)));
                    continue;
                }
                let next = standing.routes.get(&route.member).map(|latest| latest.version + 1).unwrap_or(1);
                if route.version != next {
                    refused.push((label, format!("the next route for {} was version {next}", route.member)));
                    continue;
                }
                standing.routes.insert(route.member.clone(), route.clone());
            }
        }
    }
    standing
}

impl Standing {
    /// Works out `name`'s scope now, and whether it is a branch, into
    /// `scopes`.
    fn scope(
        &self,
        name: &MachineName,
        accepted: &BTreeMap<MachineName, Joined>,
        scopes: &mut BTreeMap<MachineName, Result<(Scope, bool), String>>,
    ) -> Result<(Scope, bool), String> {
        if let Some(done) = scopes.get(name) {
            return done.clone();
        }
        let joined = &accepted[name];
        let result = if self.history.managers.contains(name) {
            Ok((Scope::all(), false))
        } else {
            let base = match self.latest.get(name) {
                Some(grant) => grant.scope.clone(),
                None => joined
                    .endorsement
                    .as_ref()
                    .and_then(|endorsement| endorsement.scope.clone())
                    .unwrap_or_else(Scope::all),
            };
            match &joined.invited_by {
                Some(inviter) if self.history.delegated(name) => match accepted.get(inviter) {
                    None => Err(format!("its inviter {inviter} is not trusted")),
                    Some(theirs) if self.revocations.contains_key(&theirs.record.key) => {
                        Err(format!("it is in the branch of {inviter}, which was revoked"))
                    }
                    Some(_) if self.history.delegated(inviter) => {
                        Err(format!("it was invited by {inviter}, itself in a branch, which may not invite"))
                    }
                    Some(_) => match self.scope(inviter, accepted, scopes) {
                        Ok((theirs, _)) if theirs.invite => {
                            let mut scope = base.intersect(&theirs);
                            scope.invite = false;
                            Ok((scope, true))
                        }
                        Ok(_) => Err(format!("it was invited by {inviter}, which may not invite")),
                        Err(_) => Err(format!("it is in the branch of {inviter}, which is not trusted")),
                    },
                },
                _ => Ok((base, false)),
            }
        };
        scopes.insert(name.clone(), result.clone());
        result
    }
}

fn others(names: &[MachineName], except: &MachineName) -> String {
    let others: BTreeSet<&str> = names.iter().filter(|name| *name != except).map(|name| name.as_str()).collect();
    others.into_iter().collect::<Vec<_>>().join(", ")
}
