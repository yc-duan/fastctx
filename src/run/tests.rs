//! Frozen inline-image wire contracts and the public response budget.

use super::*;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

#[test]
fn atomic_records_preserve_open_readers_of_the_previous_generation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("job.json");
    let before = serde_json::json!({"state": "running"});
    let after = serde_json::json!({"state": "exited"});
    super::record::write(&path, &before).unwrap();
    let held_reader = std::fs::File::open(&path).unwrap();
    super::record::write(&path, &after).unwrap();
    assert_eq!(
        serde_json::from_reader::<_, serde_json::Value>(held_reader).unwrap(),
        before
    );
    assert_eq!(
        serde_json::from_reader::<_, serde_json::Value>(std::fs::File::open(path).unwrap())
            .unwrap(),
        after
    );
}

fn assemble_bytes(bytes: &[u8]) -> RunResult {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("out.log");
    std::fs::write(&log, bytes).unwrap();
    let job = JobRecord {
        id: "j-abc123".into(),
        pid: Some(1),
        supervisor_pid: 2,
        supervisor_identity: "fixture".into(),
        command: "fixture".into(),
        cwd: directory.path().to_owned(),
        machine: "desktop".into(),
        started_at: "2026-10-02T00:00:00Z".into(),
        ended_at: Some("2026-10-02T00:00:01Z".into()),
        output_path: log,
        state: JobState::Exited,
        exit_code: Some(0),
        error: None,
        hook_error: None,
    };
    assemble(job, directory.path().join("job.json"), bytes.len() as u64).unwrap()
}

#[test]
fn inline_image_protocols_preserve_wire_order_and_report_damage() {
    // https://iterm2.com/documentation-images.html and
    // https://sw.kovidgoyal.net/kitty/graphics-protocol/#remote-client
    let source = format!(
        "before\n\x1b]1337;File=inline=1:{PNG}\x07between\n\x1b_Ga=T,f=100,m=1;{}\x1b\\\x1b_Gm=0;{}\x1b\\after\n\x1b]1337;MultipartFile=inline=1\x07\x1b]1337;FilePart={}\x07\x1b]1337;FilePart={}\x07\x1b]1337;FileEnd\x07\x1b_Ga=T,f=100;@@@@\x1b\\",
        &PNG[..40],
        &PNG[40..],
        &PNG[..40],
        &PNG[40..]
    );
    let response = assemble_bytes(source.as_bytes());
    assert_eq!(response.content.len(), 7);
    for index in [1, 3, 5] {
        let ContentBlock::Image {
            data,
            width,
            height,
            ..
        } = &response.content[index]
        else {
            panic!("image missing at {index}");
        };
        assert_eq!((data.as_str(), *width, *height), (PNG, 1, 1));
    }
    for (index, expected) in [
        (0, "before\n"),
        (2, "between\n"),
        (4, "after\n"),
        (6, "Incomplete kitty image"),
    ] {
        let ContentBlock::Text { text } = &response.content[index] else {
            panic!("text missing");
        };
        assert!(text.contains(expected), "{text}");
    }
    for broken in [
        "\x1b]1337;File=inline=1:YW",
        "\x1b_Ga=T,f=100,m=1;YWJj\x1b\\",
    ] {
        let response = assemble_bytes(broken.as_bytes());
        assert!(
            matches!(&response.content[0], ContentBlock::Text { text } if text.contains("Incomplete"))
        );
    }
}

#[test]
fn character_budget_keeps_unicode_boundaries_and_explicit_log_coordinates() {
    let source = format!("head{}tail", "甲🦀".repeat(100_000));
    let response = assemble_bytes(source.as_bytes());
    let ContentBlock::Text { text } = &response.content[0] else {
        panic!("text missing");
    };
    assert!(text.chars().count() <= CHARACTER_BUDGET);
    assert!(text.contains("head") && text.ends_with("tail"));
    let omitted = response.omitted.unwrap();
    assert!(source.is_char_boundary(omitted.start as usize));
    assert!(source.is_char_boundary(omitted.end as usize));
    assert!(omitted.start < omitted.end);
    assert!(text.contains("omitted log bytes"));
}
