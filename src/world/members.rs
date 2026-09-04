//! The member's view of who is in the World: records verified by MAC, the selectors that
//! name members in tool calls, and the local cache file.

use super::crypto::{self, Key32, b64_array, b64_decode, b64_encode};
use super::identity::{self, Identity};
use super::keys::KeyRing;
use super::messages::{MemberEntry, MemberRecord, SignedRecord};
use super::{WorldPaths, read_optional, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MEMBERS_FILE_VERSION: u32 = 1;
/// Signature domain of a member record.
pub(crate) const MEMBER_RECORD_DOMAIN: &str = "member_record";

/// A member whose record verified under the World key: the only kind other members act on.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct VerifiedMember {
    pub(crate) record: MemberRecord,
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
}

/// The cached member table, keyed by name.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct MemberTable {
    pub(crate) version: u64,
    pub(crate) members: BTreeMap<String, VerifiedMember>,
    /// Members the hub listed whose record did not verify, with the reason; kept so a
    /// `nodes` listing can say why a name is missing rather than hiding it.
    #[serde(default)]
    pub(crate) unverified: BTreeMap<String, String>,
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

    /// Rebuilds the table from a hub listing, verifying every record's MAC and signature.
    pub(crate) fn from_entries(version: u64, entries: &[MemberEntry], keys: &KeyRing) -> Self {
        let mut table = Self {
            version,
            ..Self::default()
        };
        for entry in entries {
            let Some(signed) = &entry.signed else {
                table.unverified.insert(
                    entry.name.clone(),
                    "the hub holds no published record for this member".to_string(),
                );
                continue;
            };
            match verify_record(signed, keys) {
                Ok(record) if record.name == entry.name => {
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
                        },
                    );
                }
                Ok(record) => {
                    table.unverified.insert(
                        entry.name.clone(),
                        format!(
                            "the record is signed for \"{}\" but the hub lists it as \"{}\"",
                            record.name, entry.name
                        ),
                    );
                }
                Err(error) => {
                    table.unverified.insert(entry.name.clone(), error);
                }
            }
        }
        table
    }

    pub(crate) fn get(&self, name: &str) -> Option<&VerifiedMember> {
        self.members.get(name)
    }

    /// Expands a selector (`design-objects.md` §2.1) into member names.
    ///
    /// `all` and tags expand to online members only; explicit names are kept regardless so a
    /// call to an offline member is answered with its real state instead of silently dropped.
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
                    for member in self.members.values().filter(|member| member.is_online()) {
                        push(member.record.name.clone());
                    }
                }
                tagged if tagged.starts_with("tag:") => {
                    let tag = &tagged[4..];
                    if tag.is_empty() {
                        return Err("A tag selector needs a tag after \"tag:\".".to_string());
                    }
                    for member in self.members.values().filter(|member| {
                        member.is_online() && member.record.tags.iter().any(|entry| entry == tag)
                    }) {
                        push(member.record.name.clone());
                    }
                }
                name => {
                    if self.members.contains_key(name) {
                        push(name.to_string());
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

#[cfg(test)]
mod tests {
    use super::{MemberTable, Selector, publish_record, verify_record};
    use crate::world::identity::Identity;
    use crate::world::keys::KeyRing;
    use crate::world::messages::{MemberEntry, MemberRecord};

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
    fn selectors_expand_names_tags_and_all_over_online_members() {
        let keys = KeyRing::new_initial().unwrap();
        let entries = [
            ("desktop", &["office"][..], "online"),
            ("laptop", &["office", "mobile"][..], "offline"),
            ("vps", &[][..], "online"),
        ]
        .iter()
        .map(|(name, tags, state)| {
            let identity = Identity::generate().unwrap();
            MemberEntry {
                name: name.to_string(),
                signed: Some(
                    publish_record(&identity, &keys, &record(&identity, name, tags)).unwrap(),
                ),
                state: state.to_string(),
                last_seen: "2026-09-04T00:00:00Z".to_string(),
                hub_rtt_ms: None,
                tls: None,
                network: None,
                version: None,
                inventory_version: 0,
            }
        })
        .collect::<Vec<_>>();
        let table = MemberTable::from_entries(3, &entries, &keys);
        assert_eq!(table.members.len(), 3);
        let all = table
            .expand(&Selector::parse_items(&["all".to_string()]).unwrap())
            .unwrap();
        assert_eq!(all, vec!["desktop", "vps"]);
        let office = table
            .expand(&Selector::parse_items(&["tag:office".to_string()]).unwrap())
            .unwrap();
        assert_eq!(office, vec!["desktop"]);
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
}
