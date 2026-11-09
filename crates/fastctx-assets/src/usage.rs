//! Per-account financial shares and owner-only login quota are different facts.
//! Private conversation/turn links use separate conversation-scoped payloads.
use crate::{
    Error, json,
    source::{Binding, denied},
};
use fastctx_sync::{
    identity::{AccountId, DeviceId, Digest, ObjectKey, SourceId},
    store::ingest::SourceTarget,
};
use fastctx_types::{
    agents::LimitWindow,
    sync::scope::{ScopeDescriptor, ScopeSubject},
    usage::{KindsUsd, Tokens, UsageReset, UsageResetSource, Usd},
    usage_calls::{CallOutcome, CallPhase, CallSource, CallTiming},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Amount {
    pub model: String,
    pub usd: Option<Usd>,
    pub tokens: Tokens,
    pub kinds: Option<KindsUsd>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub identity: Digest,
    pub source: CallSource,
    pub agent: Option<String>,
    pub effort: Option<String>,
    pub timing: CallTiming,
    pub outcome: CallOutcome,
    pub phase: CallPhase,
    pub observed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub index: u64,
    pub count: u64,
}
impl Allocation {
    pub fn validate(self) -> Result<(), Error> {
        if self.count == 0 || self.index >= self.count {
            Err(json::invalid("invalid financial allocation slot"))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Usage {
    /// The corresponding financial share keeps the same call identity. This
    /// payload is routed only to the canonical conversation's own scope.
    ConversationLink {
        call: Digest,
        conversation: ObjectKey,
        turn: Option<String>,
    },
    Share {
        device: DeviceId,
        login: SourceId,
        account: AccountId,
        at: Timestamp,
        allocation: Allocation,
        amount: Amount,
        call: Option<Box<Call>>,
    },
    Quota {
        device: DeviceId,
        login: SourceId,
        owner: AccountId,
        at: Timestamp,
        /// The vendor identity fingerprint, never a World account name.
        vendor_account: Option<String>,
        windows: Vec<LimitWindow>,
        resets: Vec<UsageReset>,
    },
    LoginIdentity {
        device: DeviceId,
        login: SourceId,
        owner: AccountId,
        at: Timestamp,
        vendor_account: String,
    },
}
impl Usage {
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Share { allocation, amount, call, .. } => {
                allocation.validate()?;
                if call.as_ref().is_some_and(|call| {
                    call.source == CallSource::Background
                        && !call.observed
                        && (!amount.model.is_empty() || call.effort.is_some())
                }) {
                    return Err(json::invalid("unobserved background use cannot name a model or effort"));
                }
            }
            Self::Quota { at, windows, resets, .. } => {
                if resets.iter().any(|reset| !windows.iter().any(|window| valid_reset(reset, window, *at))) {
                    return Err(json::invalid("reset lacks its committed weekly window evidence"));
                }
            }
            Self::LoginIdentity { .. } | Self::ConversationLink { .. } => {}
        }
        Ok(())
    }
}

/// Shares are computed before deduplication. Repeated recipients retain every
/// original slot; missing account evidence remains None and cannot be published
/// as the current machine owner's account. The iterator allocates one item only.
pub fn split<'a>(
    who: Option<&'a AccountId>,
    with: &'a [Option<AccountId>],
    amount: &'a Amount,
) -> impl Iterator<Item = (Option<&'a AccountId>, Allocation, Amount)> + 'a {
    let count = with.len() as u64 + 1;
    std::iter::once(who).chain(with.iter().map(Option::as_ref)).enumerate().map(move |(index, account)| {
        let allocation = Allocation { index: index as u64, count };
        (account, allocation, share(amount, allocation))
    })
}
fn share(value: &Amount, allocation: Allocation) -> Amount {
    let part = |value: u64| value / allocation.count + u64::from(allocation.index < value % allocation.count);
    let usd = |value: Usd| Usd(part(value.0));
    Amount {
        model: value.model.clone(),
        usd: value.usd.map(usd),
        tokens: Tokens {
            input: part(value.tokens.input),
            cache_read: part(value.tokens.cache_read),
            cache_write: part(value.tokens.cache_write),
            output: part(value.tokens.output),
            reasoning: part(value.tokens.reasoning),
        },
        kinds: value.kinds.as_ref().map(|value| KindsUsd {
            input: usd(value.input),
            cache_read: usd(value.cache_read),
            cache_write: usd(value.cache_write),
            output: usd(value.output),
        }),
    }
}

/// Mirrors the old recorded-reset rule, without rerunning the reset detector.
pub fn valid_reset(reset: &UsageReset, window: &LimitWindow, at: Timestamp) -> bool {
    let Some(start) = reset.resets_at.checked_sub(jiff::SignedDuration::from_secs(604_800)).ok() else { return false };
    window.minutes == Some(10_080)
        && window.name == reset.window
        && window
            .resets_at
            .is_some_and(|end| (end.as_nanosecond() - reset.resets_at.as_nanosecond()).abs() <= 120_000_000_000)
        && if reset.source == UsageResetSource::Scheduled { reset.since == start } else { reset.since > start }
        && reset.since < reset.resets_at
        && reset.since <= reset.confirmed_at
        && reset.confirmed_at <= at
}

/// Independently looked up from the registered login and the original charge's
/// acting accounts. Current login ownership must not relabel historical shares.
pub struct LoginEvidence<'a> {
    pub binding: &'a Binding,
    pub device: &'a DeviceId,
    pub login: &'a SourceId,
    pub owner: &'a AccountId,
    pub charged_accounts: &'a BTreeSet<AccountId>,
    /// Computed from the committed ledger row, its exact allocation slot, or
    /// its recorded quota conclusion; not copied from the incoming payload.
    pub statement: &'a Usage,
}
pub fn authorize_source(
    value: &Usage,
    target: &SourceTarget,
    evidence: LoginEvidence<'_>,
    scope: &ScopeDescriptor,
) -> Result<(), Error> {
    value.validate()?;
    evidence.binding.check(target, "account.usage")?;
    evidence.binding.device(evidence.device)?;
    let (device, login, account) = match value {
        Usage::Share { device, login, account, .. } if evidence.charged_accounts.contains(account) => {
            (device, login, account)
        }
        Usage::Quota { device, login, owner, .. } | Usage::LoginIdentity { device, login, owner, .. }
            if owner == evidence.owner =>
        {
            (device, login, owner)
        }
        _ => return Err(denied("usage beneficiary or quota owner differs from the original login evidence")),
    };
    if value != evidence.statement
        || device != evidence.device
        || login != evidence.login
        || scope.scope != evidence.binding.object.scope
        || scope.acl_revision != evidence.binding.acl_revision
        || scope.subject != ScopeSubject::Account
        || scope.owner.account().map(|owner| &owner.id) != Some(account)
    {
        return Err(denied("usage must use its registered login device and exact account scope"));
    }
    Ok(())
}
