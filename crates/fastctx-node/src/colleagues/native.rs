//! Build native-agent trees from existing host facts and child histories.

use crate::Node;
use fastctx_types::{
    ResidentId,
    collaboration::NativeAgent,
    session::{Happened, HostAgentStart, SessionChange, SessionEvent},
};
use jiff::Timestamp;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
use tokio::time::Instant;

#[derive(Default)]
struct Kept {
    agent: NativeAgent,
    parent: Option<String>,
    observed: Option<Timestamp>,
}
#[derive(Default)]
pub(super) struct Fold {
    agents: BTreeMap<String, Kept>,
    calls: BTreeMap<(Option<String>, String), Timestamp>,
}
impl Fold {
    fn entry(&mut self, id: &str) -> &mut Kept {
        self.agents.entry(id.into()).or_insert_with(|| Kept {
            agent: NativeAgent { id: id.into(), ..Default::default() },
            ..Default::default()
        })
    }
    fn start(&mut self, start: &HostAgentStart, parent: Option<&str>, at: Timestamp) {
        let began = start
            .call
            .as_ref()
            .and_then(|call| self.calls.get(&(parent.map(str::to_string), call.clone())).copied())
            .or_else(|| start.live.then_some(at));
        let kept = self.entry(&start.id);
        if let Some(parent) = start.parent.as_deref().or(parent) {
            kept.parent = Some(parent.into());
        }
        if start.name.is_some() {
            kept.agent.name = start.name.clone();
        }
        if start.task.is_some() {
            kept.agent.task = start.task.clone();
        }
        if start.model.is_some() {
            kept.agent.model = start.model.clone();
        }
        if let Some(began) = began {
            kept.agent.started_at = Some(kept.agent.started_at.map_or(began, |old| old.min(began)));
        }
        if start.live && kept.observed.is_none_or(|old| old <= at) {
            kept.agent.state = Some("running".into());
            kept.observed = Some(at);
        }
    }
    pub(super) fn events(&mut self, events: &[SessionEvent], parent: Option<&str>) {
        for event in events {
            self.event(&event.happened, event.at, parent, false);
        }
    }
    fn event(&mut self, event: &Happened, at: Timestamp, parent: Option<&str>, live: bool) {
        match event {
            Happened::Began { item, .. } | Happened::Finished { item, .. } | Happened::Updated { item, .. } => {
                if let Some(started) = item.started_at {
                    self.calls.insert((parent.map(str::to_string), item.id.clone()), started);
                }
            }
            Happened::TurnStarted { .. } if live && parent.is_some() => {
                let kept = self.entry(parent.expect("matched"));
                if kept.observed.is_none_or(|old| old <= at) {
                    kept.agent.state = Some("running".into());
                    kept.observed = Some(at);
                }
            }
            Happened::TurnEnded { end, .. } if parent.is_some() => {
                let kept = self.entry(parent.expect("matched"));
                if kept.observed.is_none_or(|old| old <= at) {
                    kept.agent.state = Some(end.how.to_string());
                    kept.agent.ended_at = Some(at);
                    kept.agent.duration_ms = end.duration_ms;
                    kept.observed = Some(at);
                } else if kept.agent.state.as_deref() == Some(end.how.to_string().as_str()) {
                    kept.agent.ended_at = Some(kept.agent.ended_at.map_or(at, |old| old.max(at)));
                }
            }
            Happened::Changed { change: SessionChange::AgentStarted { agent }, .. } => self.start(agent, parent, at),
            Happened::Changed { change: SessionChange::AgentEvent { agent_id, happened }, .. } => {
                let kept = self.entry(agent_id);
                if kept.parent.is_none() {
                    kept.parent = parent.map(str::to_string);
                }
                self.event(happened, at, Some(agent_id), live);
            }
            Happened::Changed { change: SessionChange::AgentEnded { agent_id, end }, .. } => {
                let kept = self.entry(agent_id);
                if kept.parent.is_none() {
                    kept.parent = parent.map(str::to_string);
                }
                if kept.agent.task.is_none() {
                    kept.agent.task = end.task.clone();
                }
                if kept.observed.is_none_or(|old| old <= at) {
                    kept.agent.state = Some(end.how.to_string());
                    kept.agent.duration_ms = end.duration_ms;
                    kept.observed = Some(at);
                }
            }
            Happened::Changed { change: SessionChange::Agents { agents }, .. } => {
                for agent in agents {
                    let kept = self.entry(&agent.id);
                    kept.parent = agent.parent_id.clone().or_else(|| parent.map(str::to_string));
                    if agent.name.is_some() {
                        kept.agent.name = agent.name.clone();
                    }
                    if !agent.description.is_empty() {
                        kept.agent.task = Some(agent.description.clone());
                    }
                    if kept.observed.is_none_or(|old| old <= at) {
                        kept.agent.state = Some(agent.status.clone());
                        kept.observed = Some(at);
                    }
                }
            }
            _ => {}
        }
    }
    pub(super) fn finish(mut self) -> Vec<NativeAgent> {
        fn take(id: String, agents: &mut BTreeMap<String, Kept>) -> NativeAgent {
            let mut kept = agents.remove(&id).expect("known agent");
            let children: Vec<_> = agents
                .iter()
                .filter(|(_, child)| child.parent.as_ref() == Some(&id))
                .map(|(id, _)| id.clone())
                .collect();
            for child in children {
                if agents.contains_key(&child) {
                    kept.agent.children.push(take(child, agents));
                }
            }
            kept.agent
        }
        let roots: Vec<_> = self
            .agents
            .iter()
            .filter(|(_, kept)| kept.parent.as_ref().is_none_or(|parent| !self.agents.contains_key(parent)))
            .map(|(id, _)| id.clone())
            .collect();
        let mut result = Vec::new();
        for root in roots {
            if self.agents.contains_key(&root) {
                result.push(take(root, &mut self.agents));
            }
        }
        // Removing before following children makes even cyclic host metadata finite.
        while let Some(id) = self.agents.keys().next().cloned() {
            let mut agent = take(id, &mut self.agents);
            agent.unavailable = Some(fastctx_types::collab_prompt::NATIVE_CYCLE.into());
            result.push(agent);
        }
        result
    }
}

pub(super) async fn read(node: &Arc<Node>, id: &ResidentId, deadline: Instant) -> (Vec<NativeAgent>, Vec<String>) {
    let mut fold = Fold::default();
    let mut unavailable = Vec::new();
    match tokio::time::timeout_at(deadline, crate::talk::history_here(node, id)).await {
        Ok(Ok(mut history)) => {
            fold.events(&history.history, None);
            while let Ok(event) = history.events.try_recv() {
                fold.event(&event.happened, event.at, None, true);
            }
        }
        Ok(Err(error)) => return (Vec::new(), vec![error.message]),
        Err(_) => return (Vec::new(), vec![fastctx_types::collab_prompt::NATIVE_DEADLINE.into()]),
    }
    let mut read = BTreeSet::new();
    let mut queue: VecDeque<_> = fold.agents.keys().cloned().collect();
    let mut scheduled: BTreeSet<_> = fold.agents.keys().cloned().collect();
    while let Some(agent) = queue.pop_front() {
        if !read.insert(agent.clone()) {
            continue;
        }
        match tokio::time::timeout_at(deadline, crate::agent_activity::here(node, id, &agent)).await {
            Ok(Ok(activity)) => {
                if let Some(start) = activity.agent {
                    let parent = fold.agents.get(&agent).and_then(|kept| kept.parent.clone());
                    fold.start(
                        &start,
                        parent.as_deref(),
                        activity.events.first().map_or_else(Timestamp::now, |event| event.at),
                    );
                }
                fold.events(&activity.events, Some(&agent));
                if let Some(error) = activity.unavailable {
                    fold.entry(&agent).agent.unavailable = Some(error);
                }
                for id in fold.agents.keys() {
                    if scheduled.insert(id.clone()) {
                        queue.push_back(id.clone());
                    }
                }
            }
            Ok(Err(error)) => fold.entry(&agent).agent.unavailable = Some(error.message),
            Err(_) => {
                unavailable.push(fastctx_types::collab_prompt::NATIVE_DEADLINE.into());
                for (id, kept) in &mut fold.agents {
                    if !read.contains(id) || *id == agent {
                        kept.agent.unavailable = Some(fastctx_types::collab_prompt::NATIVE_HISTORY_DEADLINE.into());
                    }
                }
                break;
            }
        }
    }
    (fold.finish(), unavailable)
}
