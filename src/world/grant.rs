//! Grants: principal × member set × verbs (`design-protocol.md` §4).
//!
//! The hub keeps grant shapes to route and refuse; members keep MAC-verified copies and
//! decide for themselves. With no grant published, the v1 default applies: every member may
//! use every verb on every member.

use super::crypto::{self, b64_decode, b64_encode};
use super::identity::{self, Identity};
use super::keys::KeyRing;
use super::messages::SignedGrant;
use super::{WorldPaths, read_optional, write_atomic};
use serde::{Deserialize, Serialize};

const GRANTS_FILE_VERSION: u32 = 1;
/// Signature domain of a grant.
pub(crate) const GRANT_DOMAIN: &str = "grant";
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

/// The grants in force, as verified by a member or as shaped by the hub.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct GrantSet {
    pub(crate) version: u64,
    pub(crate) grants: Vec<(String, Grant)>,
}

#[derive(Deserialize, Serialize)]
struct GrantsFile {
    v: u32,
    set: GrantSet,
}

impl GrantSet {
    /// Whether `principal` may run `verb` on `node` (whose tags are given).
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
        self.grants.iter().any(|(_, grant)| {
            !grant.is_expired(now)
                && grant.matches_principal(principal)
                && grant.matches_verb(verb)
                && grant.matches_node(node, node_tags)
        })
    }

    pub(crate) fn load(paths: &WorldPaths) -> Result<Option<Self>, String> {
        let Some(bytes) = read_optional(&paths.grants)? else {
            return Ok(None);
        };
        let file: GrantsFile = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Cannot parse {}: {error}",
                crate::paths::display_path(&paths.grants)
            )
        })?;
        if file.v > GRANTS_FILE_VERSION {
            return Err(format!(
                "{} was written by a newer fastctx (format {}); this build reads format {} at most.",
                crate::paths::display_path(&paths.grants),
                file.v,
                GRANTS_FILE_VERSION
            ));
        }
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

    /// Rebuilds the set from a hub `grant_sync`, keeping only grants that verify.
    pub(crate) fn from_signed(
        version: u64,
        signed: &[SignedGrant],
        keys: &KeyRing,
        publisher_key: impl Fn(&str) -> Option<crypto::Key32>,
    ) -> (Self, Vec<String>) {
        let mut set = Self {
            version,
            grants: Vec::new(),
        };
        let mut rejected = Vec::new();
        for entry in signed {
            match verify_grant(entry, keys, &publisher_key) {
                Ok(grant) => set.grants.push((entry.id.clone(), grant)),
                Err(error) => rejected.push(format!("{}: {error}", entry.id)),
            }
        }
        (set, rejected)
    }
}

/// Signs a grant as the publishing member.
pub(crate) fn publish_grant(
    identity: &Identity,
    keys: &KeyRing,
    publisher: &str,
    id: &str,
    grant: &Grant,
) -> Result<SignedGrant, String> {
    let bytes =
        serde_json::to_vec(grant).map_err(|error| format!("Cannot encode the grant: {error}"))?;
    let current = keys.current();
    Ok(SignedGrant {
        id: id.to_string(),
        mac: b64_encode(&crypto::hmac_sha256(&current.subkeys().mac, &bytes)),
        mac_epoch: current.epoch(),
        sig: b64_encode(&identity.sign(GRANT_DOMAIN, &bytes)),
        grant: String::from_utf8(bytes).expect("serde_json produces UTF-8"),
        published_by: publisher.to_string(),
    })
}

/// Verifies a grant's MAC and its publisher's signature.
pub(crate) fn verify_grant(
    signed: &SignedGrant,
    keys: &KeyRing,
    publisher_key: impl Fn(&str) -> Option<crypto::Key32>,
) -> Result<Grant, String> {
    let subkeys = keys.subkeys(signed.mac_epoch).ok_or_else(|| {
        format!(
            "the grant is MAC'd under World key epoch {}, which this member does not hold",
            signed.mac_epoch
        )
    })?;
    let mac = b64_decode(&signed.mac).map_err(|error| format!("invalid grant MAC: {error}"))?;
    if !crypto::hmac_verify(&subkeys.mac, signed.grant.as_bytes(), &mac) {
        return Err("the grant's MAC does not verify under the World key".to_string());
    }
    let key = publisher_key(&signed.published_by).ok_or_else(|| {
        format!(
            "the grant was published by \"{}\", which is not a verified member",
            signed.published_by
        )
    })?;
    let signature =
        b64_decode(&signed.sig).map_err(|error| format!("invalid grant signature: {error}"))?;
    identity::verify(&key, GRANT_DOMAIN, signed.grant.as_bytes(), &signature)
        .map_err(|_| "the grant's signature does not verify against its publisher".to_string())?;
    serde_json::from_str(&signed.grant)
        .map_err(|error| format!("the grant is not valid JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{Grant, GrantSet, publish_grant, verify_grant};
    use crate::world::identity::Identity;
    use crate::world::keys::KeyRing;

    #[test]
    fn the_default_allows_everything_and_a_published_grant_narrows_it() {
        let empty = GrantSet::default();
        assert!(empty.allows("desktop", "run", "vps", &[]));

        let keys = KeyRing::new_initial().unwrap();
        let owner = Identity::generate().unwrap();
        let grant = Grant {
            principal: "desktop".to_string(),
            nodes: vec!["tag:office".to_string()],
            verbs: vec!["grep".to_string(), "glob".to_string()],
            expires: None,
        };
        let signed = publish_grant(&owner, &keys, "desktop", "g-1", &grant).unwrap();
        let lookup = |name: &str| (name == "desktop").then_some(owner.public_key());
        assert_eq!(verify_grant(&signed, &keys, lookup).unwrap(), grant);
        let (set, rejected) =
            GrantSet::from_signed(1, std::slice::from_ref(&signed), &keys, lookup);
        assert!(rejected.is_empty());
        assert!(set.allows("desktop", "grep", "laptop", &["office".to_string()]));
        assert!(!set.allows("desktop", "run", "laptop", &["office".to_string()]));
        assert!(!set.allows("desktop", "grep", "vps", &[]));
        assert!(!set.allows("laptop", "grep", "laptop", &["office".to_string()]));

        let stranger = |_: &str| None;
        assert!(verify_grant(&signed, &keys, stranger).is_err());
    }
}
