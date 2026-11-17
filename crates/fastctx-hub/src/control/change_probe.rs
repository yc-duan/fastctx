//! Opt-in local mutation probe with authorized calls already in flight.
use super::*;

pub(crate) fn seed(hub: &Hub) -> Vec<Standing> {
    let store = hub.store();
    let directory = store.indexed();
    let mut standings = Vec::new();
    for account in &directory.all.accounts {
        for from in &directory.all.members {
            for to in &directory.all.members {
                for purpose in [Purpose::Hands, Purpose::Agents, Purpose::Look, Purpose::Words, Purpose::Echo] {
                    if directory
                        .authorize(
                            &account.name,
                            fastctx_net::authorization::Action::Call { from: &from.name, to: &to.name, purpose },
                        )
                        .is_ok()
                    {
                        standings.push(Standing {
                            from: from.name.clone(),
                            from_key: from.key,
                            to_key: to.key,
                            account: account.name.clone(),
                            to: to.name.clone(),
                            purpose,
                            project_entry: None,
                            plan: None,
                        });
                    }
                }
            }
        }
    }
    let member = &directory.all.members[0];
    for (project_entry, plan) in [
        (Some(("codex-abcd".parse().expect("resident"), "probe".parse().expect("town"))), None),
        (None, Some("codex-abcd".parse().expect("resident"))),
    ] {
        standings.push(Standing {
            from: member.name.clone(),
            from_key: member.key,
            to_key: member.key,
            account: "a0".parse().expect("account"),
            to: member.name.clone(),
            purpose: Purpose::Hands,
            project_entry,
            plan,
        });
    }
    let selected: Vec<_> = standings
        .iter()
        .step_by((standings.len() / 32).max(1))
        .take(30)
        .chain(standings.iter().rev().take(2))
        .cloned()
        .collect();
    for (i, standing) in selected.iter().enumerate() {
        hub.calls.lock().expect("calls").insert(
            i as u64,
            Pending {
                caller: standing.from.clone(),
                caller_generation: 0,
                id: i as u64,
                target: standing.to.clone(),
                target_generation: 0,
                standing: standing.clone(),
                stream: None,
                at: tokio::time::Instant::now(),
                diagnostic: Arc::new(diagnostics::Call {
                    id: i as u64,
                    caller: standing.from.clone(),
                    target: standing.to.clone(),
                    account: standing.account.clone(),
                    kind: None,
                }),
            },
        );
    }
    selected
}

pub(crate) fn snapshot(hub: &Hub) -> Vec<u64> {
    let mut keys: Vec<_> = hub.calls.lock().expect("calls").iter().map(|(key, _)| *key).collect();
    keys.sort_unstable();
    keys
}
