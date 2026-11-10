//! Build native-agent trees from existing host facts and child histories.

mod history;

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

#[derive(Clone, Default)]
struct Kept {
    agent: NativeAgent,
    parent: Option<String>,
    observed: Option<Timestamp>,
}

impl Kept {
    fn ended(&self) -> bool {
        matches!(self.agent.state.as_deref(), Some("completed" | "failed" | "interrupted" | "killed" | "stopped"))
    }
}
/// How many of the latest calls a followed conversation keeps the start of:
/// an agent's start is told with the call that started it, so only a recent
/// call is still looked for once the history has been read.
const RECENT_CALLS: usize = 256;

type Call = (Option<String>, String);

#[derive(Default)]
pub(super) struct Fold {
    agents: BTreeMap<String, Kept>,
    /// When each call began, by the agent whose loop made it.
    calls: BTreeMap<Call, Timestamp>,
    /// The calls in the order they were first seen.
    seen: VecDeque<Call>,
    /// Whether only the recent calls are kept, as when the conversation is
    /// followed after its history was read.
    recent: bool,
}
impl Fold {
    fn entry(&mut self, id: &str) -> &mut Kept {
        self.agents.entry(id.into()).or_insert_with(|| Kept {
            agent: NativeAgent { id: id.into(), reading: true, ..Default::default() },
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
    pub(super) fn event(&mut self, event: &Happened, at: Timestamp, parent: Option<&str>, live: bool) {
        match event {
            Happened::Began { item, .. } | Happened::Finished { item, .. } | Happened::Updated { item, .. } => {
                if let Some(started) = item.started_at {
                    self.call((parent.map(str::to_string), item.id.clone()), started);
                }
            }
            Happened::TurnStarted { .. } if parent.is_some() => {
                let kept = self.entry(parent.expect("matched"));
                if kept.observed.is_none_or(|old| old <= at) {
                    // A historical start does not prove it still runs, but
                    // does supersede the completion of an earlier turn.
                    kept.agent.state = live.then(|| "running".into());
                    kept.agent.ended_at = None;
                    kept.agent.duration_ms = None;
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
    fn call(&mut self, call: Call, started: Timestamp) {
        if self.calls.insert(call.clone(), started).is_none() {
            self.seen.push_back(call);
        }
        if self.recent {
            self.forget_calls();
        }
    }
    fn forget_calls(&mut self) {
        while self.calls.len() > RECENT_CALLS {
            let Some(oldest) = self.seen.pop_front() else { break };
            self.calls.remove(&oldest);
        }
    }
    /// Keeps only the recent calls from now on.
    pub(super) fn recent_only(&mut self) {
        self.recent = true;
        self.forget_calls();
    }
    /// The agents as they stand, the fold left to take more.
    pub(super) fn agents(&self) -> Vec<NativeAgent> {
        Fold { agents: self.agents.clone(), ..Default::default() }.finish()
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

pub(super) async fn read(
    node: &Arc<Node>,
    id: &ResidentId,
    deadline: Instant,
) -> (Vec<NativeAgent>, Vec<String>, bool) {
    let read = open(node, id, deadline, Read::default(), |_, _, _| {}).await;
    let reading = read.reading();
    (read.fold.finish(), read.unavailable, reading)
}

/// The folded history, its live stream, and which agents still need a read.
#[derive(Default)]
pub(super) struct Read {
    pub(super) fold: Fold,
    pub(super) unavailable: Vec<String>,
    pub(super) events: Option<crate::watch::Events>,
    pub(super) goal_changed: bool,
    history: history::History,
    history_seen: bool,
}

impl Read {
    pub(super) fn reading(&self) -> bool {
        self.history.incomplete() || self.agents_reading()
    }

    pub(super) fn agents_reading(&self) -> bool {
        self.fold.agents.values().any(|kept| kept.agent.reading)
    }

    pub(super) fn history_again(&self) -> Option<Instant> {
        self.history.again()
    }

    pub(super) fn lost(&mut self) {
        self.events = None;
        self.history.lost();
    }

    pub(super) fn event(&mut self, event: &SessionEvent) {
        self.fold.event(&event.happened, event.at, None, true);
        self.goal_changed |= crate::goals::watch::is_goal(event);
    }

    fn drain(&mut self) {
        while let Some(events) = &mut self.events {
            match events.try_recv() {
                Ok(event) => self.event(&event),
                Err(fastctx_net::watch_queue::TryRecvError::Empty) => break,
                Err(_) => {
                    self.lost();
                    break;
                }
            }
        }
    }
}

pub(super) async fn open(
    node: &Arc<Node>,
    id: &ResidentId,
    deadline: Instant,
    mut read: Read,
    mut progress: impl FnMut(&Fold, &[String], bool),
) -> Read {
    progress(&read.fold, &read.unavailable, true);
    read.drain();
    // Catching up the parent history has its own backoff. Its old stream
    // continues delivering events until a complete replacement is ready.
    if read.history.due() {
        read.fold.recent = false;
        let history = tokio::time::timeout_at(deadline, crate::talk::history_quietly(node, id));
        tokio::pin!(history);
        let result = loop {
            tokio::select! {
                result = &mut history => break result,
                event = async { read.events.as_mut().expect("stream present").recv().await }, if read.events.is_some() => {
                    match event {
                        Ok(Some(event)) => read.event(&event),
                        Ok(None) | Err(_) => read.lost(),
                    }
                }
            }
        };
        match result {
            Ok(Ok(history)) => {
                let reconnecting = read.events.is_none();
                read.fold = folded(read.fold, history.history, None).await;
                read.unavailable = history.unavailable;
                read.goal_changed |= reconnecting && read.history_seen;
                read.history_seen = true;
                if reconnecting || !history.unread {
                    read.events = Some(history.events);
                }
                read.drain();
                if history.unsettled || read.events.is_none() {
                    read.history.retry();
                } else {
                    read.history.settled();
                }
                if reconnecting {
                    for kept in read.fold.agents.values_mut() {
                        if !kept.ended() {
                            kept.agent.reading = true;
                        }
                    }
                }
            }
            Ok(Err(error)) => {
                read.unavailable = vec![error.message];
                read.history.settled();
            }
            Err(_) => read.history.retry(),
        }
    }
    progress(&read.fold, &read.unavailable, read.reading());
    let mut queue: VecDeque<_> =
        read.fold.agents.iter().filter(|(_, kept)| kept.agent.reading).map(|(id, _)| id.clone()).collect();
    let mut scheduled: BTreeSet<_> = read.fold.agents.keys().cloned().collect();
    while let Some(agent) = queue.pop_front() {
        if Instant::now() >= deadline {
            break;
        }
        let activity = tokio::time::timeout_at(deadline, crate::agent_activity::here(node, id, &agent));
        tokio::pin!(activity);
        // Keep consuming live events while a child history is slow, so a
        // backlog of child reads does not itself lose the parent's stream.
        let result = loop {
            tokio::select! {
                result = &mut activity => break result,
                event = async { read.events.as_mut().expect("stream present").recv().await }, if read.events.is_some() => {
                    match event {
                        Ok(Some(event)) => read.event(&event),
                        Ok(None) | Err(_) => read.lost(),
                    }
                }
            }
        };
        match result {
            Ok(Ok(activity)) => {
                if let Some(start) = activity.agent {
                    let parent = read.fold.agents.get(&agent).and_then(|kept| kept.parent.clone());
                    read.fold.start(
                        &start,
                        parent.as_deref(),
                        activity.events.first().map_or_else(Timestamp::now, |event| event.at),
                    );
                }
                read.fold = folded(read.fold, activity.events, Some(agent.clone())).await;
                let kept = read.fold.entry(&agent);
                kept.agent.unavailable = activity.unavailable;
                kept.agent.reading = false;
            }
            Ok(Err(error)) => {
                let kept = read.fold.entry(&agent);
                kept.agent.reading = !error.is_refused() && !error.is_deleted();
                kept.agent.unavailable = Some(error.message);
            }
            Err(_) => break,
        }
        for (id, kept) in &read.fold.agents {
            if kept.agent.reading && scheduled.insert(id.clone()) {
                queue.push_back(id.clone());
            }
        }
        progress(&read.fold, &read.unavailable, read.reading());
    }
    if !read.reading() {
        read.fold.recent_only();
    }
    read
}

/// A whole history is folded on a thread that may block.
async fn folded(mut fold: Fold, events: Vec<SessionEvent>, parent: Option<String>) -> Fold {
    fastctx_platform::task::blocking(move || {
        fold.events(&events, parent.as_deref());
        fold
    })
    .await
}
