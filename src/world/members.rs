//! Locally verified admission chains and permanent signed revocations.

use super::admission::Credential;
use super::crypto::{Key32, b64_array, b64_decode, b64_encode};
use super::identity::{self, Identity};
use super::messages::{
    MemberEntry, MemberRecord, RevocationStatement, SignedRecord, SignedRevocation,
};
use super::{WorldPaths, read_optional, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MEMBERS_FILE_VERSION: u32 = 2;
/// Signature domain of a member record.
pub(crate) const MEMBER_RECORD_DOMAIN: &str = "member_record";
/// Signature domain of a revocation.
pub(crate) const REVOCATION_DOMAIN: &str = "revocation";
/// Presence word for a member this member knows but the hub no longer lists.
pub(crate) const STATE_UNLISTED: &str = "unlisted";
/// Presence word the hub reports for a revoked member.
pub(crate) const STATE_REVOKED: &str = "revoked";

/// A device admitted through the locally pinned founding key.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct VerifiedMember {
    pub(crate) record: MemberRecord,
    pub(crate) signed: SignedRecord,
    /// `online`, `offline`, `revoked` (reported by the hub without a signed statement), or
    /// `unlisted`.
    pub(crate) state: String,
    pub(crate) last_seen: String,
    #[serde(default)]
    pub(crate) hub_rtt_ms: Option<u32>,
    #[serde(default)]
    pub(crate) tls: Option<String>,
    #[serde(default)]
    pub(crate) network: Option<String>,
    #[serde(default)]
    pub(crate) inventory_version: u64,
    /// Why the hub's latest record for this member did not verify; `record` is then the last
    /// one that did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stale: Option<String>,
}

impl VerifiedMember {
    pub(crate) fn public_key(&self) -> Result<Key32, String> {
        b64_array::<32>(&self.record.node_pub, "member public key")
    }

    pub(crate) fn is_online(&self) -> bool {
        self.state == "online"
    }

    /// Whether this member still takes part: not revoked by the hub's account either.
    pub(crate) fn is_current(&self) -> bool {
        self.state != STATE_REVOKED
    }
}

/// The cached member table, keyed by name.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct MemberTable {
    pub(crate) version: u64,
    #[serde(default)]
    pub(crate) records: BTreeMap<String, SignedRecord>,
    #[serde(default)]
    pub(crate) blocked: std::collections::BTreeSet<String>,
    /// Every identity this member has verified, including ones the hub reports revoked or no
    /// longer lists; `get` filters to current members.
    pub(crate) members: BTreeMap<String, VerifiedMember>,
    /// Members the hub listed whose record did not verify and whom this member never knew,
    /// with the reason; kept so a `nodes` listing can say why a name is missing.
    #[serde(default)]
    pub(crate) unverified: BTreeMap<String, String>,
    /// Signed revocations by name; also index the revoked keys, which stay refused forever.
    #[serde(default)]
    pub(crate) revoked: BTreeMap<String, SignedRevocation>,
    #[serde(default)]
    pub(crate) invitations: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) conflicts: std::collections::BTreeSet<String>,
}

#[derive(Deserialize, Serialize)]
struct MembersFile {
    v: u32,
    table: MemberTable,
}

impl MemberTable {
    pub(crate) fn load(paths: &WorldPaths) -> Result<Option<Self>, String> {
        let Some(bytes) = read_optional(&paths.members)? else {
            return Ok(None);
        };
        let file: MembersFile = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Cannot parse {}: {error}",
                crate::paths::display_path(&paths.members)
            )
        })?;
        if file.v != MEMBERS_FILE_VERSION {
            return Err(format!(
                "{} was written by a newer fastctx (format {}); this build reads format {} at most.",
                crate::paths::display_path(&paths.members),
                file.v,
                MEMBERS_FILE_VERSION
            ));
        }
        Ok(Some(file.table))
    }

    pub(crate) fn save(&self, paths: &WorldPaths) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(&MembersFile {
            v: MEMBERS_FILE_VERSION,
            table: self.clone(),
        })
        .map_err(|error| format!("Cannot encode the member table: {error}"))?;
        write_atomic(&paths.members, &json)
    }

    /// Rebuilds the table from a hub listing on top of `previous`: records are verified under
    /// the keys held, revocations under the signatures of members already trusted, and
    /// identities that verified before are kept when the fresh record does not verify.
    pub(crate) fn from_entries(
        version: u64,
        entries: &[MemberEntry],
        founder: &str,
        world: &str,
        previous: &Self,
    ) -> Self {
        let mut table = Self {
            version,
            records: previous.records.clone(),
            blocked: previous.blocked.clone(),
            members: BTreeMap::new(),
            unverified: BTreeMap::new(),
            revoked: previous.revoked.clone(),
            invitations: previous.invitations.clone(),
            conflicts: previous.conflicts.clone(),
        };

        let mut proofs = previous.records.clone();
        let mut replacements = std::collections::BTreeSet::new();
        for entry in entries {
            if let Some(signed) = &entry.signed {
                if let Some(known) = proofs.get(&entry.name)
                    && (verify_record(known).ok().map(|r| r.node_pub)
                        != verify_record(signed).ok().map(|r| r.node_pub)
                        || known.admission != signed.admission)
                {
                    replacements.insert(entry.name.clone());
                    continue;
                }
                proofs.insert(entry.name.clone(), signed.clone());
            }
        }
        // Resolve the whole graph before accepting anything; a duplicate invitation poisons
        // both claims, including a previously accepted claim, without forgetting its identity.
        for signed in proofs.values() {
            if let Ok(record) = verify_record(signed)
                && let Credential::Invited { endorsement, .. } = &signed.admission
                && let Some(inviter) = proofs.get(&endorsement.inviter)
                && let Ok(inviter) = verify_record(inviter)
                && signed
                    .admission
                    .verify(
                        &record.name,
                        &record.node_pub,
                        &b64_array::<32>(&inviter.node_pub, "inviter").unwrap_or_default(),
                    )
                    .is_ok()
            {
                if let Some(key) = table.invitations.get(&endorsement.invite_pub) {
                    if key != &record.node_pub {
                        table.conflicts.insert(endorsement.invite_pub.clone());
                    }
                } else {
                    table
                        .invitations
                        .insert(endorsement.invite_pub.clone(), record.node_pub);
                }
            }
        }
        let mut fresh: BTreeMap<&str, Result<MemberRecord, String>> = BTreeMap::new();
        for entry in entries {
            let outcome = if replacements.contains(&entry.name) {
                Err("member identity/admission cannot be replaced".into())
            } else {
                verify_chain(
                    &entry.name,
                    &proofs,
                    founder,
                    world,
                    &table.conflicts,
                    &mut Vec::new(),
                )
            };
            if outcome.is_ok() {
                table
                    .records
                    .insert(entry.name.clone(), proofs[&entry.name].clone());
            }
            if outcome
                .as_ref()
                .is_err_and(|reason| reason.contains("one invitation"))
            {
                table.blocked.insert(entry.name.clone());
            }
            fresh.insert(entry.name.as_str(), outcome);
        }

        // Pass two: revocations. A revoker's key comes from a fresh record that verified or
        // from the previous table, never from the hub's word.
        let trusted_key = |name: &str| -> Option<Key32> {
            if table.revoked.contains_key(name) {
                return None;
            }
            match fresh.get(name) {
                Some(Ok(record)) => b64_array::<32>(&record.node_pub, "member public key").ok(),
                _ => previous
                    .members
                    .get(name)
                    .and_then(|member| member.public_key().ok()),
            }
        };
        let mut newly_revoked = Vec::new();
        for entry in entries {
            let Some(signed) = &entry.revocation else {
                continue;
            };
            if table.revoked.contains_key(&entry.name) {
                continue;
            }
            match verify_revocation(signed, trusted_key) {
                Ok(statement) if statement.name == entry.name => {
                    newly_revoked.push((entry.name.clone(), signed.clone()));
                }
                Ok(statement) => {
                    table.unverified.insert(
                        entry.name.clone(),
                        format!(
                            "the hub attached a revocation of \"{}\" to \"{}\"",
                            statement.name, entry.name
                        ),
                    );
                }
                Err(error) => {
                    table
                        .unverified
                        .insert(entry.name.clone(), format!("revocation rejected: {error}"));
                }
            }
        }
        for (name, signed) in newly_revoked {
            table.revoked.insert(name, signed);
        }

        // Pass three: the members themselves.
        for entry in entries {
            if table.revoked.contains_key(&entry.name) {
                continue;
            }
            let fresh_record = fresh
                .remove(entry.name.as_str())
                .unwrap_or_else(|| Err("the hub listed this member without a record".to_string()));
            let fresh_record = match fresh_record {
                Ok(record) if table.is_revoked_key(&record.node_pub) => {
                    table.unverified.insert(
                        entry.name.clone(),
                        "the record's key was revoked under another name".to_string(),
                    );
                    continue;
                }
                other => other,
            };
            let (record, stale) = match fresh_record {
                Ok(record) => (record, None),
                Err(reason) => match previous.members.get(&entry.name) {
                    Some(known) if !table.is_revoked_key(&known.record.node_pub) => {
                        (known.record.clone(), Some(reason))
                    }
                    _ => {
                        table.unverified.insert(entry.name.clone(), reason);
                        continue;
                    }
                },
            };
            table.members.insert(
                entry.name.clone(),
                VerifiedMember {
                    signed: if stale.is_none() {
                        entry.signed.clone().expect("verified record")
                    } else {
                        previous.members[&entry.name].signed.clone()
                    },
                    record,
                    state: entry.state.clone(),
                    last_seen: entry.last_seen.clone(),
                    hub_rtt_ms: entry.hub_rtt_ms,
                    tls: entry.tls.clone(),
                    network: entry.network.clone(),
                    inventory_version: entry.inventory_version,
                    stale,
                },
            );
        }

        // Members known before that the hub no longer lists stay known, unlisted.
        for (name, known) in &previous.members {
            if table.members.contains_key(name)
                || table.revoked.contains_key(name)
                || table.is_revoked_key(&known.record.node_pub)
            {
                continue;
            }
            let mut kept = known.clone();
            kept.state = STATE_UNLISTED.to_string();
            kept.stale = Some("the hub no longer lists this member".to_string());
            table.members.insert(name.clone(), kept);
        }
        table
    }

    /// Whether a signed revocation names this key.
    pub(crate) fn is_revoked_key(&self, node_pub: &str) -> bool {
        self.revoked
            .values()
            .filter_map(|signed| signed.parse().ok())
            .any(|statement| statement.node_pub == node_pub)
    }

    /// A current member: verified, not revoked by anyone's account.
    pub(crate) fn get(&self, name: &str) -> Option<&VerifiedMember> {
        self.members.get(name).filter(|member| {
            member.is_current()
                && !self.blocked.contains(name)
                && !self.revoked.contains_key(name)
                && !self.is_revoked_key(&member.record.node_pub)
                && match &member.signed.admission {
                    Credential::Founder => true,
                    Credential::Invited { endorsement, .. } => {
                        !self.conflicts.contains(&endorsement.invite_pub)
                    }
                }
        })
    }

    /// Any verified identity, including one the hub reports revoked without a signed
    /// statement; for countersigning such a revocation, never for trusting a message.
    pub(crate) fn identity(&self, name: &str) -> Option<&VerifiedMember> {
        self.members.get(name)
    }

    /// The key of a current member, for verifying what it signed.
    pub(crate) fn trusted_key(&self, name: &str) -> Option<Key32> {
        self.get(name).and_then(|member| member.public_key().ok())
    }

    /// Whether `name` is a current member a call may be addressed to. An offline member
    /// resolves: "offline" is an answer the caller wants to see, and the hub gives it. The
    /// errors are the sentences the tool surface shows the model.
    pub(crate) fn resolve(&self, name: &str) -> Result<(), String> {
        if self.get(name).is_some() {
            return Ok(());
        }
        if self.revoked.contains_key(name)
            || self
                .members
                .get(name)
                .is_some_and(|member| !member.is_current())
        {
            return Err(format!(
                "\"{name}\" was removed from this World. List machines with nodes."
            ));
        }
        if let Some(reason) = self.unverified.get(name) {
            return Err(format!(
                "\"{name}\" is listed by the hub but its record does not verify ({reason})."
            ));
        }
        Err(format!(
            "No machine named \"{name}\". List machines with nodes."
        ))
    }
}

/// Builds and signs this member's own record.
pub(crate) fn publish_record(
    identity: &Identity,
    admission: &Credential,
    record: &MemberRecord,
) -> Result<SignedRecord, String> {
    let bytes = serde_json::to_vec(record)
        .map_err(|error| format!("Cannot encode the member record: {error}"))?;
    let sig = identity.sign(MEMBER_RECORD_DOMAIN, &bytes);
    Ok(SignedRecord {
        record: String::from_utf8(bytes).expect("serde_json produces UTF-8"),
        admission: admission.clone(),
        sig: b64_encode(&sig),
    })
}

/// Verifies a self-signed record; admission-chain verification is separate.
pub(crate) fn verify_record(signed: &SignedRecord) -> Result<MemberRecord, String> {
    let record: MemberRecord = serde_json::from_str(&signed.record)
        .map_err(|error| format!("the record is not valid JSON: {error}"))?;
    let public_key = b64_array::<32>(&record.node_pub, "member public key")
        .map_err(|error| format!("the record's public key is invalid: {error}"))?;
    let signature =
        b64_decode(&signed.sig).map_err(|error| format!("invalid record signature: {error}"))?;
    identity::verify(
        &public_key,
        MEMBER_RECORD_DOMAIN,
        signed.record.as_bytes(),
        &signature,
    )
    .map_err(|_| "the record's signature does not verify against its own public key".to_string())?;
    super::validate_node_name(&record.name)?;
    Ok(record)
}

/// Signs a revocation as the member `by`.
pub(crate) fn sign_revocation(
    identity: &Identity,
    statement: &RevocationStatement,
) -> Result<SignedRevocation, String> {
    let bytes = serde_json::to_vec(statement)
        .map_err(|error| format!("Cannot encode the revocation: {error}"))?;
    Ok(SignedRevocation {
        sig: b64_encode(&identity.sign(REVOCATION_DOMAIN, &bytes)),
        statement: String::from_utf8(bytes).expect("serde_json produces UTF-8"),
    })
}

/// Verifies a revocation against the key of the member it names as revoker. `revoker_key`
/// resolves a name to a key the verifier already trusts.
pub(crate) fn verify_revocation(
    signed: &SignedRevocation,
    revoker_key: impl Fn(&str) -> Option<Key32>,
) -> Result<RevocationStatement, String> {
    let statement = signed.parse()?;
    let key = revoker_key(&statement.by).ok_or_else(|| {
        format!(
            "the revocation is signed by \"{}\", which is not a verified member",
            statement.by
        )
    })?;
    let signature = b64_decode(&signed.sig)
        .map_err(|error| format!("invalid revocation signature: {error}"))?;
    identity::verify(
        &key,
        REVOCATION_DOMAIN,
        signed.statement.as_bytes(),
        &signature,
    )
    .map_err(|_| "the revocation's signature does not verify against its signer".to_string())?;
    Ok(statement)
}

fn verify_chain(
    name: &str,
    proofs: &BTreeMap<String, SignedRecord>,
    founder: &str,
    world: &str,
    conflicts: &std::collections::BTreeSet<String>,
    visiting: &mut Vec<String>,
) -> Result<MemberRecord, String> {
    if visiting.iter().any(|seen| seen == name) {
        return Err("admission cycle".into());
    }
    let signed = proofs
        .get(name)
        .ok_or_else(|| format!("missing admission record for {name}"))?;
    let record = verify_record(signed)?;
    if record.name != name {
        return Err("record name differs from listing".into());
    }
    super::validate_node_name(name)?;
    visiting.push(name.into());
    match &signed.admission {
        Credential::Founder if record.node_pub == founder => {}
        Credential::Founder => return Err("founder key does not match pinned founder".into()),
        Credential::Invited { endorsement, .. } => {
            if endorsement.world != world {
                return Err("invitation belongs to another world".into());
            }
            if conflicts.contains(&endorsement.invite_pub) {
                return Err("one invitation endorsed different member keys".into());
            }
            let inviter = verify_chain(
                &endorsement.inviter,
                proofs,
                founder,
                world,
                conflicts,
                visiting,
            )?;
            signed.admission.verify(
                name,
                &record.node_pub,
                &b64_array(&inviter.node_pub, "inviter")?,
            )?;
        }
    }
    visiting.pop();
    Ok(record)
}
