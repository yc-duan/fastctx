//! Opt-in observations of cached activity and its real projection.
use super::*;

pub(crate) fn seed(hub: &Hub) {
    let store = hub.store();
    for member in &store.book.directory.members {
        let state = hub.afoots.member(member.key, store.sequence);
        let id: fastctx_types::JobId = "j-abcdef".parse().expect("job id");
        let job = fastctx_types::town::AfootJob {
            job: fastctx_types::town::RunningJob {
                source: None,
                job: id.clone(),
                town: None,
                machine: member.name.clone(),
                owner: fastctx_types::run::JobOwner::Resident(fastctx_types::Address {
                    machine: member.name.clone(),
                    resident: "codex-abcd".parse().expect("resident"),
                }),
                started_at: member.added,
                pid: None,
                command: "probe".into(),
                output: "probe.log".into(),
                stderr: None,
                account: None,
            },
            accounts: member.logins.iter().cloned().collect(),
        };
        Arc::make_mut(&mut state.lock().told).jobs.insert(id, job);
    }
}

#[derive(Default)]
pub(crate) struct Projection(HashMap<MachineName, Shown>);
impl Projection {
    pub(crate) fn refresh(&mut self, hub: &Hub) -> anyhow::Result<()> {
        anyhow::ensure!(hub.afoots.publishing.probe_take_all(), "activity projection requested");
        tell(hub, &mut self.0, Scope::All);
        Ok(())
    }
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let entries: std::collections::BTreeMap<_, _> = self.0.iter().map(|(reader, shown)| {
            let machines: std::collections::BTreeMap<_, _> = shown.machines.iter().map(|(machine, seen)| {
                (machine, serde_json::json!({"residents": seen.afoot.residents, "jobs": seen.afoot.jobs, "busy": seen.busy}))
            }).collect();
            (reader, serde_json::json!(machines))
        }).collect();
        serde_json::json!(entries)
    }
}

pub(crate) fn snapshot(hub: &Hub) -> serde_json::Value {
    let store = hub.store();
    let entries: Vec<_> = store
        .book
        .directory
        .members
        .iter()
        .map(|member| {
            serde_json::json!([
                member.key,
                hub.afoots
                    .get(&member.key)
                    .map(|snapshot| { serde_json::json!({"residents": snapshot.residents, "jobs": snapshot.jobs}) })
            ])
        })
        .collect();
    serde_json::json!({"entries": entries, "publish_all": hub.afoots.publishing.probe_all()})
}
