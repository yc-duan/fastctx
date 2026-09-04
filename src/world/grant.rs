//! Grants: principal × member set × verbs (`design-protocol.md` §4), published as one
//! signed snapshot.
//!
//! The whole set in force is a single document with a revision number, MAC'd under the World
//! key and signed by the member that published it. The hub stores the latest snapshot and
//! relays it byte-exact; members verify it and refuse anything older than what they hold.
//! Publishing the set as one document is what makes narrowing hold against the hub: a set of
//! individually signed grants could be thinned by simply leaving entries out, and an empty
//! list would then read as "no grant was ever published" — the permissive default. With a
//! snapshot, an empty set is a signed, deliberate act, and omission has no representation.
//!
//! With no snapshot published, the v1 default applies: every member may use every verb on
//! every member.

use super::crypto::{self, Key32, b64_decode, b64_encode};
use super::identity::{self, Identity};
use super::keys::KeyRing;
use super::{WorldPaths, read_optional, write_atomic};
use serde::{Deserialize, Serialize};

const GRANTS_FILE_VERSION: u32 = 2;
/// Signature domain of a grant snapshot.
pub(crate) const GRANT_SET_DOMAIN: &str = "grant_set";
/// Every verb a grant can name, in manifest order plus the World verbs.
pub(crate) const ALL_VERBS: [&str; 8] = [
    "inspect_local_file",
    "grep",
    "glob",
    "replace",
    "run",
    "tasks",
    "copy",
    "lease",
];

/// One grant shape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Grant {
    /// Member name, or `*` for every member.
    pub(crate) principal: String,
    /// Selector items over target members: names, `tag:<t>`, or `all`.
    pub(crate) nodes: Vec<String>,
    /// Allowed verbs; `*` means every verb.
    pub(crate) verbs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires: Option<String>,
}

impl Grant {
    /// The implicit v1 default when no grant has been published.
    pub(crate) fn default_all() -> Self {
        Self {
            principal: "*".to_string(),
            nodes: vec!["all".to_string()],
            verbs: vec!["*".to_string()],
            expires: None,
        }
    }

    fn matches_principal(&self, principal: &str) -> bool {
        self.principal == "*" || self.principal == principal
    }

    fn matches_verb(&self, verb: &str) -> bool {
        self.verbs.iter().any(|entry| entry == "*" || entry == verb)
    }

    fn matches_node(&self, node: &str, node_tags: &[String]) -> bool {
        self.nodes.iter().any(|item| {
            item == "all"
                || item == node
                || item
                    .strip_prefix("tag:")
                    .is_some_and(|tag| node_tags.iter().any(|entry| entry == tag))
        })
    }

    fn is_expired(&self, now: time::OffsetDateTime) -> bool {
        self.expires
            .as_deref()
            .and_then(|text| super::parse_rfc3339(text).ok())
            .is_some_and(|expiry| expiry <= now)
    }
}

/// One grant with its id, as it sits inside a snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct GrantEntry {
    pub(crate) id: String,
    #[serde(flatten)]
    pub(crate) grant: Grant,
}

/// The whole grant set as one document; its canonical JSON bytes are what is MAC'd and signed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct GrantSnapshot {
    /// Strictly increasing; a member never replaces its set with a lower revision.
    pub(crate) revision: u64,
    pub(crate) grants: Vec<GrantEntry>,
    pub(crate) published_by: String,
    pub(crate) published_at: String,
}

/// A snapshot as the hub stores and relays it: the exact bytes the publisher signed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SignedGrantSet {
    /// JSON text of a `GrantSnapshot`, byte-exact.
    pub(crate) snapshot: String,
    /// HMAC-SHA256 under `K_mac`, base64.
    pub(crate) mac: String,
    /// World key epoch the MAC was made under.
    pub(crate) mac_epoch: u32,
    /// Ed25519 signature by the publishing member over `snapshot` bytes (domain `grant_set`).
    pub(crate) sig: String,
}

impl SignedGrantSet {
    pub(crate) fn parse(&self) -> Result<GrantSnapshot, String> {
        serde_json::from_str(&self.snapshot)
            .map_err(|error| format!("the grant snapshot is not valid JSON: {error}"))
    }

    /// Verifies the publisher's signature only; the hub, holding no World key, checks this
    /// much and stores the rest for members to verify.
    pub(crate) fn verify_signature(&self, publisher_key: &Key32) -> Result<(), String> {
        let signature = b64_decode(&self.sig)
            .map_err(|error| format!("invalid grant snapshot signature: {error}"))?;
        identity::verify(
            publisher_key,
            GRANT_SET_DOMAIN,
            self.snapshot.as_bytes(),
            &signature,
        )
        .map_err(|_| {
            "the grant snapshot's signature does not verify against its publisher".to_string()
        })
    }
}

/// A change to apply on top of the set in force.
#[derive(Clone, Debug)]
pub(crate) enum GrantChange {
    /// Adds a grant, or replaces the one with the same id.
    Set(GrantEntry),
    /// Removes the grant with this id.
    Remove(String),
}

/// The grants in force on this member: the last snapshot that verified.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct GrantSet {
    /// Revision of the snapshot these came from; 0 when none was ever received.
    pub(crate) revision: u64,
    pub(crate) grants: Vec<GrantEntry>,
    #[serde(default)]
    pub(crate) published_by: String,
    #[serde(default)]
    pub(crate) published_at: String,
    /// The signed document these were verified from, kept so it can be shown and re-checked.
    #[serde(default)]
    pub(crate) signed: Option<SignedGrantSet>,
}

#[derive(Deserialize, Serialize)]
struct GrantsFile {
    v: u32,
    set: GrantSet,
}

impl GrantSet {
    /// Whether `principal` may run `verb` on `node` (whose tags are given). An empty set,
    /// whether never published or published empty, means the v1 default.
    pub(crate) fn allows(
        &self,
        principal: &str,
        verb: &str,
        node: &str,
        node_tags: &[String],
    ) -> bool {
        let now = time::OffsetDateTime::now_utc();
        if self.grants.is_empty() {
            return Grant::default_all().matches_verb(verb);
        }
        self.grants.iter().any(|entry| {
            !entry.grant.is_expired(now)
                && entry.grant.matches_principal(principal)
                && entry.grant.matches_verb(verb)
                && entry.grant.matches_node(node, node_tags)
        })
    }

    /// The set a verified snapshot establishes.
    pub(crate) fn from_verified(snapshot: GrantSnapshot, signed: SignedGrantSet) -> Self {
        Self {
            revision: snapshot.revision,
            grants: snapshot.grants,
            published_by: snapshot.published_by,
            published_at: snapshot.published_at,
            signed: Some(signed),
        }
    }

    /// The entries after `change`, for the next snapshot.
    pub(crate) fn entries_after(&self, change: &GrantChange) -> Result<Vec<GrantEntry>, String> {
        let mut entries = self.grants.clone();
        match change {
            GrantChange::Set(entry) => {
                match entries.iter_mut().find(|existing| existing.id == entry.id) {
                    Some(existing) => *existing = entry.clone(),
                    None => entries.push(entry.clone()),
                }
            }
            GrantChange::Remove(id) => {
                let before = entries.len();
                entries.retain(|entry| entry.id != *id);
                if entries.len() == before {
                    return Err(format!(
                        "No grant has the id \"{id}\"; list the grants in force with 'fastctx world grants'."
                    ));
                }
            }
        }
        Ok(entries)
    }

    /// Loads the cached set; an older file format is treated as no snapshot at all, which
    /// the next connection repairs.
    pub(crate) fn load(paths: &WorldPaths) -> Result<Option<Self>, String> {
        let Some(bytes) = read_optional(&paths.grants)? else {
            return Ok(None);
        };
        let version: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Cannot parse {}: {error}",
                crate::paths::display_path(&paths.grants)
            )
        })?;
        let version = version
            .get("v")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        if version > GRANTS_FILE_VERSION {
            return Err(format!(
                "{} was written by a newer fastctx (format {}); this build reads format {} at most.",
                crate::paths::display_path(&paths.grants),
                version,
                GRANTS_FILE_VERSION
            ));
        }
        if version < GRANTS_FILE_VERSION {
            return Ok(None);
        }
        let file: GrantsFile = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Cannot parse {}: {error}",
                crate::paths::display_path(&paths.grants)
            )
        })?;
        Ok(Some(file.set))
    }

    pub(crate) fn save(&self, paths: &WorldPaths) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(&GrantsFile {
            v: GRANTS_FILE_VERSION,
            set: self.clone(),
        })
        .map_err(|error| format!("Cannot encode the grant set: {error}"))?;
        write_atomic(&paths.grants, &json)
    }
}

/// MACs and signs a snapshot as the publishing member.
pub(crate) fn sign_snapshot(
    identity: &Identity,
    keys: &KeyRing,
    snapshot: &GrantSnapshot,
) -> Result<SignedGrantSet, String> {
    let bytes = serde_json::to_vec(snapshot)
        .map_err(|error| format!("Cannot encode the grant snapshot: {error}"))?;
    let current = keys.current();
    Ok(SignedGrantSet {
        mac: b64_encode(&crypto::hmac_sha256(&current.subkeys().mac, &bytes)),
        mac_epoch: current.epoch(),
        sig: b64_encode(&identity.sign(GRANT_SET_DOMAIN, &bytes)),
        snapshot: String::from_utf8(bytes).expect("serde_json produces UTF-8"),
    })
}

/// Verifies a snapshot's MAC and its publisher's signature, returning the parsed snapshot.
/// `publisher_key` resolves a member name to a key this member already trusts.
pub(crate) fn verify_snapshot(
    signed: &SignedGrantSet,
    keys: &KeyRing,
    publisher_key: impl Fn(&str) -> Option<Key32>,
) -> Result<GrantSnapshot, String> {
    let subkeys = keys.subkeys(signed.mac_epoch).ok_or_else(|| {
        format!(
            "the grant snapshot is MAC'd under World key epoch {}, which this member does not hold",
            signed.mac_epoch
        )
    })?;
    let mac = b64_decode(&signed.mac).map_err(|error| format!("invalid grant MAC: {error}"))?;
    if !crypto::hmac_verify(&subkeys.mac, signed.snapshot.as_bytes(), &mac) {
        return Err("the grant snapshot's MAC does not verify under the World key".to_string());
    }
    let snapshot = signed.parse()?;
    let key = publisher_key(&snapshot.published_by).ok_or_else(|| {
        format!(
            "the grant snapshot was published by \"{}\", which is not a verified member",
            snapshot.published_by
        )
    })?;
    signed.verify_signature(&key)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::{
        Grant, GrantChange, GrantEntry, GrantSet, GrantSnapshot, sign_snapshot, verify_snapshot,
    };
    use crate::world::identity::Identity;
    use crate::world::keys::KeyRing;

    #[test]
    fn the_default_allows_everything_and_a_signed_snapshot_narrows_it() {
        let empty = GrantSet::default();
        assert!(empty.allows("desktop", "run", "vps", &[]));

        let keys = KeyRing::new_initial().unwrap();
        let owner = Identity::generate().unwrap();
        let entry = GrantEntry {
            id: "g-1".to_string(),
            grant: Grant {
                principal: "desktop".to_string(),
                nodes: vec!["tag:office".to_string()],
                verbs: vec!["grep".to_string(), "glob".to_string()],
                expires: None,
            },
        };
        let snapshot = GrantSnapshot {
            revision: 1,
            grants: vec![entry.clone()],
            published_by: "desktop".to_string(),
            published_at: "2026-09-04T00:00:00Z".to_string(),
        };
        let signed = sign_snapshot(&owner, &keys, &snapshot).unwrap();
        let lookup = |name: &str| (name == "desktop").then_some(owner.public_key());
        assert_eq!(verify_snapshot(&signed, &keys, lookup).unwrap(), snapshot);
        let set = GrantSet::from_verified(snapshot, signed.clone());
        assert!(set.allows("desktop", "grep", "laptop", &["office".to_string()]));
        assert!(!set.allows("desktop", "run", "laptop", &["office".to_string()]));
        assert!(!set.allows("desktop", "grep", "vps", &[]));
        assert!(!set.allows("laptop", "grep", "laptop", &["office".to_string()]));

        // A snapshot from a name this member has not verified is worthless, and so is one whose
        // bytes were touched after signing.
        let stranger = |_: &str| None;
        assert!(verify_snapshot(&signed, &keys, stranger).is_err());
        let mut thinned = signed.clone();
        thinned.snapshot = thinned.snapshot.replace("\"grep\",", "");
        assert!(verify_snapshot(&thinned, &keys, lookup).is_err());
        assert!(verify_snapshot(&signed, &KeyRing::new_initial().unwrap(), lookup).is_err());

        let removed = set
            .entries_after(&GrantChange::Remove("g-1".to_string()))
            .unwrap();
        assert!(removed.is_empty());
        assert!(
            set.entries_after(&GrantChange::Remove("g-9".to_string()))
                .is_err()
        );
        let replaced = set
            .entries_after(&GrantChange::Set(GrantEntry {
                id: "g-1".to_string(),
                grant: Grant::default_all(),
            }))
            .unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].grant, Grant::default_all());
    }
}
