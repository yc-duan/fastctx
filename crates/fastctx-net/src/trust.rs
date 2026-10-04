//! Who belongs to the World, decided on each machine for itself.
//!
//! The hub keeps the list of members and hands it out, but no machine takes
//! the hub's word for who is a member: the hub sees everything that passes
//! through it, and a hub that could add a member could speak as one. Every
//! machine therefore checks the list itself, from one root it learned
//! outside the hub.
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
//! an invitation's seed leaked.
//!
//! A member that is lost is revoked: any member signs a statement that a
//! key is no longer trusted. Revocations are kept forever, and they name
//! the key, not the name, so a revoked key is not trusted under any name.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

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
}

/// A member's statement that a key is no longer trusted, signed by that
/// member. A machine leaving the World revokes itself.
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
    /// Who invited it; `None` for the founder.
    pub invited_by: Option<MachineName>,
}

/// The members this machine trusts, as checked from the hub's list.
#[derive(Clone, Debug, Default)]
pub struct Table {
    trusted: BTreeMap<MachineName, Member>,
    /// Names that were members and are revoked, with who revoked them.
    revoked: BTreeMap<MachineName, Revocation>,
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

    pub fn revoked(&self, name: &MachineName) -> Option<&Revocation> {
        self.revoked.get(name)
    }

    /// The members whose keys are revoked, with their revocations.
    pub fn revoked_members(&self) -> impl Iterator<Item = (&MachineName, &Revocation)> {
        self.revoked.iter()
    }
}

/// Checks the hub's list of records and revocations against the founder's
/// key and what this machine has accepted before; returns the members to
/// trust, and what to remember from now on.
pub fn check(
    world: &WorldId,
    founder: &PublicKey,
    records: &[Signed],
    revocations: &[Signed],
    known: &Known,
) -> (Table, Known) {
    let mut table = Table::default();
    let mut known = known.clone();

    // Each record must be signed by the key it names, for this World.
    let mut candidates: BTreeMap<MachineName, Vec<Record>> = BTreeMap::new();
    for signed in records {
        let record = match signed.claim::<Record>() {
            Ok(record) => record,
            Err(error) => {
                table.refused.push(("a record".to_string(), error.to_string()));
                continue;
            }
        };
        let name = record.name.to_string();
        match signed.open::<Record>(&record.key, Domain::Record) {
            Ok(_) if record.world != *world => {
                table.refused.push((name, format!("it is for the World {}", record.world)))
            }
            Ok(record) => candidates.entry(record.name.clone()).or_default().push(record),
            Err(error) => table.refused.push((name, error.to_string())),
        }
    }

    // One record per name, and the key a name was first seen with.
    let mut records: BTreeMap<MachineName, Record> = BTreeMap::new();
    for (name, mut list) in candidates {
        list.dedup();
        let list: Vec<Record> = match known.names.get(&name) {
            Some(key) => {
                let (same, other): (Vec<Record>, Vec<Record>) = list.into_iter().partition(|record| record.key == *key);
                if !other.is_empty() {
                    table
                        .refused
                        .push((name.to_string(), "it has a different key than when it was first seen".to_string()));
                }
                same
            }
            None => list,
        };
        match list.len() {
            0 => {}
            1 => {
                records.insert(name, list.into_iter().next().expect("one record"));
            }
            _ => table.refused.push((name.to_string(), "the hub listed it with several keys".to_string())),
        }
    }

    // Keys this machine already knew to be revoked. A revoked key may still
    // sign endorsements; a machine first seen after the revocation was
    // accepted, invited by that key, is not admitted.
    let prior_revoked: BTreeSet<PublicKey> = known
        .revocations
        .iter()
        .filter_map(|signed| signed.claim::<Revocation>().ok())
        .map(|revocation| revocation.key)
        .collect();

    // Accept the founder, then whoever was invited by an accepted member,
    // until nobody more can be accepted.
    let mut accepted: BTreeMap<MachineName, (Member, Option<PublicKey>)> = BTreeMap::new();
    let mut waiting: Vec<Record> = Vec::new();
    for record in records.into_values() {
        match &record.credential {
            Credential::Founder if record.key == *founder => {
                let member = Member { name: record.name.clone(), key: record.key, invited_by: None };
                accepted.insert(record.name.clone(), (member, None));
            }
            Credential::Founder => table
                .refused
                .push((record.name.to_string(), "it claims to be the founder with another key".to_string())),
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
                Admitted::Yes { inviter, .. }
                    if !known.names.contains_key(&record.name)
                        && accepted.get(&inviter).is_some_and(|(member, _)| prior_revoked.contains(&member.key)) =>
                {
                    table.refused.push((
                        record.name.to_string(),
                        format!("it was invited by {inviter} after {inviter} was revoked"),
                    ));
                }
                Admitted::Yes { inviter, invite } => {
                    let member = Member { name: record.name.clone(), key: record.key, invited_by: Some(inviter) };
                    accepted.insert(record.name.clone(), (member, Some(invite)));
                    progressed = true;
                }
                Admitted::NotYet => still.push(record),
                Admitted::No(reason) => table.refused.push((record.name.to_string(), reason)),
            }
        }
        waiting = still;
        if !progressed {
            break;
        }
    }
    for record in waiting {
        table.refused.push((record.name.to_string(), "its inviter is not a member".to_string()));
    }

    // One invitation admits one machine; when it admitted more, its seed
    // leaked, and none of them is trusted.
    let mut by_invite: BTreeMap<PublicKey, Vec<MachineName>> = BTreeMap::new();
    for (name, (_, invite)) in &accepted {
        if let Some(invite) = invite {
            by_invite.entry(*invite).or_default().push(name.clone());
        }
    }
    for names in by_invite.values().filter(|names| names.len() > 1) {
        for name in names {
            accepted.remove(name);
            table.refused.push((name.to_string(), format!("its invitation also admitted {}", others(names, name))));
        }
    }

    for (name, (member, _)) in &accepted {
        known.names.entry(name.clone()).or_insert(member.key);
    }

    // Revocations already accepted stand; new ones are accepted in the
    // hub's order when a member whose key is not yet revoked signed them.
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
        let Some((signer, _)) = accepted.get(&revocation.by) else {
            table.refused.push((label, "its signer is not a member".to_string()));
            continue;
        };
        if revoked_keys.contains_key(&signer.key) {
            table.refused.push((label, "its signer's key was revoked before it".to_string()));
            continue;
        }
        match signed.open::<Revocation>(&signer.key, Domain::Revocation) {
            Ok(revocation) if revocation.world == *world => {
                revoked_keys.insert(revocation.key, revocation);
                known.revocations.push(signed.clone());
            }
            Ok(revocation) => table.refused.push((label, format!("it is for the World {}", revocation.world))),
            Err(error) => table.refused.push((label, error.to_string())),
        }
    }

    for (name, (member, _)) in accepted {
        match revoked_keys.get(&member.key) {
            Some(revocation) => {
                table.revoked.insert(name, revocation.clone());
            }
            None => {
                table.trusted.insert(name, member);
            }
        }
    }
    (table, known)
}

enum Admitted {
    Yes {
        inviter: MachineName,
        invite: PublicKey,
    },
    /// The inviter has not been accepted yet; it may be later in this pass.
    NotYet,
    No(String),
}

fn admitted(
    world: &WorldId,
    record: &Record,
    admission: &Signed,
    accepted: &BTreeMap<MachineName, (Member, Option<PublicKey>)>,
) -> Admitted {
    let claimed = match admission.claim::<Admission>() {
        Ok(admission) => admission,
        Err(error) => return Admitted::No(format!("its admission {error}")),
    };
    let endorsement = match claimed.endorsement.claim::<Endorsement>() {
        Ok(endorsement) => endorsement,
        Err(error) => return Admitted::No(format!("its endorsement {error}")),
    };
    let Some((inviter, _)) = accepted.get(&endorsement.inviter) else {
        return Admitted::NotYet;
    };
    let endorsement = match claimed.endorsement.open::<Endorsement>(&inviter.key, Domain::Endorsement) {
        Ok(endorsement) => endorsement,
        Err(error) => return Admitted::No(format!("its endorsement by {}: {error}", inviter.name)),
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
    Admitted::Yes { inviter: endorsement.inviter, invite: endorsement.invite }
}

fn others(names: &[MachineName], except: &MachineName) -> String {
    let others: BTreeSet<&str> = names.iter().filter(|name| *name != except).map(|name| name.as_str()).collect();
    others.into_iter().collect::<Vec<_>>().join(", ")
}
