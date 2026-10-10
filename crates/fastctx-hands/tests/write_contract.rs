use fastctx_hands::{
    encoding,
    files::{canonical_path, content_hash},
    write::*,
};
use fastctx_types::{
    files::{SeenFile, SeenFiles, SeenVersion},
    layout::WorldDir,
};
use std::fs;
use std::io::Read;
use std::sync::{Arc, Barrier};

#[test]
fn publication_and_observation_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let world = WorldDir::under(dir.path());
    let writer = Writer::new(&world, "test").unwrap();
    let path = dir.path().join("file.txt");
    fs::write(&path, b"before\r\n").unwrap();
    let mut reader = fs::File::open(&path).unwrap();
    let mut seen = SeenFiles::new();
    seen.insert(
        canonical_path(&path).unwrap(),
        SeenFile { version: SeenVersion::Full { sha256: content_hash(b"before\r\n") }, encoding: Some("UTF-8".into()) },
    );
    let result = writer
        .edit(
            EditRequest {
                encoding: None,
                file_path: path.clone(),
                old_string: "before".into(),
                new_string: "after".into(),
                replace_all: false,
            },
            &seen,
        )
        .unwrap();
    assert_eq!(result.changes[0].before_hash, Some(content_hash(b"before\r\n")));
    assert_eq!(result.changes[0].after_hash, Some(content_hash(b"after\r\n")));
    assert_eq!(fs::read(&path).unwrap(), b"after\r\n");
    let mut held = Vec::new();
    reader.read_to_end(&mut held).unwrap();
    assert_eq!(held, b"before\r\n");
    assert!(
        writer.write(WriteRequest { encoding: None, file_path: path.clone(), content: "stale".into() }, &seen).is_err()
    );
    seen.get_mut(&canonical_path(&path).unwrap()).unwrap().version =
        SeenVersion::Partial { size: 0, modified: fs::metadata(&path).unwrap().modified().unwrap() };
    assert!(
        writer.write(WriteRequest { encoding: None, file_path: path.clone(), content: "stale".into() }, &seen).is_err()
    );
    let result = writer
        .edit(
            EditRequest {
                encoding: None,
                file_path: path.clone(),
                old_string: "after".into(),
                new_string: "current".into(),
                replace_all: false,
            },
            &seen,
        )
        .unwrap();
    assert_eq!(result.changed_since_seen, vec![path]);
    let journal = world.path().join("towns/test/ledger/journals/note/text.md");
    assert!(
        writer
            .write(WriteRequest { encoding: None, file_path: journal.clone(), content: "not allowed".into() }, &seen)
            .is_err()
    );
    assert!(!journal.exists());
    let path = dir.path().join("partial.txt");
    let error = writer.apply_patch(ApplyPatchRequest { input: format!("*** Begin Patch\n*** Add File: {}\n+committed\n*** Update File: {}\n@@\n-missing\n+no\n*** End Patch", path.display(), dir.path().join("missing").display()) }, &seen).unwrap_err();
    assert_eq!(error.changes.len(), 1);
    assert_eq!(fs::read(path).unwrap(), b"committed\n");
}

#[test]
fn edit_blocks_publish_only_complete_batches() {
    let dir = tempfile::tempdir().unwrap();
    let writer = Writer::new(&WorldDir::under(dir.path()), "test").unwrap();
    let path = dir.path().join("batch.txt");
    let before = b"first\r\nlast\r\n";
    fs::write(&path, before).unwrap();
    let first = "<<<<<<< SEARCH\nfirst\n=======\nchanged\n>>>>>>> REPLACE\n";
    for second in [
        "<<<<<<< SEARCH\nmissing\n=======\nend\n>>>>>>> REPLACE\n",
        "<<<<<<< SEARCH\nlast\n=======\nend\n<<<<<<< SEARCH\n",
    ] {
        let error = writer
            .edit_blocks(
                EditBlocksRequest {
                    encoding: None,
                    file_path: path.clone(),
                    input: format!("{first}{second}"),
                    replace_all: false,
                },
                &SeenFiles::new(),
            )
            .unwrap_err();
        assert!(error.message.contains("Block 2"), "{error}");
        assert!(error.changes.is_empty());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    let mut reader = fs::File::open(&path).unwrap();
    let result = writer
        .edit_blocks(
            EditBlocksRequest {
                encoding: None,
                file_path: path.clone(),
                input: format!("{first}<<<<<<< SEARCH\nchanged\n=======\nfinal\nline\n>>>>>>> REPLACE\n"),
                replace_all: false,
            },
            &SeenFiles::new(),
        )
        .unwrap();
    assert_eq!(result.changes.len(), 1);
    assert!(result.text.contains("2 changes"));
    assert_eq!(fs::read(&path).unwrap(), b"final\r\nline\r\nlast\r\n");
    let mut held = Vec::new();
    reader.read_to_end(&mut held).unwrap();
    assert_eq!(held, before);
}

#[test]
fn concurrent_writers_preserve_every_successful_edit() {
    let dir = tempfile::tempdir().unwrap();
    let world = WorldDir::under(dir.path());
    let path = dir.path().join("file.txt");
    fs::write(&path, b"anchor\n").unwrap();
    let barrier = Arc::new(Barrier::new(4));
    std::thread::scope(|scope| {
        for id in 0..4 {
            let writer = Writer::new(&world, "test").unwrap();
            let path = &path;
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                for index in 0..20 {
                    writer
                        .edit(
                            EditRequest {
                                encoding: None,
                                file_path: path.clone(),
                                old_string: "anchor\n".into(),
                                new_string: format!("anchor\n{id}:{index}\n"),
                                replace_all: false,
                            },
                            &SeenFiles::new(),
                        )
                        .unwrap();
                }
            });
        }
    });
    let text = fs::read_to_string(path).unwrap();
    assert_eq!(text.lines().count(), 81);
    for id in 0..4 {
        for index in 0..20 {
            assert_eq!(text.lines().filter(|line| *line == format!("{id}:{index}")).count(), 1);
        }
    }
}

/// Frozen against OpenAI Codex 7431f10's parser and matching behavior.
#[test]
fn codex_matching_contract() {
    let cases = [
        ("alpha \nalpha\n", "@@\n-alpha\n+beta", "alpha \nbeta\n"),
        ("anchor\nlast\n", "@@ anchor\n+added", "anchor\nlast\nadded\n"),
        ("one\nlast\n", "@@\n+first\n@@\n+second", "one\nlast\nfirst\nsecond\n"),
        ("“quoted”\n", "@@\n-\"quoted\"\n+changed", "changed\n"),
        ("one\nlast\n", "@@ last\n-last\n+changed\n*** End of File", "one\nchanged\n"),
        ("one", "@@\n-one\n+two", "two\n"),
        ("one\n", "-one", ""),
    ];
    let dir = tempfile::tempdir().unwrap();
    let writer = Writer::new(&WorldDir::under(dir.path()), "test").unwrap();
    let path = dir.path().join("file.txt");
    for (before, chunk, expected) in cases {
        fs::write(&path, before).unwrap();
        writer
            .apply_patch(
                ApplyPatchRequest {
                    input: format!("*** Begin Patch\n*** Update File: {}\n{chunk}\n*** End Patch", path.display()),
                },
                &SeenFiles::new(),
            )
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), expected, "{chunk}");
    }
}

#[test]
fn strict_encoding_streams_and_roundtrips() {
    for (label, text) in [
        ("utf-8", "A中😀\r\n"),
        ("utf-16le", "A中😀\r\n"),
        ("utf-16be", "A中😀\r\n"),
        ("utf-32le", "A中😀\r\n"),
        ("utf-32be", "A中😀\r\n"),
        ("gbk", "中文\r\n"),
        ("shift_jis", "日本語\r\n"),
        ("big5", "繁體\r\n"),
        ("windows-1252", "café\r\n"),
    ] {
        let codec = encoding::detect(b"", Some(label)).unwrap();
        let bytes = codec.encode(text).unwrap();
        assert_eq!(codec.decode_for_write(&bytes).unwrap(), text);
        let mut decoder = encoding::TextDecoder::new(codec);
        let mut result = String::new();
        for byte in &bytes {
            result.push_str(&decoder.feed(&[*byte], false).unwrap());
        }
        result.push_str(&decoder.feed(&[], true).unwrap());
        assert_eq!(result, text);
    }
    // The UTF-16LE BOM stays metadata even when the first scalar is NUL.
    let raw = b"\xff\xfe\x00\x00A\x00";
    let codec = encoding::detect(raw, Some("utf-16le")).unwrap();
    assert_eq!(codec.bom_len(), 2);
    assert_eq!(codec.decode_for_write(raw).unwrap(), "\0A");
    assert!(encoding::detect(&[0xc3], Some("utf-8")).is_err());
    assert!(encoding::detect(&[0xff, 0xfe, 0, 0xd8], None).is_err());
    assert!(matches!(
        encoding::detect(&[0xc4, 0xe3, 0xba, 0xc3], None),
        Err(encoding::EncodingError::Ambiguous { .. })
    ));
    assert!(encoding::detect(b"", Some("shift_jis")).unwrap().encode("¥").is_err());
}
