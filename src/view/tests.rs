use super::*;
use base64::Engine;

fn args(paths: Vec<PathBuf>) -> ViewArgs {
    ViewArgs {
        paths,
        lines: None,
        pages: None,
        image: false,
        encoding: None,
        files0_from: None,
    }
}

#[test]
fn ordered_view_preserves_failures_and_stops_at_its_character_budget() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let first = directory.path().join("first.txt");
    let large = directory.path().join("large.txt");
    let last = directory.path().join("last.txt");
    fs::write(&first, "hello\n").unwrap();
    fs::write(&large, "界".repeat(CHARACTER_BUDGET * 2)).unwrap();
    fs::write(&last, "not reached").unwrap();
    let mut output = Vec::new();
    assert!(
        write(
            &args(vec![missing, first, large, last]),
            "test-node",
            &mut output
        )
        .unwrap()
    );
    let output = String::from_utf8(output).unwrap();
    assert!(output.starts_with("=== view (4 files: 2 shown, 2 not shown) ===\n"));
    assert_eq!(output.chars().count(), 150_000);
    assert!(output.contains("File does not exist."));
    assert!(output.contains("1\thello\n"));
    assert!(output.contains("line 1, chars 1-"));
    assert!(output.ends_with("last.txt (not shown: 11 bytes) ===\n"));
    assert!(!output.contains('\u{fffd}'));
}

#[test]
fn osc1337_preserves_original_gif_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pixel.gif");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7")
        .unwrap();
    fs::write(&path, &bytes).unwrap();
    let mut output = Vec::new();
    assert!(!write(&args(vec![path]), "test-node", &mut output).unwrap());
    let output = String::from_utf8(output).unwrap();
    let (head, transport) = output.split_once("\x1b]1337;File=").unwrap();
    assert!(head.ends_with("(image 1x1) ===\n"));
    let (parameters, payload) = transport.split_once(':').unwrap();
    assert!(parameters.contains("inline=1"));
    assert!(parameters.contains(&format!("size={}", bytes.len())));
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(payload.strip_suffix("\x07\n").unwrap())
            .unwrap(),
        bytes
    );
}
