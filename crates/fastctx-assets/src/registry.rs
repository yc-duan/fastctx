//! The intake list describes readiness; it is not an activation gate by itself.
//! Service composition must use [`crate::installation::ready`] and install each
//! returned kind's reader policy and mutation authorizer with its handler before
//! advertising it. A standalone schema handler never authorizes object reads.
use fastctx_sync::registry::{DataClass, DecisionState, Lightweight, REGISTRY, Registration};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Unavailable,
    SchemaOnly,
    Ready,
}
impl Intake {
    /// Service readiness also requires an approved persistence registration.
    /// Advertising additionally requires installation of all returned ports.
    pub fn readiness(&self) -> Readiness {
        if !self.schema_available {
            Readiness::Unavailable
        } else if self.registration.decision != DecisionState::Confirmed || !self.missing.is_empty() {
            Readiness::SchemaOnly
        } else {
            Readiness::Ready
        }
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
                        MissingPort::KindReaderPolicy,
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
                        MissingPort::KindReaderPolicy,
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
                    "fastctx-assets::diagnostics::alert",
                    true,
                    &[
                        MissingPort::FoundationRegistration,
                        MissingPort::SourceAuthorizer,
                        MissingPort::KindReaderPolicy,
                        MissingPort::ProjectionContext,
                    ],
                ),
                "account.usage" | "account.activity" | "machine.audit" | "machine.connection"
                | "device.preferences" => (
                    "fastctx-assets::records",
                    true,
                    &[
                        MissingPort::SourceAuthorizer,
                        MissingPort::KindReaderPolicy,
                        MissingPort::ProjectionContext,
                        MissingPort::FoundationRegistration,
                    ],
                ),
                "account.read-position" | "machine.settings" => (
                    "fastctx-assets::records",
                    true,
                    &[
                        MissingPort::InternalServiceSubmission,
                        MissingPort::KindReaderPolicy,
                        MissingPort::ProjectionContext,
                        MissingPort::LegacyNodeDelegation,
                    ],
                ),
                "conversation.agents" | "job.facts" => {
                    ("account and machine records", false, &[MissingPort::BusinessHandler])
                }
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
