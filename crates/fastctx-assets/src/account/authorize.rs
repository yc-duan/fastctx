//! Field capabilities are derived from verified payloads, independently of IPC names.
use super::{Kind, collaboration, profile, settings};
use crate::context::AccountState;
use fastctx_sync::store::{
    access::{AccessContext, Decision, Principal},
    authorize::{MutationAuthorizer, PreparedChange},
    mutation::OriginalIntent,
};
use serde_json::Value;
use std::sync::Arc;

pub struct Authorizer {
    kind: Kind,
    state: Arc<dyn AccountState>,
}
impl Authorizer {
    pub fn new(kind: Kind, state: Arc<dyn AccountState>) -> Self {
        Self { kind, state }
    }
    fn check(&self, context: &AccessContext, change: &PreparedChange<'_>) -> bool {
        if change.proposal.kind != self.kind.id() || change.proposal.key.scope != context.scope {
            return false;
        }
        // Directory facts are never ordinary user or agent mutations.
        if self.kind == Kind::Directory {
            return false;
        }
        if matches!(
            change.proposal.original,
            OriginalIntent::Delete
                | OriginalIntent::Remove
                | OriginalIntent::Directory { .. }
                | OriginalIntent::Edit { .. }
        ) {
            return false;
        }
        let Some(access) = self.state.current(context, &change.proposal.key) else { return false };
        if !access.permits(context, &change.proposal.key) {
            return false;
        }
        let user = matches!(context.principal, Principal::Account { .. });
        if self.kind != Kind::Profile && !user {
            return false;
        }
        // A real user is necessary but cannot replace the missing prepared
        // directory proof and atomic name/index publication for a skill.
        if self.kind == Kind::Skill {
            return false;
        }
        match self.kind {
            Kind::Profile => {
                let Ok(after) = profile::decode(change.after) else { return false };
                let before = if change.before.is_empty() {
                    None
                } else {
                    let Ok(value) = profile::decode(change.before) else { return false };
                    Some(value)
                };
                if !change.fields.iter().all(|f| profile_field(&f.path, user, access.phone)) {
                    return false;
                }
                profile::command_targets(before.as_ref(), &after).iter().all(|machine| access.hands.contains(machine))
            }
            Kind::Collaboration => {
                if !user || collaboration::decode(change.after).is_err() {
                    return false;
                }
                let Ok(after) = collaboration::semantic(change.after) else { return false };
                change.fields.iter().all(|f| collaboration_field(&f.path, &after))
                    && collaboration::caller_extensions_preserved(change.before, change.after)
            }
            Kind::Settings => {
                if !user {
                    return false;
                }
                let Ok(after) = settings::decode(change.after) else { return false };
                if !change.before.is_empty() {
                    let Ok(before) = settings::decode(change.before) else { return false };
                    if std::mem::discriminant(&before) != std::mem::discriminant(&after) {
                        return false;
                    }
                }
                match after {
                    settings::Settings::History { .. } => true,
                    settings::Settings::Sentinel { value } => {
                        value.home.as_ref().is_none_or(|home| access.hands.contains(home))
                    }
                }
            }
            Kind::Directory | Kind::Skill => false,
        }
    }
}
impl MutationAuthorizer for Authorizer {
    fn kind(&self) -> &'static str {
        self.kind.id()
    }
    fn authorize(&self, context: &AccessContext, change: PreparedChange<'_>) -> Decision {
        if self.check(context, &change) {
            Decision::Allowed { revision: context.observed_acl }
        } else {
            Decision::Denied {
                revision: context.observed_acl,
                reason: "the current actor or publication port cannot make this asset change".into(),
            }
        }
    }
}
fn profile_field(path: &[String], user: bool, phone: bool) -> bool {
    let Some(first) = path.first().map(String::as_str) else { return false };
    match first {
        "memory" => user && path.len() == 1,
        "layer" => user && !phone && path.len() == 1,
        "prompt" => path.len() == 1,
        "prompts" => path.len() == 1 || path.len() == 2 && matches!(path[1].as_str(), "claude" | "codex"),
        "claude" => path.len() == 1 || fastctx_types::profile::CLAUDE_SETTINGS.contains(&path[1].as_str()),
        "codex" => path.len() == 1 || fastctx_types::profile::CODEX_SETTINGS.contains(&path[1].as_str()),
        "subagents" => path.len() == 1 || matches!(path[1].as_str(), "agents" | "instructions"),
        "servers" => true,
        _ => false,
    }
}
fn collaboration_field(path: &[String], after: &Value) -> bool {
    let Some(first) = path.first().map(String::as_str) else { return false };
    match first {
        "on" | "prompt" | "rules" => path.len() == 1,
        "callees" | "callers" => {
            if path.len() <= 2 {
                return true;
            }
            if after.get(first).and_then(|v| v.get(&path[1])).is_none() {
                return true;
            }
            if first == "callers" {
                return matches!(path[2].as_str(), "machine" | "agent" | "home");
            }
            match path[2].as_str() {
                "name" | "use_when" | "model" | "effort" | "screen" | "at_once" => true,
                "login" => {
                    path.len() == 3 || path.len() == 4 && matches!(path[3].as_str(), "machine" | "agent" | "home")
                }
                _ => false,
            }
        }
        _ => false,
    }
}
