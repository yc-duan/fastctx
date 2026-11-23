//! Tests of `tool_results`: which pictures a session's record holds itself.
use super::*;

const SHOWN: &str = "mcp-fastctx-blob-1794995318368-78snu7.png";
const RESIZED: &str = "mcp-plugin_x-blob-1795376882438-g0417o.png";
const ONLY: &str = "mcp-fastctx-blob-1795374292680-1xqgck.jpg";
const EMPTY: &str = "mcp-fastctx-blob-1795374292681-aaaaaa.webp";

fn line(blocks: serde_json::Value) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": [ { "type": "tool_result", "content": blocks } ] },
        "toolUseResult": blocks,
    })
    .to_string()
}

fn image(data: &str) -> serde_json::Value {
    serde_json::json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": data } })
}

fn text(text: String) -> serde_json::Value {
    serde_json::json!({ "type": "text", "text": text })
}

/// A picture the record holds with its bytes goes, shrunk or not; one it only
/// names, or names after a picture block without bytes, is the only copy.
#[test]
fn only_the_pictures_a_record_holds_with_their_bytes_are_taken_as_held() {
    let folder = tempfile::tempdir().unwrap();
    let transcript = folder.path().join("session.jsonl");
    let at = |name: &str| format!("/home/claude/.claude/projects/p/session/tool-results/{name}");
    let lines = [
        line(serde_json::json!([
            image("iVBORw0KGgo="),
            text(format!("[Image: source: {}]", at(SHOWN))),
            text("\n".into())
        ])),
        line(serde_json::json!([
            image("/9j/4AAQ"),
            text(format!("[Image: source: {}, original 2400x800, displayed at 2000x667.]", at(RESIZED)))
        ])),
        line(serde_json::json!([text(format!("[Image: source: {}]", at(ONLY)))])),
        line(serde_json::json!([image(""), text(format!("[Image: source: {}]", at(EMPTY)))])),
        "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"no pictures\"}]}}".to_string(),
    ];
    std::fs::write(&transcript, lines.join("\n")).unwrap();
    let held = held_inline(&transcript).unwrap();
    assert!(held.contains(SHOWN));
    assert!(held.contains(RESIZED));
    assert!(!held.contains(ONLY));
    assert!(!held.contains(EMPTY));
    assert_eq!(held.len(), 2);
}

#[test]
fn a_picture_is_named_by_claude_code_s_own_name_for_its_copy() {
    assert!(picture(SHOWN));
    assert!(picture("mcp-a-b-blob-1-x.JPEG"));
    assert!(!picture("toolu_01.txt"));
    assert!(!picture("mcp-a-blob.json"));
    assert_eq!(named(&text(format!("[Image: source: C:\\Users\\u\\tool-results\\{SHOWN}]"))).as_deref(), Some(SHOWN));
    assert_eq!(named(&text("[Image: source: /x/notes.txt]".into())), None);
}
