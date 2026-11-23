//! Tests of `stored`: which pictures' bytes an event is kept without.
use super::*;

const SHOWN: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk";
const ALONE: &str = "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

fn tool(output: serde_json::Value, detail: serde_json::Value) -> SessionEvent {
    serde_json::from_value(serde_json::json!({
        "at": "2026-11-23T00:00:00Z",
        "event": "finished",
        "turn": "t1",
        "item": {
            "id": "i1",
            "detail": detail,
            "kind": "tool",
            "name": "screenshot",
            "server": "browser",
            "machine": null,
            "does": { "as": "call" },
            "input": {},
            "output": output,
            "status": "done",
            "duration_ms": null
        }
    }))
    .unwrap()
}

fn detail_of(event: &SessionEvent) -> serde_json::Value {
    match &event.happened {
        Happened::Finished { item, .. } => item.detail.clone().unwrap_or_default(),
        _ => panic!("a finished item"),
    }
}

/// A picture the tool's output holds goes from the host's raw record; one
/// only the raw record holds, as Codex's app may leave it, stays there.
#[test]
fn a_raw_record_keeps_the_bytes_of_the_pictures_it_alone_holds() {
    let repeated = tool(
        serde_json::json!([{ "type": "image", "mime": "image/png", "base64": SHOWN }]),
        serde_json::json!({ "result": { "content": [{ "type": "image", "mimeType": "image/png", "data": SHOWN }] } }),
    );
    let kept = form(&repeated).expect("the repeated picture goes");
    assert_eq!(detail_of(&kept)["result"]["content"][0]["data"], "");
    assert_eq!(serde_json::to_value(&kept).unwrap()["item"]["output"][0]["base64"], SHOWN);

    let alone = tool(
        serde_json::json!([{ "type": "text", "text": "a picture" }]),
        serde_json::json!({ "result": { "content": [{ "type": "image", "mimeType": "image/png", "data": ALONE }] } }),
    );
    assert!(form(&alone).is_none());

    let both = tool(
        serde_json::json!([{ "type": "image", "mime": "image/png", "base64": SHOWN }]),
        serde_json::json!({ "result": { "content": [
            { "type": "image", "mimeType": "image/png", "data": SHOWN },
            { "type": "image", "mimeType": "image/gif", "data": ALONE },
            format!("data:image/png;base64,{SHOWN}")
        ] } }),
    );
    let kept = form(&both).expect("the repeated picture goes");
    let detail = detail_of(&kept);
    assert_eq!(detail["result"]["content"][0]["data"], "");
    assert_eq!(detail["result"]["content"][1]["data"], ALONE);
    assert_eq!(detail["result"]["content"][2], "data:image/png;base64,");
}
