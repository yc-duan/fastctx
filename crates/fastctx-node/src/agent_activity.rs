//! What one of the host's own agents in a resident's conversation did, as
//! its host keeps it (`world-gui.md`, 对话): a conversation's history tells
//! that an agent started and how it ended, and the agent's own record is
//! read here, on the machine its conversation lives on, when the user opens
//! it. Claude Code keeps each subagent's record beside its session's;
//! Codex keeps each agent it spawned as a thread of its own.

use fastctx_types::access::Written;
use std::sync::Arc;

use fastctx_hosts::claude::{subagents, transcript};
use fastctx_types::ipc::Failure;
use fastctx_types::session::{AgentActivity, Happened, HostAgentStart, SessionEvent};
use fastctx_types::{Address, ResidentId};

use crate::Node;

/// How many agents deep a Codex agent may be spawned under the
/// conversation's own thread and still be one of its own: Codex refuses a
/// spawn deeper than its `agent_max_depth`, 1 by default; this bounds the
/// walk up through spawners a reply names, not what Codex allows.
const DEEPEST: usize = 16;

/// What the agent `agent` of the resident's conversation did, on whichever
/// machine the resident lives.
pub(crate) async fn read(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
    agent: String,
) -> Result<(Address, AgentActivity), Failure> {
    let local = machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.about(&resident).is_some() => node.machine().into(),
        None => crate::colleagues::find_for(node, &fastctx_types::access::Acting::Here, &resident).await?,
    };
    if local || node.is_here(&machine) {
        let activity = here(node, &resident, &agent).await?;
        return Ok((Address { machine: machine.name, resident }, activity));
    }
    let call = fastctx_types::remote::RemoteCall::AgentActivity { resident: resident.clone(), agent };
    let written = &machine;
    match node
        .call(&fastctx_types::access::Acting::Here, written, fastctx_types::access::Purpose::Agents, call)
        .await
        .map_err(|failure| Failure::new(failure.to_string()))?
    {
        Ok(fastctx_types::remote::RemoteReply::AgentActivity { activity }) => {
            Ok((Address { machine: machine.name, resident }, *activity))
        }
        Ok(_) => Err(Failure::new(format!("{machine} answered something other than an agent's record"))),
        Err(failure) => Err(failure),
    }
}

/// What the agent `agent` of a resident of this machine did.
pub(crate) async fn here(node: &Arc<Node>, resident: &ResidentId, agent: &str) -> Result<AgentActivity, Failure> {
    let (about, _) = node
        .residents
        .about(resident)
        .ok_or_else(|| Failure::new(format!("no resident {resident} lives on {}", node.machine())))?;
    match about.host.agent.as_str() {
        "claude" => claude(node, resident, agent).await,
        "codex" => {
            let thread =
                about.session.clone().ok_or_else(|| Failure::new(format!("{resident} has no Codex thread yet")))?;
            let codex = node.codex.of(node, &about)?;
            let owner = Address { machine: node.machine(), resident: resident.clone() };
            codex_agent(&codex, &thread, agent, &owner).await
        }
        other => Err(Failure::new(format!("{other} keeps no agents of its own the World can read"))),
    }
}

async fn claude(node: &Arc<Node>, resident: &ResidentId, agent: &str) -> Result<AgentActivity, Failure> {
    if !subagents::valid_id(agent) {
        return Err(Failure::new(format!("{agent:?} is not the id of one of Claude Code's agents")));
    }
    let Some(path) = crate::claude::history::agent_record(node, resident, agent) else {
        return Ok(AgentActivity {
            unavailable: Some("Claude Code keeps no record of this agent: it removes old sessions itself".into()),
            ..Default::default()
        });
    };
    // Claude Code adds to the record as the agent goes; a read it overtook
    // is read again, until one sees it whole.
    let read = loop {
        let (reading, id) = (path.clone(), agent.to_string());
        let read = tokio::task::spawn_blocking(move || transcript::read_agent(&reading, &id))
            .await
            .map_err(|error| Failure::new(format!("reading the agent's record: {error}")))?;
        match read {
            Ok(read) => break read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(Failure::new(format!("{}: {error}", path.display()))),
        }
    };
    let (call, meta) = subagents::meta(&path, agent).map_or((None, None), |(call, meta)| (Some(call), Some(meta)));
    let start = HostAgentStart {
        id: agent.to_string(),
        call,
        kind: meta.as_ref().and_then(|meta| meta.kind.clone()),
        task: meta.as_ref().and_then(|meta| meta.task.clone()),
        background: meta.is_some_and(|meta| meta.background),
        ..Default::default()
    };
    let unavailable = (!read.problems.is_empty()).then(|| {
        format!("{} part(s) of its record could not be read: {}", read.problems.len(), read.problems.join("; "))
    });
    Ok(AgentActivity { agent: Some(start), events: runs(read.events, agent), unavailable })
}

/// An agent's record has no turns of its own: each time it is asked
/// something begins a run of it, a turn of its own conversation, which
/// ends with what it handed back, as the record of its spawner tells.
fn runs(events: Vec<SessionEvent>, agent: &str) -> Vec<SessionEvent> {
    let mut placed = Vec::with_capacity(events.len() + 1);
    let mut turn: Option<String> = None;
    let mut count = 0;
    for mut event in events {
        if turn.is_none() || matches!(event.happened, Happened::Heard { .. }) {
            count += 1;
            let begun = format!("{agent}:{count}");
            placed.push(SessionEvent { at: event.at, happened: Happened::TurnStarted { turn: begun.clone() } });
            turn = Some(begun);
        }
        let current = turn.clone().expect("a run began");
        match &mut event.happened {
            Happened::Heard { turn: slot, .. }
            | Happened::Health { turn: slot, .. }
            | Happened::Changed { turn: slot, .. } => *slot = Some(current),
            Happened::Began { turn: slot, .. }
            | Happened::Grew { turn: slot, .. }
            | Happened::Finished { turn: slot, .. }
            | Happened::Updated { turn: slot, .. }
            | Happened::Steps { turn: slot, .. } => *slot = current,
            _ => {}
        }
        placed.push(event);
    }
    placed
}

/// What a thread Codex spawned for the conversation's thread `thread` did:
/// its own history, once it is known to be one of the conversation's own,
/// spawned by its thread or by an agent of it.
async fn codex_agent(
    codex: &Arc<fastctx_hosts::codex::CodexHost>,
    thread: &str,
    agent: &str,
    owner: &Address,
) -> Result<AgentActivity, Failure> {
    let failed = |error: String| Failure::new(format!("asking Codex about its agent {agent}: {error}"));
    let spawned = codex.spawned_by(agent).await.map_err(failed)?;
    let Some(spawned) = spawned else {
        return Err(Failure::new(format!("{agent} is not an agent of this conversation")));
    };
    let mut parent = spawned.parent.clone();
    let mut depth = 1;
    while parent != thread {
        depth += 1;
        let above = codex.spawned_by(&parent).await.map_err(failed)?;
        match above {
            Some(above) if depth <= DEEPEST => parent = above.parent,
            _ => return Err(Failure::new(format!("{agent} is not an agent of this conversation"))),
        }
    }
    let events = codex.history(agent, owner).await.map_err(failed)?;
    Ok(AgentActivity {
        agent: Some(HostAgentStart {
            id: agent.to_string(),
            parent: (spawned.parent != thread).then(|| spawned.parent.clone()),
            kind: spawned.role,
            name: spawned.nickname,
            model: spawned.model,
            background: true,
            ..Default::default()
        }),
        events,
        unavailable: None,
    })
}
