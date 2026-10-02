use super::*;

#[test]
fn atomic_publication_keeps_existing_read_handles_valid() {
    use std::io::Read;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("open.txt");
    std::fs::write(&path, b"before").unwrap();
    let mut reader = std::fs::File::open(&path).unwrap();
    write(
        WriteRequest {
            file_path: path.to_string_lossy().into(),
            content: "after".into(),
        },
        &WriteContext::default(),
    )
    .unwrap();
    let mut original = String::new();
    reader.read_to_string(&mut original).unwrap();
    assert_eq!(original, "before");
    assert_eq!(std::fs::read(path).unwrap(), b"after");
}

#[test]
fn byte_integrity_and_observed_revision_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("source.txt");
    let original: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain("old\r\nlast\r\n".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    std::fs::write(&file, &original).unwrap();
    let context = WriteContext {
        observed_hash: Some("stale"),
        ..WriteContext::default()
    };
    assert!(
        write(
            WriteRequest {
                file_path: file.to_string_lossy().into(),
                content: "lost".into()
            },
            &context
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&file).unwrap(), original);
    let result = edit(
        EditRequest {
            file_path: file.to_string_lossy().into(),
            old_string: "old".into(),
            new_string: "new\nadded".into(),
            replace_all: false,
        },
        &context,
    )
    .unwrap();
    let expected: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain(
            "new\r\nadded\r\nlast\r\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        )
        .collect();
    assert_eq!(std::fs::read(file).unwrap(), expected);
    assert_eq!(result.changes[0].before_hash, Some(content_hash(&original)));
    assert_eq!(result.changes[0].after_hash, Some(content_hash(&expected)));
    assert_eq!(result.changed_since_observation.len(), 1);
    assert!(result.text.contains("2: added"));
}

#[test]
fn codex_ordered_sections_fuzzy_matching_and_failure_ledger_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("source.txt");
    let missing = temporary.path().join("missing.txt");
    std::fs::write(&file, "item  \nitem\n").unwrap();
    let input = format!(
        "<<'EOF'\n*** Begin Patch\n*** Update File: {}\n@@\n-item\n+chosen\n*** Update File: {}\n@@ item\n+appended\n*** Delete File: {}\n*** End Patch\nEOF",
        file.display(),
        file.display(),
        missing.display()
    );
    let error = apply_patch(ApplyPatchRequest { input }, &WriteContext::default()).unwrap_err();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "item  \nchosen\nappended\n"
    );
    assert_eq!(error.changes.len(), 2);
    assert_eq!(error.changes[0].after_hash, error.changes[1].before_hash);
    assert_eq!(
        error.changes[1].after_hash,
        Some(content_hash(&std::fs::read(file).unwrap()))
    );
}

#[test]
fn concurrent_editors_keep_independent_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("shared.txt");
    let entries = ["first", "second", "third", "fourth"];
    std::fs::write(&file, entries.join("\n")).unwrap();
    std::thread::scope(|scope| {
        for entry in entries {
            let file = &file;
            scope.spawn(move || {
                edit(
                    EditRequest {
                        file_path: file.to_string_lossy().into(),
                        old_string: entry.into(),
                        new_string: entry.to_uppercase(),
                        replace_all: false,
                    },
                    &WriteContext::default(),
                )
                .unwrap();
            });
        }
    });
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        entries.join("\n").to_uppercase()
    );
}
