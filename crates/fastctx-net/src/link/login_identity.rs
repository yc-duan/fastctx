//! Preserve one login identity across default and explicit directory spellings.

use std::path::{Path, PathBuf};

use fastctx_types::access::Purpose;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AccountName, AgentName, MachineName};

use super::Link;

fn spelling(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.as_bytes().get(1) == Some(&b':') || text.starts_with('\\') {
        text.replace('\\', "/").to_lowercase().trim_end_matches('/').to_string()
    } else {
        text.trim_end_matches('/').to_string()
    }
}

pub(super) fn canonical(home: Option<&Path>, homes: &[(PathBuf, bool)]) -> Option<PathBuf> {
    let home = home?;
    match homes.iter().find(|(dir, _)| spelling(dir) == spelling(home)) {
        Some((_, true)) => None,
        Some((dir, false)) => Some(dir.clone()),
        None => Some(home.to_path_buf()),
    }
}

impl Link {
    pub(super) async fn login_homes(
        &self,
        machine: &MachineName,
        account: &AccountName,
        agent: &AgentName,
    ) -> Result<Vec<(PathBuf, bool)>, String> {
        if self.view().is_me(machine) {
            return self.inner.node.login_homes(agent);
        }
        let reply = Box::pin(self.inner.call(machine, account, Purpose::Look, RemoteCall::AgentLogins, None))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.message)?;
        let RemoteReply::AgentLogins(logins) = reply else {
            return Err(format!("upgrade {machine} to identify its agent logins"));
        };
        let logins = logins.of(agent.as_str()).ok_or_else(|| format!("unknown agent {agent}"))?;
        Ok(logins.listed.iter().chain(&logins.candidates).map(|login| (login.dir.clone(), login.default)).collect())
    }

    pub(super) async fn login_call(
        &self,
        to: &MachineName,
        account: &AccountName,
        mut call: RemoteCall,
    ) -> Result<RemoteCall, String> {
        let (agent, home) = match &mut call {
            RemoteCall::Open { agent, home, .. } => (agent, home),
            RemoteCall::Ask { started, .. } => (&mut started.callee.login.agent, &mut started.callee.login.home),
            _ => return Ok(call),
        };
        let view = self.view();
        let whole = view.rights(account, to).hands;
        if home.is_none() && (whole || view.assisting(account, to).any(|entry| entry.is(agent, None))) {
            return Ok(call);
        }
        let homes = self.login_homes(to, account, agent).await?;
        let identity = canonical(home.as_deref(), &homes);
        // Older hubs compare the original spelling. Use the matching grant's
        // spelling on the wire; the execution node records the canonical one.
        *home = if whole {
            identity
        } else {
            view.assisting(account, to)
                .find(|entry| entry.agent == *agent && canonical(entry.home.as_deref(), &homes) == identity)
                .map(|entry| entry.home.clone())
                .unwrap_or(identity)
        };
        Ok(call)
    }
}
