use super::usage::Fixtures;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::usage::{Tokens, Usd};
use fastctx_types::usage_calls::*;
use fastctx_types::{Call, Reply};

pub fn fixtures() -> Fixtures {
    let at = "2026-10-20T12:00:00Z".parse().unwrap();
    let row = UsageCall {
        cursor: CallCursor { at, key: "example-request".into() },
        who: None,
        model: Some("claude-sonnet-5-5".into()),
        effort: Some("medium".into()),
        tokens: Some(Tokens { input: 100, output: 50, ..Default::default() }),
        usd: Some(Usd(35)),
        source: CallSource::Agent,
        agent: Some("worker".into()),
        timing: CallTiming { started_at: Some(at), first_at: Some(at), ended_at: at, approximate: false },
        outcome: CallOutcome::Completed,
        phase: CallPhase::Finished,
        conversation: Some("claude-abcd".parse().unwrap()),
        turn: Some("turn-one".into()),
    };
    let private = UsageCall { conversation: None, turn: None, ..row.clone() };
    let page =
        UsageCallsPage { rows: vec![row, private], next: Some(CallCursor { at, key: "next-page".into() }), total: Some(240) };
    let query = UsageCallsQuery {
        source: Some(CallSource::Agent),
        person: Some(CallPerson { account: None, with_machine: false }),
        from: Some(at),
        page: Some(2),
        ..Default::default()
    };
    let machine = "laptop".parse().unwrap();
    let agent: fastctx_types::AgentName = "claude".parse().unwrap();
    let dir = std::path::PathBuf::from("/home/me/.claude");
    Fixtures {
        calls: vec![Call::AgentCalls { machine, agent: agent.clone(), dir: dir.clone(), query: query.clone() }],
        replies: vec![Reply::AgentCalls(Box::new(page.clone()))],
        remote_calls: vec![RemoteCall::AgentCalls { agent, dir, query }],
        remote_replies: vec![RemoteReply::AgentCalls(Box::new(page))],
    }
}
