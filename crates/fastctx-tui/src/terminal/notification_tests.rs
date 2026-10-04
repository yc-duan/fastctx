use super::{NotificationProtocol::*, *};
use crate::input::Focus;

#[test]
fn notification_wire_contract() {
    // Frozen from the terminal protocols, including a single OSC 99 completion
    // and no activation/close reports. Payload cannot terminate its own OSC.
    assert_eq!(encode(Osc9, "4;标题\x07", "正文\x1b\\\n"), "\x1b]9;4,标题 : 正文 \\ \x1b\\");
    assert_eq!(encode(Osc777, "标题;\u{9d}", "正文\x07"), "\x1b]777;notify;标题, ;正文 \x1b\\");
    assert_eq!(
        encode(Osc99, "标题", "正文;😀"),
        "\x1b]99;p=title:d=0:a=focus:c=0:o=unfocused;标题\x1b\\\x1b]99;p=body:d=1:a=focus:c=0:o=unfocused;正文;😀\x1b\\"
    );
    let text = "😀".repeat(1025);
    let message = encode(Osc99, &text, &text);
    let chunks: Vec<_> = message.split("\x1b\\").filter(|s| !s.is_empty()).collect();
    assert_eq!(chunks.len(), 6);
    assert_eq!(chunks.iter().filter(|s| s.contains(":d=1:")).count(), 1);
    let mut recovered = String::new();
    for chunk in chunks {
        let payload = chunk.splitn(3, ';').nth(2).unwrap();
        assert!(payload.len() <= 2048);
        recovered.push_str(payload);
    }
    assert_eq!(recovered, text.repeat(2));
}

#[test]
fn foreground_unknown_and_unsupported_never_write_or_flush() {
    struct NoOutput;
    impl std::io::Write for NoOutput {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            panic!("suppressed notification wrote output")
        }
        fn flush(&mut self) -> std::io::Result<()> {
            panic!("suppressed notification flushed output")
        }
    }
    for protocol in [None, Some(Osc9), Some(Osc777), Some(Osc99)] {
        for focus in [Focus::Unknown, Focus::Focused, Focus::Unfocused] {
            if protocol.is_some() && focus == Focus::Unfocused {
                continue;
            }
            assert!(!notify(&mut NoOutput, protocol, || focus, "Title", "Body").unwrap());
        }
    }
    let mut output = Vec::new();
    assert!(notify(&mut output, Some(Osc777), || Focus::Unfocused, "Title", "Body").unwrap());
    assert_eq!(output, b"\x1b]777;notify;Title;Body\x1b\\");
    assert!(!notify(&mut NoOutput, Some(Osc777), || Focus::Focused, "Title", "Body").unwrap());
}
