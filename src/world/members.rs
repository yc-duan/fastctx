//! The member's view of who is in the World: records verified by MAC, revocations verified
//! by signature, the selectors that name members in tool calls, and the local cache file.
//!
//! Identity is monotonic on the member side. A name whose record once verified is never
//! forgotten because a later listing fails to verify it (a hub cannot un-know a member for
//! us), and a key that a member revoked in a signed statement is never trusted again, whatever
//! the hub lists afterwards. The hub only ever adds facts here: presence, and records or
//! revocations that carry their own proof.

use super::crypto::{self, Key32, b64_array, b64_decode, b64_encode};
use super::identity::{self, Identity};
use super::keys::KeyRing;
use super::messages::{
    MemberEntry, MemberRecord, RevocationStatement, SignedRecord, SignedRevocation,
};
use super::{WorldPaths, read_optional, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MEMBERS_FILE_VERSION: u32 = 1;
/// Signature domain of a member record.
pub(crate) const MEMBER_RECORD_DOMAIN: &str = "member_record";
/// Signature domain of a revocation.
pub(crate) const REVOCATION_DOMAIN: &str = "revocation";
/// Presence word for a member this member knows but the hub no longer lists.
pub(crate) const STATE_UNLISTED: &str = "unlisted";
/// Presence word the hub reports for a revoked member.
pub(crate) const STATE_REVOKED: &str = "revoked";

/// A member whose record verified under the World key: the only kind other members act on.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct VerifiedMember {
    pub(crate) record: MemberRecord,
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

    pub(crate) fn wrap_public(&self) -> Result<Key32, String> {
        b64_array::<32>(&self.record.wrap_pub, "member wrap key")
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
        if file.v > MEMBERS_FILE_VERSION {
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
        keys: &KeyRing,
        previous: &Self,
    ) -> Self {
        let mut table = Self {
            version,
            members: BTreeMap::new(),
            unverified: BTreeMap::new(),
            revoked: previous.revoked.clone(),
        };

        // Pass one: which fresh records verify on their own.
        let mut fresh: BTreeMap<&str, Result<MemberRecord, String>> = BTreeMap::new();
        for entry in entries {
            let outcome = match &entry.signed {
                None => Err("the hub holds no published record for this member".to_string()),
                Some(signed) => match verify_record(signed, keys) {
                    Ok(record) if record.name == entry.name => Ok(record),
                    Ok(record) => Err(format!(
                        "the record is signed for \"{}\" but the hub lists it as \"{}\"",
                        record.name, entry.name
                    )),
                    Err(error) => Err(error),
                },
            };
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
        self.members.get(name).filter(|member| member.is_current())
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

    /// Current members the hub lists, online or not.
    pub(crate) fn listed(&self) -> impl Iterator<Item = &VerifiedMember> {
        self.members
            .values()
            .filter(|member| member.is_current() && member.state != STATE_UNLISTED)
    }

    /// Expands a selector (`design-objects.md` §2.1) into member names.
    ///
    /// `all` and tags expand to every listed member, offline ones included: a fan-out answers
    /// for each machine, and "offline" is an answer the caller wants to see rather than a
    /// machine silently left out of the count. Explicit names are kept as given so a call to
    /// an offline member is answered with its real state.
    pub(crate) fn expand(&self, selector: &Selector) -> Result<Vec<String>, String> {
        let mut names = Vec::new();
        let mut push = |name: String| {
            if !names.contains(&name) {
                names.push(name);
            }
        };
        for item in &selector.items {
            match item.as_str() {
                "all" => {
                    for member in self.listed() {
                        push(member.record.name.clone());
                    }
                }
                tagged if tagged.starts_with("tag:") => {
                    let tag = &tagged[4..];
                    if tag.is_empty() {
                        return Err("A tag selector needs a tag after \"tag:\".".to_string());
                    }
                    for member in self
                        .listed()
                        .filter(|member| member.record.tags.iter().any(|entry| entry == tag))
                    {
                        push(member.record.name.clone());
                    }
                }
                name => {
                    if self.get(name).is_some() {
                        push(name.to_string());
                    } else if self.revoked.contains_key(name)
                        || self
                            .members
                            .get(name)
                            .is_some_and(|member| !member.is_current())
                    {
                        return Err(format!(
                            "revoked: \"{name}\" was removed from this World. List machines with nodes."
                        ));
                    } else if let Some(reason) = self.unverified.get(name) {
                        return Err(format!(
                            "node_unknown: \"{name}\" is listed by the hub but its record does not verify ({reason})."
                        ));
                    } else {
                        return Err(format!(
                            "node_unknown: no member named \"{name}\". List machines with nodes."
                        ));
                    }
                }
            }
        }
        Ok(names)
    }
}

/// A parsed `node` argument: one or more names, `all`, or `tag:<t>` items.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Selector {
    pub(crate) items: Vec<String>,
}

impl Selector {
    pub(crate) fn parse_items(items: &[String]) -> Result<Self, String> {
        if items.is_empty() {
            return Err("The node selector is empty.".to_string());
        }
        for item in items {
            let item = item.trim();
            if item == "all" || item.starts_with("tag:") {
                continue;
            }
            super::validate_node_name(item)?;
        }
        Ok(Self {
            items: items.iter().map(|item| item.trim().to_string()).collect(),
        })
    }

    /// Whether the selector names exactly one explicit member.
    pub(crate) fn single_name(&self) -> Option<&str> {
        match self.items.as_slice() {
            [item] if item != "all" && !item.starts_with("tag:") => Some(item),
            _ => None,
        }
    }

    pub(crate) fn describe(&self) -> String {
        self.items.join(", ")
    }
}

/// Builds and signs this member's own record.
pub(crate) fn publish_record(
    identity: &Identity,
    keys: &KeyRing,
    record: &MemberRecord,
) -> Result<SignedRecord, String> {
    let bytes = serde_json::to_vec(record)
        .map_err(|error| format!("Cannot encode the member record: {error}"))?;
    let current = keys.current();
    let mac = crypto::hmac_sha256(&current.subkeys().mac, &bytes);
    let sig = identity.sign(MEMBER_RECORD_DOMAIN, &bytes);
    Ok(SignedRecord {
        record: String::from_utf8(bytes).expect("serde_json produces UTF-8"),
        mac: b64_encode(&mac),
        mac_epoch: current.epoch(),
        sig: b64_encode(&sig),
    })
}

/// Verifies a record's MAC (under the epoch it names) and the author's signature, returning
/// the parsed record. The hub cannot produce either, so a record that passes was published
/// by a member holding the World key and the private key it claims.
pub(crate) fn verify_record(signed: &SignedRecord, keys: &KeyRing) -> Result<MemberRecord, String> {
    let subkeys = keys.subkeys(signed.mac_epoch).ok_or_else(|| {
        format!(
            "the record is MAC'd under World key epoch {}, which this member does not hold",
            signed.mac_epoch
        )
    })?;
    let mac = b64_decode(&signed.mac).map_err(|error| format!("invalid record MAC: {error}"))?;
    if !crypto::hmac_verify(&subkeys.mac, signed.record.as_bytes(), &mac) {
        return Err("the record's MAC does not verify under the World key".to_string());
    }
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

#[cfg(test)]
mod tests {
    use super::{MemberTable, Selector, publish_record, sign_revocation, verify_record};
    use crate::world::identity::Identity;
    use crate::world::keys::KeyRing;
    use crate::world::messages::{MemberEntry, MemberRecord, RevocationStatement};

    fn record(identity: &Identity, name: &str, tags: &[&str]) -> MemberRecord {
        MemberRecord {
            name: name.to_string(),
            node_pub: crate::world::crypto::b64_encode(&identity.public_key()),
            wrap_pub: crate::world::crypto::b64_encode(&identity.wrap_public()),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            kind: "stateful".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            version: "1.0.0".to_string(),
            enrolled_at: "2026-09-04T00:00:00Z".to_string(),
        }
    }

    fn entry(
        identity: &Identity,
        keys: &KeyRing,
        name: &str,
        tags: &[&str],
        state: &str,
    ) -> MemberEntry {
        MemberEntry {
            name: name.to_string(),
            signed: Some(publish_record(identity, keys, &record(identity, name, tags)).unwrap()),
            state: state.to_string(),
            last_seen: "2026-09-04T00:00:00Z".to_string(),
            hub_rtt_ms: None,
            tls: None,
            network: None,
            version: None,
            inventory_version: 0,
            revocation: None,
        }
    }

    #[test]
    fn records_verify_only_under_the_world_key_and_their_own_signature() {
        let keys = KeyRing::new_initial().unwrap();
        let identity = Identity::generate().unwrap();
        let signed = publish_record(&identity, &keys, &record(&identity, "desktop", &[])).unwrap();
        assert_eq!(verify_record(&signed, &keys).unwrap().name, "desktop");
        assert!(verify_record(&signed, &KeyRing::new_initial().unwrap()).is_err());
        let mut renamed = signed.clone();
        renamed.record = renamed.record.replace("desktop", "evil");
        assert!(verify_record(&renamed, &keys).is_err());
    }

    #[test]
    fn selectors_expand_names_tags_and_all_over_listed_members() {
        let keys = KeyRing::new_initial().unwrap();
        let entries = [
            ("desktop", &["office"][..], "online"),
            ("laptop", &["office", "mobile"][..], "offline"),
            ("vps", &[][..], "online"),
        ]
        .iter()
        .map(|(name, tags, state)| entry(&Identity::generate().unwrap(), &keys, name, tags, state))
        .collect::<Vec<_>>();
        let table = MemberTable::from_entries(3, &entries, &keys, &MemberTable::default());
        assert_eq!(table.members.len(), 3);
        let all = table
            .expand(&Selector::parse_items(&["all".to_string()]).unwrap())
            .unwrap();
        assert_eq!(all, vec!["desktop", "laptop", "vps"]);
        let office = table
            .expand(&Selector::parse_items(&["tag:office".to_string()]).unwrap())
            .unwrap();
        assert_eq!(office, vec!["desktop", "laptop"]);
        let explicit = table
            .expand(
                &Selector::parse_items(&[
                    "laptop".to_string(),
                    "desktop".to_string(),
                    "laptop".to_string(),
                ])
                .unwrap(),
            )
            .unwrap();
        assert_eq!(explicit, vec!["laptop", "desktop"]);
        assert!(
            table
                .expand(&Selector::parse_items(&["ghost".to_string()]).unwrap())
                .unwrap_err()
                .contains("node_unknown")
        );
        assert_eq!(
            Selector::parse_items(&["laptop".to_string()])
                .unwrap()
                .single_name(),
            Some("laptop")
        );
        assert!(Selector::parse_items(&["Bad Name".to_string()]).is_err());
    }

    #[test]
    fn a_hub_cannot_unknow_a_member_and_a_signed_revocation_outlives_any_listing() {
        let keys = KeyRing::new_initial().unwrap();
        let desktop = Identity::generate().unwrap();
        let laptop = Identity::generate().unwrap();
        let first = vec![
            entry(&desktop, &keys, "desktop", &[], "online"),
            entry(&laptop, &keys, "laptop", &[], "online"),
        ];
        let table = MemberTable::from_entries(1, &first, &keys, &MemberTable::default());
        assert_eq!(table.members.len(), 2);

        // A later listing whose record for laptop is MAC'd under an epoch this member lacks
        // keeps laptop's identity, marked stale; a name never seen stays unverified.
        let mut later = KeyRing::new_initial().unwrap();
        later.rotate().unwrap();
        let mut second = vec![
            entry(&desktop, &keys, "desktop", &[], "online"),
            entry(&laptop, &later, "laptop", &[], "offline"),
            entry(
                &Identity::generate().unwrap(),
                &later,
                "ghost",
                &[],
                "online",
            ),
        ];
        let table = MemberTable::from_entries(2, &second, &keys, &table);
        assert!(table.get("laptop").unwrap().stale.is_some());
        assert_eq!(table.get("laptop").unwrap().state, "offline");
        assert!(table.get("ghost").is_none());
        assert!(table.unverified.contains_key("ghost"));

        // desktop revokes laptop; the statement verifies against desktop's known key, and a
        // hub that later lists laptop as online again changes nothing.
        let statement = RevocationStatement {
            name: "laptop".to_string(),
            node_pub: crate::world::crypto::b64_encode(&laptop.public_key()),
            by: "desktop".to_string(),
            at: "2026-09-04T01:00:00Z".to_string(),
            reason: "revoked".to_string(),
        };
        second[1].state = "revoked".to_string();
        second[1].revocation = Some(sign_revocation(&desktop, &statement).unwrap());
        let table = MemberTable::from_entries(3, &second, &keys, &table);
        assert!(table.revoked.contains_key("laptop"));
        assert!(table.get("laptop").is_none());
        let mut resurrected = vec![
            entry(&desktop, &keys, "desktop", &[], "online"),
            entry(&laptop, &keys, "laptop", &[], "online"),
        ];
        let table = MemberTable::from_entries(4, &resurrected, &keys, &table);
        assert!(table.get("laptop").is_none());
        assert!(
            table
                .expand(&Selector::parse_items(&["laptop".to_string()]).unwrap())
                .unwrap_err()
                .contains("revoked")
        );
        // Nor under a new name.
        resurrected[1] = entry(&laptop, &keys, "laptop2", &[], "online");
        let table = MemberTable::from_entries(5, &resurrected, &keys, &table);
        assert!(table.get("laptop2").is_none());

        // A revocation signed by a stranger is ignored.
        let stranger = Identity::generate().unwrap();
        let bogus = RevocationStatement {
            name: "desktop".to_string(),
            node_pub: crate::world::crypto::b64_encode(&desktop.public_key()),
            by: "nobody".to_string(),
            at: "2026-09-04T01:00:00Z".to_string(),
            reason: "revoked".to_string(),
        };
        let mut forged = vec![entry(&desktop, &keys, "desktop", &[], "revoked")];
        forged[0].revocation = Some(sign_revocation(&stranger, &bogus).unwrap());
        let table = MemberTable::from_entries(6, &forged, &keys, &table);
        assert!(!table.revoked.contains_key("desktop"));
        // Reported revoked without proof: identity kept, not current, not selectable.
        assert!(table.identity("desktop").is_some());
        assert!(table.get("desktop").is_none());
    }
}
