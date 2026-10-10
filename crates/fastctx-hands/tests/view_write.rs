use std::fs;
use std::path::PathBuf;

use fastctx_hands::view::{Options, view};
use fastctx_hands::write::{WriteRequest, Writer};
use fastctx_types::files::{SeenFile, SeenFiles, SeenVersion};
use fastctx_types::layout::WorldDir;

#[test]
fn relative_view_prevents_stale_absolute_overwrite() {
    // Avoid changing process-wide cwd: the fixture is beneath the test's cwd.
    let cwd = std::env::current_dir().unwrap();
    let directory = tempfile::tempdir_in(&cwd).unwrap();
    let file = directory.path().join("observed.txt");
    let relative = file.strip_prefix(&cwd).unwrap().to_path_buf();
    let writer = Writer::new(&WorldDir::under(&directory.path().join("world")), "test-machine").unwrap();
    for lines in [None, Some((1, 1))] {
        fs::write(&file, "original\nsecond\n").unwrap();
        let observed = view(std::slice::from_ref(&relative), &Options { lines, ..Options::default() }, &mut Vec::new())
            .unwrap()
            .seen;
        assert_eq!(observed.len(), 1);
        let observed = &observed[0];
        let key = PathBuf::from(&observed.path);
        assert_eq!(key, fs::canonicalize(&file).unwrap());
        let version = match &observed.sha256 {
            Some(sha256) => SeenVersion::Full { sha256: sha256.clone() },
            None => SeenVersion::Partial { size: observed.size, modified: observed.modified.unwrap().into() },
        };
        assert!(observed.sha256.is_some());
        let seen = SeenFiles::from([(key, SeenFile { version, encoding: observed.encoding_selection.clone() })]);
        fs::write(&file, "someone else's new contents\n").unwrap();
        let error = writer
            .write(WriteRequest { encoding: None, file_path: file.clone(), content: "overwrite\n".into() }, &seen)
            .unwrap_err();
        assert!(error.message.contains("Whole-file overwrite refused"), "{}", error.message);
        assert_eq!(fs::read_to_string(&file).unwrap(), "someone else's new contents\n");
    }
}
