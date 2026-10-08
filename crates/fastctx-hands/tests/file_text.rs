//! Byte preservation and the view-to-write encoding contract.

use fastctx_hands::{
    files::canonical_path,
    view::{Options, view},
    write::*,
};
use fastctx_types::{
    files::{SeenFile, SeenFiles, SeenVersion},
    layout::WorldDir,
};
use std::{fs, path::Path};

fn observe(path: &Path, encoding: Option<&str>) -> SeenFiles {
    let observed = view(
        &[path.to_path_buf()],
        &Options { encoding: encoding.map(str::to_owned), ..Options::default() },
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(observed.len(), 1);
    let observed = &observed[0];
    SeenFiles::from([(
        canonical_path(path).unwrap(),
        SeenFile {
            version: SeenVersion::Full { sha256: observed.sha256.clone().unwrap() },
            encoding: observed.encoding_selection.clone(),
        },
    )])
}

#[test]
fn view_selection_and_local_bytes_agree() {
    let directory = tempfile::tempdir().unwrap();
    let writer = Writer::new(&WorldDir::under(directory.path()), "test").unwrap();
    let path = directory.path().join("text");
    // Noncanonical Shift_JIS, WHATWG Big5, a GB18030 four-byte scalar, and
    // UTF-16 byte order signatures are independent encoded-byte fixtures.
    let cases: &[(&[u8], Option<&str>, &str, &str, &[u8])] = &[
        (b"\x87\x90 old\r\n", Some("shift_jis"), "old", "NEW", b"\x87\x90 NEW\r\n"),
        (b"\xc6\xa1\xa4\xa4 old\n", Some("big5"), "①", "X", b"X\xa4\xa4 old\n"),
        (b"\x90\x30\x81\x30 old\n", Some("gbk"), "old", "𐀀", b"\x90\x30\x81\x30 \x90\x30\x81\x30\n"),
        (b"\xff\xfe\0\0A\0", Some("utf-16le"), "A", "B", b"\xff\xfe\0\0B\0"),
        (b"a\0b\0\n\0", None, "b", "c", b"a\0c\0\n\0"),
        (b"\x1b$B$\x22$$$&\x1b(B", None, "い", "A", b"\x1b$B$\x22\x1b(BA\x1b$B$&\x1b(B"),
    ];
    for &(before, selection, old, new, expected) in cases {
        fs::write(&path, before).unwrap();
        let seen = observe(&path, selection);
        let result = writer
            .edit(
                EditRequest {
                    file_path: path.clone(),
                    old_string: old.into(),
                    new_string: new.into(),
                    replace_all: false,
                },
                &seen,
            )
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert_eq!(result.changes[0].encoding_selection.as_deref(), selection);
    }
}

#[test]
fn mixed_segments_and_batches_preserve_untouched_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let writer = Writer::new(&WorldDir::under(directory.path()), "test").unwrap();
    let path = directory.path().join("mixed");
    let before = b"UTF8 \xc3\xa9\nGBK \xc4\xe3\xba\xc3\r\n";
    fs::write(&path, before).unwrap();
    let seen = observe(&path, Some("utf-8,gb18030"));
    writer
        .edit(
            EditRequest {
                file_path: path.clone(),
                old_string: "你好".into(),
                new_string: "您好".into(),
                replace_all: false,
            },
            &seen,
        )
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"UTF8 \xc3\xa9\nGBK \xc4\xfa\xba\xc3\r\n");
    fs::write(&path, before).unwrap();
    let failure = writer.edit_blocks(EditBlocksRequest {
        file_path: path.clone(), replace_all: false,
        input: "<<<<<<< SEARCH\n你好\n=======\n您好\n>>>>>>> REPLACE\n<<<<<<< SEARCH\nmissing\n=======\nX\n>>>>>>> REPLACE\n".into(),
    }, &seen).unwrap_err();
    assert!(failure.changes.is_empty());
    assert_eq!(fs::read(&path).unwrap(), before);
    let unchanged = writer
        .edit(
            EditRequest {
                file_path: path.clone(),
                old_string: "你好\r\n".into(),
                new_string: "你好\n".into(),
                replace_all: false,
            },
            &seen,
        )
        .unwrap();
    assert!(unchanged.changes.is_empty());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(writer.write(WriteRequest { file_path: path.clone(), content: "replacement".into() }, &seen).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}
