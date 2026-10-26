//! Frozen app-server 0.160.0 contracts. The session fixture was captured from
//! the unmodified official binary; only paths and opaque IDs are normalized.

use super::{CodexHost, HostEvent, history, items, millis};
use fastctx_types::session::{Content, Does, Happened, SessionChange, ToolStatus, What};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

fn host() -> (Arc<CodexHost>, tokio::sync::mpsc::UnboundedReceiver<HostEvent>) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "codex-contract-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    CodexHost::new(PathBuf::new(), None, root, Vec::new(), Arc::new(|_| {}))
}

#[test]
fn dormant_threads_and_completed_turns_release_runtime_state_without_losing_identity() {
    let (host, mut receiver) = host();
    let at = jiff::Timestamp::now();
    for index in 0..1024 {
        let thread = format!("thread-{}", index / 16);
        let turn = format!("turn-{index}");
        host.adopt(
            &thread,
            super::ThreadSpec {
            native_resume: false,                user_mcp: Default::default(),
                owner: fastctx_types::Address {
                    machine: "desktop".parse().unwrap(),
                    resident: "codex-c0de".parse().unwrap(),
                },
                cwd: host.workdir.clone(),
                developer_instructions: "private fixture".repeat(128),
                config: json!({}),
                name: None,
                choices: host.workdir.join("choices.json"),
            },
        );
        host.notified("turn/started", &json!({"threadId":thread,"turn":{"id":turn}}), at);
        host.end_with(&thread, &turn, fastctx_types::session::TurnHow::Completed, None, None, at, None, None);
        while let Ok(event) = receiver.try_recv() {
            host.acknowledge(&event);
        }
        assert!(host.lock().threads.values().next().is_none());
        assert!(host.finished.contains(&thread, &turn));
        host.notified("turn/started", &json!({"threadId":thread,"turn":{"id":turn}}), at);
        assert!(host.running_turn(&thread).is_none());
        assert!(receiver.try_recv().is_err());
    }
    let spec = host.lock().threads.get("thread-0").unwrap().spec.clone().unwrap();
    assert_eq!(spec.developer_instructions, "private fixture".repeat(128));
    assert!(host.lock().threads.values().next().is_none());
    let root = host.workdir.clone();
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}

fn item(value: Value, completed: bool) -> fastctx_types::session::Item {
    match items::read(&value, completed) {
        items::Read::Item(item) => item,
        _ => panic!("expected visible item"),
    }
}

#[test]
fn captured_live_items_match_completed_history() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/session-0.160.json")).unwrap();
    let (host, mut receiver) = host();
    for message in fixture["notifications"].as_array().unwrap() {
        host.notified(message["method"].as_str().unwrap(), &message["params"], millis(message.get("emittedAtMs")));
    }
    let mut live = Vec::new();
    while let Ok(HostEvent::Happened { event, .. }) = receiver.try_recv() {
        live.push(event);
    }
    let history = history::project(&fixture["history"], None, None);
    let finished = |events: Vec<fastctx_types::session::SessionEvent>| {
        events
            .into_iter()
            .filter_map(|event| {
                if let Happened::Finished { turn, mut item } = event.happened {
                    // Timing is when the events arrived live, and what
                    // stamps the history keeps; the items are what match.
                    item.started_at = None;
                    if let What::Thinking { duration_ms, .. } = &mut item.what {
                        *duration_ms = None;
                    }
                    Some((turn, item))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    let live = finished(live);
    assert_eq!(live, finished(history));
    assert!(live.iter().any(|(_, item)| matches!(&item.what, What::Tool(tool) if matches!(&tool.does, Does::Command { exit: Some(7), .. }) && tool.output == vec![Content::Text { text: "EVENTS_PROBE_COMMAND\r\n".into() }])));
    assert!(live.iter().any(|(_, item)| {
        item.detail.as_ref().is_some_and(|d| {
            d.pointer("/result/structuredContent") == Some(&json!({"exact":[1,true,null,"汉字"]}))
                && d.pointer("/result/_meta") == Some(&json!({"probe":"metadata"}))
        })
    }));
    assert!(live.iter().any(|(_, item)| matches!(&item.what, What::Tool(tool) if tool.output.iter().any(|c| matches!(c, Content::Other { block: value } if value["type"] == "resource_link")))));
}

#[test]
fn presentation_fields_and_hidden_items_follow_the_frozen_contract() {
    let message = item(
        json!({"type":"agentMessage","id":"m","text":"原文","phase":"final_answer","delivery":"async","memoryCitation":{"entries":[{"path":"memory.md","lineStart":2,"lineEnd":4,"note":"n"}],"threadIds":["t"]},"questions":[{"title":"choice","options":["a","b"]}]}),
        true,
    );
    let detail = message.detail.unwrap();
    assert_eq!(detail["phase"], "final_answer");
    assert_eq!(detail["delivery"], "async");
    assert_eq!(
        detail["memoryCitation"],
        json!({"entries":[{"path":"memory.md","lineStart":2,"lineEnd":4,"note":"n"}],"threadIds":["t"]})
    );
    assert_eq!(detail["questions"], json!([{"title":"choice","options":["a","b"]}]));
    let call = item(
        json!({"type":"mcpToolCall","id":"m","server":"s","tool":"t","arguments":"{\"literal\":true}","status":"completed","mcpAppUi":{"resourceUri":"ui://a","preferredModelDisplayMode":"inline"},"result":{"content":[{"type":"text","text":"t","annotations":{"audience":["user"]}},{"type":"audio","data":"YQ==","mimeType":"audio/wav"},{"type":"resource","resource":{"uri":"file:///x","text":"exact"}}],"structuredContent":[1],"_meta":{"x":2}}}),
        true,
    );
    let What::Tool(tool) = call.what else { panic!() };
    assert_eq!(tool.input, json!("{\"literal\":true}"));
    assert!(tool.output.iter().all(|p| matches!(p, Content::Other { .. })));
    assert_eq!(call.detail.unwrap()["mcpAppUi"]["resourceUri"], "ui://a");
    let dynamic = item(
        json!({"type":"dynamicToolCall","id":"d","tool":"x","arguments":"123","status":"completed","success":true,"contentItems":[{"type":"inputImage","imageUrl":"https://example.org/x.png"},{"type":"inputAudio","audioUrl":"data:audio/wav;base64,YQ=="}]}),
        true,
    );
    let What::Tool(tool) = dynamic.what else { panic!() };
    assert_eq!(tool.input, json!("123"));
    assert_eq!(
        tool.output,
        vec![
            Content::ImageUrl { url: "https://example.org/x.png".into() },
            Content::AudioUrl { url: "data:audio/wav;base64,YQ==".into() }
        ]
    );
    let collab = item(
        json!({"type":"collabAgentToolCall","id":"c","tool":"sendInput","status":"completed","senderThreadId":"a","receiverThreadIds":["b","c"],"agentsStates":{"b":{"status":"completed","message":"done"},"c":{"status":"running","message":null}},"prompt":"task","model":"model","reasoningEffort":"high"}),
        true,
    );
    let detail = collab.detail.unwrap();
    assert_eq!(detail["receiverThreadIds"], json!(["b", "c"]));
    assert_eq!(detail["agentsStates"]["b"]["message"], "done");
    let search = item(json!({"type":"webSearch","id":"s","query":"","action":null,"results":null}), false);
    assert!(matches!(search.what,What::Tool(tool) if tool.status == ToolStatus::Running));
    for kind in ["sleep", "enteredReviewMode", "exitedReviewMode", "functionCallOutput"] {
        assert!(matches!(
            items::read(&json!({"id":"hidden","type":kind}), true),
            items::Read::Item(fastctx_types::session::Item { what: What::Hidden { .. }, .. })
        ));
    }
    for name in ["update_running_summary", "update_up_next", "load_workspace_dependencies", "automation_update"] {
        assert!(matches!(
            items::read(
                &json!({"id":"hidden","type":"dynamicToolCall","tool":name,"namespace":"codex_app","status":"inProgress"}),
                false
            ),
            items::Read::Item(fastctx_types::session::Item { what: What::Hidden { .. }, .. })
        ));
    }
    let hook = items::read(
        &json!({"type":"hookPrompt","id":"h","fragments":[{"text":"first","hookRunId":"a"},{"text":"second","hookRunId":"b"}]}),
        true,
    );
    assert!(
        matches!(hook,items::Read::Words{text,message} if text == "first\nsecond" && message.hook_fragments.len()==2)
    );
    for raw in [
        json!({"type":"imageGeneration","id":"g","status":"failed","result":"","failure":null}),
        json!({"type":"fileChange","id":"p","status":"completed","changes":[]}),
    ] {
        let hidden = item(raw.clone(), true);
        assert!(matches!(hidden.what, What::Hidden { .. }));
        assert_eq!(hidden.detail, Some(raw));
    }
    // Reasoning without a summary is thinking all the same, its private
    // content never shown.
    let thought = item(json!({"type":"reasoning","id":"r","summary":[],"content":["private"]}), true);
    assert_eq!(thought.what, What::Thinking { text: String::new(), duration_ms: None });
    let limit = item(
        json!({"type":"imageGeneration","id":"g","status":"failed","result":"","failure":{"type":"usageLimitExceeded","limitId":"image_gen","resetsAt":null}}),
        true,
    );
    assert!(matches!(limit.what, What::Tool(tool) if tool.status == ToolStatus::Failed));
}

#[test]
fn indexed_summaries_patch_replacement_and_thread_state() {
    let (host, mut receiver) = host();
    let at = jiff::Timestamp::from_millisecond(1000).unwrap();
    let send = |method: &str, params: Value| host.notified(method, &params, at);
    send(
        "item/started",
        json!({"threadId":"t","turnId":"a","startedAtMs":1000,"item":{"id":"r","type":"reasoning","summary":["A","B"],"content":["never display"]}}),
    );
    send("item/reasoning/summaryPartAdded", json!({"threadId":"t","turnId":"a","itemId":"r","summaryIndex":1}));
    send(
        "item/reasoning/summaryTextDelta",
        json!({"threadId":"t","turnId":"a","itemId":"r","summaryIndex":1,"delta":" second"}),
    );
    send(
        "item/reasoning/summaryTextDelta",
        json!({"threadId":"t","turnId":"a","itemId":"r","summaryIndex":0,"delta":" first"}),
    );
    send(
        "item/fileChange/patchUpdated",
        json!({"threadId":"t","turnId":"a","itemId":"p","changes":[{"path":"old","kind":{"type":"update","move_path":"new"},"diff":"@@\n-a\n+b\n"}]}),
    );
    send("thread/status/changed", json!({"threadId":"t","status":{"type":"idle"}}));
    send(
        "model/safetyBuffering/updated",
        json!({"threadId":"t","turnId":"a","model":"m","showBufferingUi":false,"useCases":[],"reasons":[],"fasterModel":null}),
    );
    send("warning", json!({"threadId":"t","message":"hidden warning"}));
    send("thread/name/updated", json!({"threadId":"t","threadName":"  exact  "}));
    send("thread/name/updated", json!({"threadId":"t","threadName":"  "}));
    let mut events = Vec::new();
    while let Ok(HostEvent::Happened { event, .. }) = receiver.try_recv() {
        events.push(event);
    }
    assert_eq!(events.len(), 7);
    assert!(events.iter().any(|e| matches!(&e.happened,Happened::Updated{item,..} if item.what == What::Thinking{text:"A first\n\nB second".into(),duration_ms:None})));
    assert!(events.iter().any(|e| matches!(&e.happened,Happened::Updated{item,..} if matches!(&item.what,What::Tool(tool) if matches!(&tool.does,Does::Edit{changes} if changes[0].how == fastctx_types::session::ChangeHow::Moved{to:"new".into()})))));
    assert!(events.iter().any(|e| matches!(&e.happened,Happened::Changed{change:SessionChange::Safety{buffering},..} if buffering["showBufferingUi"] == false)));
    assert!(events.iter().any(|e| matches!(&e.happened,Happened::Titled{title} if title == "exact")));
    assert!(events.iter().all(|e| e.at == at));
}
