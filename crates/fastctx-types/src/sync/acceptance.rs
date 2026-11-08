//! Visibility-separated authority evidence survives forwarding and backup recovery.

use super::{
    catalog::Authority,
    identity::{AccountId, AuthorityEpoch, DeviceId, Digest, HubId, ObjectKey, OperationId, ScopeId},
    mutation::AcceptedObject,
    proof::SignedStatement,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Recorded evidence of the authenticated actor, never a caller-supplied grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "actor", rename_all = "snake_case")]
pub enum AcceptedActor {
    Account { account: AccountId, device: DeviceId },
    Agent { account: AccountId, device: DeviceId, conversation: ObjectKey },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceBinding {
    pub format: u16,
    pub hub: HubId,
    pub authority: Authority,
    pub authority_epoch: AuthorityEpoch,
    pub operation: OperationId,
    pub request_hash: Digest,
    pub actor: AcceptedActor,
}

/// A fragment matches the unit negotiated and filtered by replica custody.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FragmentKey {
    pub scope: ScopeId,
    pub kind: String,
    pub schema: u16,
}

/// One visible kind/schema's part of a single atomic acceptance. All allocated versions belong
/// to binding.authority_epoch; parent and selected versions may be older.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceFragment {
    pub binding: AcceptanceBinding,
    pub scope: ScopeId,
    pub kind: String,
    pub schema: u16,
    pub objects: Vec<AcceptedObject>,
}
impl AcceptanceFragment {
    pub fn key(&self) -> FragmentKey {
        FragmentKey { scope: self.scope.clone(), kind: self.kind.clone(), schema: self.schema }
    }
    pub fn digest(&self) -> Result<Digest, String> {
        let mut hash = Sha256::new();
        hash.update(b"fastctx-sync/accepted-fragment/v1\0");
        hash.update(super::canonical::encode(self)?);
        Ok(Digest::from_bytes(hash.finalize().into()))
    }
}

/// This exact binding is signed under accepted-operation/v1. Other visibility groups are
/// hidden; the authority's signature attests membership in the result commitment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedFragment {
    pub fragment: AcceptanceFragment,
    pub fragment_digest: Digest,
    pub result_digest: Digest,
    pub index: u32,
    pub count: u32,
}

impl CommittedFragment {
    /// Structural checks supplement signature verification; they grant no authority.
    pub fn validate(&self) -> Result<(), String> {
        let fragment = &self.fragment;
        let binding = &fragment.binding;
        if binding.format != 1
            || self.count == 0
            || self.count as usize > super::mutation::MAX_ATOMIC_OBJECTS
            || self.index >= self.count
            || fragment.objects.is_empty()
            || fragment.objects.len() > super::mutation::MAX_ATOMIC_OBJECTS
        {
            return Err("invalid acceptance fragment bounds".into());
        }
        let mut objects = std::collections::BTreeSet::new();
        for object in &fragment.objects {
            if object.key.scope.hub != binding.hub
                || object.key.scope.scope != fragment.scope
                || object.kind != fragment.kind
                || object.schema != fragment.schema
                || object.allocated.authority_epoch != binding.authority_epoch
                || !objects.insert(&object.key.object)
                || [&object.content, &object.current_content, &object.history]
                    .into_iter()
                    .flatten()
                    .any(|root| root.domain != object.key.scope)
            {
                return Err("accepted object differs from its scope or authority binding".into());
            }
            let selection_matches = match &object.acceptance {
                super::mutation::Acceptance::Applied | super::mutation::Acceptance::Merged => {
                    object.current_version == object.allocated && object.current_content == object.content
                }
                super::mutation::Acceptance::Branched { conflict, alternative } => {
                    conflict == &binding.operation
                        && alternative == &object.allocated
                        && object.current_version != object.allocated
                }
            };
            if !selection_matches {
                return Err("accepted object selection differs from its result".into());
            }
        }
        if fragment.digest()? != self.fragment_digest {
            return Err("acceptance fragment digest differs from its content".into());
        }
        Ok(())
    }
}

/// Retained byte-for-byte with the operation receipt and referenced versions.
/// A relay may forward only fragments whose scope, kind and schema it is authorized to retain and disclose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedAcceptanceFragment {
    pub proof: SignedStatement,
}

/// The complete result commits to visibility-sorted fragment digests without exposing
/// their contents. Callers must supply distinct (scope, kind, schema) keys in canonical order.
pub fn result_digest(fragments: &[(FragmentKey, Digest)]) -> Result<Digest, String> {
    if fragments.is_empty()
        || fragments.len() > super::mutation::MAX_ATOMIC_OBJECTS
        || fragments.windows(2).any(|pair| pair[0].0 >= pair[1].0)
    {
        return Err("an acceptance requires distinct visibility-sorted fragments within its object budget".into());
    }
    let mut hash = Sha256::new();
    hash.update(b"fastctx-sync/accepted-result/v1\0");
    hash.update((fragments.len() as u32).to_be_bytes());
    for (_, digest) in fragments {
        hash.update(digest.bytes());
    }
    Ok(Digest::from_bytes(hash.finalize().into()))
}

/// Checks a complete recovered commitment after each fragment's signature and
/// authority were verified. A partial set remains explicitly incomplete.
pub fn validate_complete(fragments: &[CommittedFragment]) -> Result<(), String> {
    let Some(first) = fragments.first() else {
        return Err("an acceptance has no fragments".into());
    };
    if fragments.len() != first.count as usize {
        return Err("the accepted operation is only partially restored".into());
    }
    let mut ordered: Vec<_> = fragments.iter().collect();
    ordered.sort_unstable_by_key(|fragment| fragment.index);
    let mut digests = Vec::with_capacity(ordered.len());
    let mut objects = 0;
    for (index, fragment) in ordered.into_iter().enumerate() {
        fragment.validate()?;
        if fragment.index as usize != index
            || fragment.count != first.count
            || fragment.fragment.binding != first.fragment.binding
            || fragment.result_digest != first.result_digest
        {
            return Err("acceptance fragments do not share one complete commitment".into());
        }
        objects += fragment.fragment.objects.len();
        digests.push((fragment.fragment.key(), fragment.fragment_digest.clone()));
    }
    if objects > super::mutation::MAX_ATOMIC_OBJECTS || result_digest(&digests)? != first.result_digest {
        return Err("accepted operation commitment does not match its fragments".into());
    }
    Ok(())
}
