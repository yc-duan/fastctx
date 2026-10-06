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

use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ledger::TownId;
use crate::scope::{Grant, Scope};
use crate::seal::SealPublic;
use crate::sign::{Digest, Domain, PublicKey, Signed, sha256};

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
    /// Left out, as before scopes, it is not.
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

/// The members this machine trusts, as checked from the hub's list.
#[derive(Clone, Debug, Default)]
pub struct Table {
    trusted: BTreeMap<MachineName, Member>,
    /// Names that were members and are revoked, with who revoked them.
    revoked: BTreeMap<MachineName, Revocation>,
    /// The latest grant taken for each member.
    grants: BTreeMap<MachineName, Grant>,
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

    /// The latest grant taken for `name`.
    pub fn grant(&self, name: &MachineName) -> Option<&Grant> {
        self.grants.get(name)
    }

    /// Who may read `town`: the members with management rights, and those
    /// whose scope includes it. The World's own ledger is everyone's.
    pub fn readers(&self, town: &TownId) -> impl Iterator<Item = &Member> {
        let world = *town == TownId::world();
        self.trusted.values().filter(move |member| world || member.manage || member.scope.projects.contains(town))
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

    // Revocations already accepted stand. New ones are taken in the hub's
    // order when the member that signed them, whose key is not revoked
    // before, may revoke that member as things stand with the revocations
    // taken so far.
    let mut revoked_keys: BTreeMap<PublicKey, Revocation> = BTreeMap::new();
    for signed in &known.revocations {
        if let Ok(revocation) = signed.claim::<Revocation>() {
            revoked_keys.insert(revocation.key, revocation);
        }
    }
    for signed in revocations {
        if known.revocations.contains(signed) {
            continue;
        }
        let revocation = match signed.claim::<Revocation>() {
            Ok(revocation) => revocation,
            Err(error) => {
                table.refused.push(("a revocation".to_string(), error.to_string()));
                continue;
            }
        };
        let label = format!("the revocation of {} by {}", revocation.name, revocation.by);
        let Some(signer) = accepted.get(&revocation.by) else {
            table.refused.push((label, "its signer is not a member".to_string()));
            continue;
        };
        if revoked_keys.contains_key(&signer.record.key) {
            table.refused.push((label, "its signer's key was revoked before it".to_string()));
            continue;
        }
        let revocation = match signed.open::<Revocation>(&signer.record.key, Domain::Revocation) {
            Ok(revocation) if revocation.world == *world => revocation,
            Ok(revocation) => {
                table.refused.push((label, format!("it is for the World {}", revocation.world)));
                continue;
            }
            Err(error) => {
                table.refused.push((label, error.to_string()));
                continue;
            }
        };
        let standing = evaluate(&accepted, &grants, &revoked_keys);
        if !standing.may_revoke(&accepted, &revocation) {
            table.refused.push((label, format!("{} may not revoke {}", revocation.by, revocation.name)));
            continue;
        }
        revoked_keys.insert(revocation.key, revocation);
        known.revocations.push(signed.clone());
    }

    let standing = evaluate(&accepted, &grants, &revoked_keys);
    let mut scopes: BTreeMap<MachineName, Result<(Scope, bool), String>> = BTreeMap::new();
    for name in accepted.keys() {
        if !revoked_keys.contains_key(&accepted[name].record.key) {
            // Kept in `scopes`, with the branch it hangs on worked out first.
            let _ = standing.scope(name, &accepted, &revoked_keys, &mut scopes);
        }
    }
    for (name, joined) in accepted {
        if let Some(revocation) = revoked_keys.get(&joined.record.key) {
            table.revoked.insert(name, revocation.clone());
            continue;
        }
        match scopes.remove(&name) {
            Some(Ok((scope, delegated))) => {
                let member = Member {
                    name: name.clone(),
                    key: joined.record.key,
                    seal: joined.record.seal,
                    invited_by: joined.invited_by,
                    kind: joined.kind,
                    manage: standing.managers.contains(&name),
                    scope,
                    delegated,
                };
                table.trusted.insert(name, member);
            }
            Some(Err(reason)) => table.refused.push((name.to_string(), reason)),
            None => {}
        }
    }
    table.grants = standing.latest.into_iter().filter(|(name, _)| table.trusted.contains_key(name)).collect();
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

/// Who holds management rights and what each member was granted, from
/// the grants and revocations taken in the order they were made.
struct Standing {
    managers: BTreeSet<MachineName>,
    latest: BTreeMap<MachineName, Grant>,
    /// When each member held management rights.
    held: BTreeMap<MachineName, Vec<(Timestamp, Option<Timestamp>)>>,
}

enum Event<'a> {
    Revoked(&'a Revocation),
    Granted(&'a Grant),
}

fn evaluate(
    accepted: &BTreeMap<MachineName, Joined>,
    grants: &[(Grant, [u8; 32])],
    revoked: &BTreeMap<PublicKey, Revocation>,
) -> Standing {
    let founder = accepted.values().find(|joined| joined.invited_by.is_none()).map(|joined| joined.record.name.clone());
    let mut standing = Standing { managers: BTreeSet::new(), latest: BTreeMap::new(), held: BTreeMap::new() };
    if let Some(founder) = founder {
        standing.managers.insert(founder.clone());
        standing.held.insert(founder, vec![(Timestamp::MIN, None)]);
    }
    // In the order they were made: a revocation before a grant made at the
    // same instant, and grants made at one instant by the digest of their
    // signed bytes, so every machine reaches the same answer.
    let mut events: Vec<(Timestamp, u8, [u8; 32], Event)> = Vec::new();
    for revocation in revoked.values() {
        events.push((revocation.at, 0, [0; 32], Event::Revoked(revocation)));
    }
    for (grant, digest) in grants {
        events.push((grant.at, 1, *digest, Event::Granted(grant)));
    }
    events.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    let mut gone: BTreeSet<MachineName> = BTreeSet::new();
    for (at, _, _, event) in events {
        match event {
            Event::Revoked(revocation) => {
                let Some(joined) = accepted.get(&revocation.name).filter(|joined| joined.record.key == revocation.key)
                else {
                    continue;
                };
                gone.insert(joined.record.name.clone());
                standing.demote(&joined.record.name, at);
            }
            Event::Granted(grant) => {
                if !standing.managers.contains(&grant.by) || gone.contains(&grant.member) {
                    continue;
                }
                if standing.latest.get(&grant.member).is_some_and(|latest| grant.version <= latest.version) {
                    continue;
                }
                let machine = accepted.get(&grant.member).is_some_and(|joined| joined.kind.is_machine());
                if grant.manage && machine {
                    standing.promote(&grant.member, at);
                } else {
                    standing.demote(&grant.member, at);
                }
                standing.latest.insert(grant.member.clone(), grant.clone());
            }
        }
    }
    standing
}

impl Standing {
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

    /// Whether `joined` is the branch of its inviter: it was invited, after
    /// scopes, by a member that did not hold management rights then.
    fn delegated(&self, joined: &Joined) -> bool {
        match (&joined.invited_by, joined.endorsement.as_ref().and_then(|endorsement| endorsement.at)) {
            (Some(inviter), Some(at)) => !self.held_at(inviter, at),
            _ => false,
        }
    }

    fn may_revoke(&self, accepted: &BTreeMap<MachineName, Joined>, revocation: &Revocation) -> bool {
        let Some(signer) = accepted.get(&revocation.by) else { return false };
        if self.managers.contains(&revocation.by) {
            return true;
        }
        let Some(target) = accepted.get(&revocation.name).filter(|joined| joined.record.key == revocation.key) else {
            return false;
        };
        if revocation.by == revocation.name {
            return true;
        }
        signer.kind.is_machine() && target.invited_by.as_ref() == Some(&revocation.by) && self.delegated(target)
    }

    /// Works out `name`'s scope, and whether it is a branch, into `scopes`.
    fn scope(
        &self,
        name: &MachineName,
        accepted: &BTreeMap<MachineName, Joined>,
        revoked: &BTreeMap<PublicKey, Revocation>,
        scopes: &mut BTreeMap<MachineName, Result<(Scope, bool), String>>,
    ) -> Result<(Scope, bool), String> {
        if let Some(done) = scopes.get(name) {
            return done.clone();
        }
        let joined = &accepted[name];
        let result = if self.managers.contains(name) {
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
                Some(inviter) if self.delegated(joined) => {
                    let inviter_joined = &accepted[inviter];
                    if revoked.contains_key(&inviter_joined.record.key) {
                        Err(format!("it is in the branch of {inviter}, which was revoked"))
                    } else {
                        match self.scope(inviter, accepted, revoked, scopes) {
                            Ok((theirs, _)) if theirs.invite => {
                                let mut scope = base.intersect(&theirs);
                                scope.invite = false;
                                Ok((scope, true))
                            }
                            Ok(_) => Err(format!("it was invited by {inviter}, which may not invite")),
                            Err(_) => Err(format!("it is in the branch of {inviter}, which is not trusted")),
                        }
                    }
                }
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
