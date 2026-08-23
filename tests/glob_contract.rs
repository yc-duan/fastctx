mod common;

#[cfg(unix)]
use common::{error_text, glob_files, normalized};
#[cfg(unix)]
use fastctx::glob_filter::GlobPatterns;
#[cfg(unix)]
use fastctx::glob_tool::{FilterMode, GlobRequest};

#[cfg(unix)]
fn request(path: &std::path::Path, pattern: &str) -> GlobRequest {
    GlobRequest {
        pattern: GlobPatterns::One(pattern.to_string()),
        path: Some(normalized(path)),
        filter_mode: None,
        sort: None,
        output_mode: None,
        offset: None,
        limit: None,
    }
}

#[cfg(unix)]
#[test]
fn glob_rejects_the_real_one_hundred_thousand_and_first_match() {
    let temp = tempfile::tempdir().unwrap();
    for directory_index in 0..100 {
        let directory = temp.path().join(format!("batch-{directory_index:03}"));
        std::fs::create_dir(&directory).unwrap();
        for file_index in 0..1_000 {
            std::fs::File::create(directory.join(format!("item-{file_index:04}.hit"))).unwrap();
        }
    }
    std::fs::File::create(temp.path().join("overflow.hit")).unwrap();

    let mut input = request(temp.path(), "**/*.hit");
    input.filter_mode = Some(FilterMode::All);
    assert_eq!(
        error_text(glob_files(input)),
        "Too many matches: over 100000 files matched. Narrow the pattern or path."
    );
}
