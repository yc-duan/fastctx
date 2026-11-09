//! Complete, ready account adapters for trusted service composition.
//!
//! A reader policy is mandatory even when a schema and mutation authorizer exist.
//! This module uses the foundation's existing AccessPolicy; it does not introduce
//! a second authorization trait or replace scope/holding checks.
use crate::{
    account::{Authorizer, Handler, Kind},
    context::AccountState,
    registry::{self, Readiness},
};
use fastctx_sync::{
    identity::ObjectKey,
    registry::{REGISTRY, handlers::KindHandler},
    store::{
        access::{AccessContext, AccessPolicy, Decision},
        authorize::MutationAuthorizer,
    },
};
use std::sync::Arc;

/// An actual reader implementation bound to one kind, not a readiness flag.
/// The supplied policy must enforce that kind's raw-object reader restrictions
/// as well as current scope/object access. Restricted views use separate trusted
/// projections; their readers must not gain access to the original object.
#[derive(Clone)]
pub struct KindReaderPolicy {
    kind: Kind,
    policy: Arc<dyn AccessPolicy>,
}
impl KindReaderPolicy {
    pub fn new(kind: Kind, policy: Arc<dyn AccessPolicy>) -> Self {
        Self { kind, policy }
    }
    pub fn kind(&self) -> &'static str {
        self.kind.id()
    }
    /// Runs both the kind gate and the final object gate, with current ACL fences.
    pub fn can_read_object(&self, context: &AccessContext, object: &ObjectKey) -> Decision {
        if context.scope != object.scope {
            return Decision::Denied { revision: context.observed_acl, reason: "read scope mismatch".into() };
        }
        let Ok(kind) = REGISTRY.asset(self.kind()) else { return Decision::Unavailable };
        match self.policy.can_read(context, kind.registered()) {
            Decision::Allowed { revision } if revision == context.observed_acl => {}
            Decision::Allowed { .. } => return Decision::Unavailable,
            other => return other,
        }
        match self.policy.can_read_object(context, kind.registered(), object) {
            Decision::Allowed { revision } if revision != context.observed_acl => Decision::Unavailable,
            other => other,
        }
    }
}

/// Install all three ports together before advertising this kind. In particular,
/// local and legacy body projections must pass `reader.can_read_object` first.
pub struct Installation {
    pub handler: Arc<dyn KindHandler>,
    pub authorizer: Arc<dyn MutationAuthorizer>,
    pub reader: KindReaderPolicy,
}

/// Returns only approved, ready kinds that also have a supplied reader policy.
/// Uses the canonical intake list, never caller-declared readiness. Schema-only
/// callers can construct `account::Handler` without activating a service.
/// No current account kind is ready while its listed foundation ports are absent.
pub fn ready(state: Arc<dyn AccountState>, readers: &[KindReaderPolicy]) -> Vec<Installation> {
    registry::entries()
        .into_iter()
        .filter(|entry| entry.readiness() == Readiness::Ready)
        .filter_map(|entry| {
            let kind = Kind::ALL.into_iter().find(|kind| kind.id() == entry.registration.id)?;
            let reader = readers.iter().find(|reader| reader.kind == kind)?.clone();
            Some(Installation {
                handler: Arc::new(Handler(kind)),
                authorizer: Arc::new(Authorizer::new(kind, state.clone())),
                reader,
            })
        })
        .collect()
}
