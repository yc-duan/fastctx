//! Intake readiness is separate from the foundation's persistence registration.
//! No entry advertises service readiness merely because its schema is available.
use crate::{
    account::{Authorizer, Handler, Kind},
    context::AccountState,
};
use fastctx_sync::registry::{DataClass, Lightweight, REGISTRY, Registration, handlers::KindHandler};
use fastctx_sync::store::authorize::MutationAuthorizer;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingPort {
    BusinessHandler,
    SourceAuthorizer,
    DirectoryControlFacts,
    InternalServiceSubmission,
    LegacyNodeDelegation,
    KindReaderPolicy,

    AtomicPublication,
    PreparedDirectory,
    ProjectionContext,
    DerivedSourceBinding,
    FoundationRegistration,
    MachineAccountWorkScope,
}
#[derive(Clone, Debug)]
pub struct Intake {
    pub registration: &'static Registration,
    pub owner: &'static str,
    pub schema_available: bool,
    pub missing: &'static [MissingPort],
}
impl Intake {
    /// Advertising follows complete service composition, not enum registration.
    pub fn can_advertise(&self) -> bool {
        self.missing.is_empty()
    }
}

pub fn entries() -> Vec<Intake> {
    REGISTRY
        .entries()
        .iter()
        .filter(|entry| {
            entry.class == DataClass::Asset
                || entry.class == DataClass::Derived && entry.mobile != Lightweight::LocalOnly
        })
        .map(|entry| {
            let (owner, schema_available, missing): (_, _, &'static [MissingPort]) = match entry.id {
                "account.directory" => (
                    "fastctx-assets::account",
                    true,
                    &[
                        MissingPort::SourceAuthorizer,
                        MissingPort::DirectoryControlFacts,
                        MissingPort::ProjectionContext,
                    ],
                ),
                "account.profile" | "account.collaboration" | "account.settings" => (
                    "fastctx-assets::account",
                    true,
                    &[
                        MissingPort::ProjectionContext,
                        MissingPort::InternalServiceSubmission,
                        MissingPort::LegacyNodeDelegation,
                        MissingPort::KindReaderPolicy,
                        MissingPort::AtomicPublication,
                    ],
                ),
                "account.skill" => (
                    "fastctx-assets::account::skill / materialization",
                    true,
                    &[
                        MissingPort::PreparedDirectory,
                        MissingPort::AtomicPublication,
                        MissingPort::ProjectionContext,
                        MissingPort::FoundationRegistration,
                        MissingPort::LegacyNodeDelegation,
                    ],
                ),
                "conversation.timeline" => {
                    ("conversation ingestion", false, &[MissingPort::BusinessHandler, MissingPort::SourceAuthorizer])
                }
                "conversation.timeline-observation" => {
                    ("legacy migration", false, &[MissingPort::BusinessHandler, MissingPort::SourceAuthorizer])
                }
                "conversation.metadata" | "conversation.plan" | "conversation.history" | "project.history" => {
                    ("conversation documents", false, &[MissingPort::BusinessHandler])
                }
                "conversation.goal" | "conversation.sentinel" | "conversation.sentinel-run" | "conversation.timer" => {
                    ("goals and sentinels", false, &[MissingPort::BusinessHandler])
                }
                "conversation.draft" => {
                    ("shared drafts", false, &[MissingPort::BusinessHandler, MissingPort::AtomicPublication])
                }
                "project.ledger" | "project.page" => {
                    ("project assets", false, &[MissingPort::BusinessHandler, MissingPort::AtomicPublication])
                }
                "conversation.artifact" | "conversation.attachment" | "conversation.picture" | "job.output" => {
                    ("materialization", false, &[MissingPort::BusinessHandler])
                }
                "derived.thumbnail" => (
                    "thumbnail materialization",
                    false,
                    &[
                        MissingPort::BusinessHandler,
                        MissingPort::DerivedSourceBinding,
                        MissingPort::FoundationRegistration,
                    ],
                ),
                "migration.legacy-pins" => ("legacy migration", false, &[MissingPort::BusinessHandler]),
                "device.pending-edit" => ("foundation pending engine", false, &[MissingPort::BusinessHandler]),
                "account.notice" => (
                    "diagnostic records",
                    false,
                    &[MissingPort::FoundationRegistration, MissingPort::BusinessHandler, MissingPort::SourceAuthorizer],
                ),
                "conversation.agents"
                | "job.facts"
                | "account.usage"
                | "account.activity"
                | "account.read-position"
                | "machine.settings"
                | "machine.audit"
                | "machine.connection"
                | "device.preferences" => ("account and machine records", false, &[MissingPort::BusinessHandler]),
                _ => (entry.owner, false, &[MissingPort::BusinessHandler]),
            };
            Intake { registration: entry, owner, schema_available, missing }
        })
        .collect()
}

/// Diagnostics are approved assets. Their absence is an implementation gap,
/// never a pending user decision. MCP logs additionally need the account/work
/// intersection scope; local cleanup receipts remain runtime data.
pub const DIAGNOSTIC_DEPENDENCIES: &[MissingPort] = &[
    MissingPort::FoundationRegistration,
    MissingPort::MachineAccountWorkScope,
    MissingPort::SourceAuthorizer,
    MissingPort::BusinessHandler,
];

pub fn schema_handlers() -> Vec<Arc<dyn KindHandler>> {
    Kind::ALL.into_iter().map(|kind| Arc::new(Handler(kind)) as Arc<dyn KindHandler>).collect()
}
pub fn mutation_authorizers(state: Arc<dyn AccountState>) -> Vec<Arc<dyn MutationAuthorizer>> {
    Kind::ALL
        .into_iter()
        .map(|kind| Arc::new(Authorizer::new(kind, state.clone())) as Arc<dyn MutationAuthorizer>)
        .collect()
}
