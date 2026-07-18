mod common;

use common::{error_text, normalized, text, write};
use fastctx::read_tool::{ReadRequest, read_file};
use std::io::{Seek, SeekFrom, Write};

fn request(path: &std::path::Path) -> ReadRequest {
    ReadRequest {
        file_path: normalized(path),
        offset: None,
        limit: None,
        pages: None,
        pdf_mode: None,
        encoding: None,
        view: Some("hex".to_string()),
    }
}

#[test]
fn hex_view_has_exact_columns_paging_and_terminal_notes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bytes.bin");
    let mut bytes = b"0123456789ABCDEF".to_vec();
    bytes.extend([0x20, 0x7E, 0x1F, 0x7F]);
    write(&path, bytes);

    assert_eq!(
        text(read_file(request(&path))),
        concat!(
            "00000000  30 31 32 33 34 35 36 37  38 39 41 42 43 44 45 46  |0123456789ABCDEF|\n",
            "00000010  20 7e 1f 7f                                       | ~..|\n\n",
            "(Complete: reached end of file; lines 1-2 of 2 shown.)"
        )
    );

    let mut first = request(&path);
    first.limit = Some(1);
    assert_eq!(
        text(read_file(first)),
        concat!(
            "00000000  30 31 32 33 34 35 36 37  38 39 41 42 43 44 45 46  |0123456789ABCDEF|\n\n",
            "(Partial: line 1 of 2 shown. Continue with offset=2.)"
        )
    );

    let mut second = request(&path);
    second.offset = Some(2);
    second.limit = Some(1);
    assert_eq!(
        text(read_file(second)),
        concat!(
            "00000010  20 7e 1f 7f                                       | ~..|\n\n",
            "(Complete: reached end of file; line 2 of 2 shown.)"
        )
    );
}

#[test]
fn hex_view_handles_empty_files_offset_bounds_and_wide_offsets() {
    let temp = tempfile::tempdir().unwrap();
    let empty = temp.path().join("empty.bin");
    write(&empty, []);
    assert_eq!(
        text(read_file(request(&empty))),
        "Warning: the file exists but is empty."
    );

    let short = temp.path().join("short.bin");
    write(&short, b"one line");
    let mut beyond = request(&short);
    beyond.offset = Some(2);
    assert_eq!(
        text(read_file(beyond)),
        "Warning: the file has only 1 line, but offset=2 was requested."
    );

    let wide = temp.path().join("wide-offset.bin");
    let mut file = std::fs::File::create(&wide).unwrap();
    file.set_len(0x1_0000_0001).unwrap();
    file.seek(SeekFrom::Start(0x1_0000_0000)).unwrap();
    file.write_all(b"x").unwrap();
    drop(file);
    let mut tail = request(&wide);
    tail.offset = Some(268_435_457);
    tail.limit = Some(1);
    assert_eq!(
        text(read_file(tail)),
        concat!(
            "100000000  78                                                |x|\n\n",
            "(Complete: reached end of file; line 268435457 of 268435457 shown.)"
        )
    );
}

#[test]
fn hex_view_preempts_text_image_and_pdf_channels_without_loading_engines() {
    let temp = tempfile::tempdir().unwrap();
    let cases: &[(&str, &[u8], &str)] = &[
        ("plain.txt", b"text", "74 65 78 74"),
        ("image.png", b"\x89PNG\r\n\x1A\n", "89 50 4e 47 0d 0a 1a 0a"),
        ("document.pdf", b"%PDF-1.7\n", "25 50 44 46 2d 31 2e 37  0a"),
    ];
    for (name, bytes, expected_hex) in cases {
        let path = temp.path().join(name);
        write(&path, bytes);
        let output = text(read_file(request(&path)));
        assert!(output.starts_with("00000000  "), "{output}");
        assert!(output.contains(expected_hex), "{output}");
        assert!(output.ends_with("(Complete: reached end of file; line 1 of 1 shown.)"));
    }
}

#[test]
fn hex_view_rejects_invalid_values_and_mutually_exclusive_parameters() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plain.txt");
    write(&path, b"text");

    let mut invalid = request(&path);
    invalid.view = Some("bytes".to_string());
    assert_eq!(
        error_text(read_file(invalid)),
        "Invalid view value \"bytes\". Use \"auto\" or \"hex\"."
    );

    for (parameter, mutate) in [("pdf_mode", 0_u8), ("pages", 1_u8), ("encoding", 2_u8)] {
        let mut input = request(&path);
        match mutate {
            0 => input.pdf_mode = Some("text".to_string()),
            1 => input.pages = Some("1".to_string()),
            2 => input.encoding = Some("utf-8".to_string()),
            _ => unreachable!(),
        }
        assert_eq!(
            error_text(read_file(input)),
            format!("The {parameter} parameter cannot be combined with view=\"hex\".")
        );
    }

    let mut zero_offset = request(&path);
    zero_offset.offset = Some(0);
    assert_eq!(
        error_text(read_file(zero_offset)),
        "Invalid offset value: 0. Expected an integer >= 1."
    );
    let mut zero_limit = request(&path);
    zero_limit.limit = Some(0);
    assert_eq!(
        error_text(read_file(zero_limit)),
        "Invalid limit value: 0. Expected an integer >= 1."
    );
}

#[test]
fn binary_errors_identify_every_contract_type_and_always_offer_hex() {
    let temp = tempfile::tempdir().unwrap();
    let cases: &[(&str, &[u8], &str)] = &[
        ("archive.zip", b"PK\x03\x04\0", "a ZIP archive"),
        ("archive.gz", b"\x1F\x8B", "gzip data"),
        ("archive.7z", b"\x37\x7A\xBC\xAF\x27\x1C", "7-Zip data"),
        ("archive.zst", b"\x28\xB5\x2F\xFD", "Zstandard data"),
        ("program.elf", b"\x7FELF\0", "an ELF executable"),
        ("program.exe", b"MZ\0", "a Windows executable"),
        ("program.macho", b"\xFE\xED\xFA\xCF", "a Mach-O executable"),
        ("data.sqlite", b"SQLite format 3\0", "a SQLite database"),
        ("module.wasm", b"\0asm", "a WebAssembly module"),
    ];
    for (name, bytes, expected_type) in cases {
        let path = temp.path().join(name);
        write(&path, bytes);
        assert_eq!(
            error_text(read_file(ReadRequest {
                view: None,
                ..request(&path)
            })),
            format!(
                "Cannot read binary file as text: {} (looks like {expected_type}). Use view=\"hex\" to inspect its raw bytes.",
                normalized(&path)
            )
        );
    }

    let tar = temp.path().join("archive.tar");
    let mut tar_bytes = vec![0_u8; 262];
    tar_bytes[257..262].copy_from_slice(b"ustar");
    write(&tar, tar_bytes);
    assert_eq!(
        error_text(read_file(ReadRequest {
            view: None,
            ..request(&tar)
        })),
        format!(
            "Cannot read binary file as text: {} (looks like a tar archive). Use view=\"hex\" to inspect its raw bytes.",
            normalized(&tar)
        )
    );

    let unknown = temp.path().join("unknown.bin");
    write(&unknown, b"prefix\0payload");
    assert_eq!(
        error_text(read_file(ReadRequest {
            view: None,
            ..request(&unknown)
        })),
        format!(
            "Cannot read binary file as text: {}. Use view=\"hex\" to inspect its raw bytes.",
            normalized(&unknown)
        )
    );
}
