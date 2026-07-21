mod common;

use common::{cwd, error_text, glob_files, normalized, set_mtime, text, write};
use fastctx::glob_tool::{FilterMode, GlobRequest, SortMode};

fn request(path: &std::path::Path, pattern: &str) -> GlobRequest {
    GlobRequest {
        pattern: pattern.to_string(),
        path: Some(normalized(path)),
        filter_mode: None,
        sort: None,
        offset: None,
        limit: None,
    }
}

#[test]
fn glob_project_default_and_all_mode_have_exact_filtering_shapes() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join(".gitignore"), b"ignored.txt\n");
    let ignored = temp.path().join("ignored.txt");
    let hidden = temp.path().join(".hidden.txt");
    let git = temp.path().join(".git/HEAD");
    let gitignore = temp.path().join(".gitignore");
    write(&ignored, b"ignored");
    write(&hidden, b"hidden");
    write(&git, b"git");

    assert_eq!(
        text(glob_files(request(temp.path(), "**/*"))),
        format!(
            "{}\n{}\n\n(Complete: all 2 files shown.)",
            normalized(&gitignore),
            normalized(&hidden)
        )
    );

    let mut all = request(temp.path(), "**/*");
    all.filter_mode = Some(FilterMode::All);
    assert_eq!(
        text(glob_files(all)),
        format!(
            "{}\n{}\n{}\n{}\n\n(Complete: all 4 files shown.)",
            normalized(&git),
            normalized(&gitignore),
            normalized(&hidden),
            normalized(&ignored)
        )
    );
}

#[test]
fn glob_path_and_modified_sort_are_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    let alpha = temp.path().join("alpha.txt");
    let beta = temp.path().join("beta.txt");
    let gamma = temp.path().join("gamma.txt");
    write(&beta, b"b");
    write(&gamma, b"g");
    write(&alpha, b"a");
    set_mtime(&alpha, 1_700_000_001);
    set_mtime(&beta, 1_700_000_003);
    set_mtime(&gamma, 1_700_000_002);

    assert_eq!(
        text(glob_files(request(temp.path(), "*.txt"))),
        format!(
            "{}\n{}\n{}\n\n(Complete: all 3 files shown.)",
            normalized(&alpha),
            normalized(&beta),
            normalized(&gamma)
        )
    );

    let mut modified = request(temp.path(), "*.txt");
    modified.sort = Some(SortMode::Modified);
    assert_eq!(
        text(glob_files(modified)),
        format!(
            "{}\n{}\n{}\n\n(Complete: all 3 files shown.)",
            normalized(&beta),
            normalized(&gamma),
            normalized(&alpha)
        )
    );

    set_mtime(&alpha, 1_700_000_003);
    let mut tied = request(temp.path(), "*.txt");
    tied.sort = Some(SortMode::Modified);
    assert_eq!(
        text(glob_files(tied)),
        format!(
            "{}\n{}\n{}\n\n(Complete: all 3 files shown.)",
            normalized(&alpha),
            normalized(&beta),
            normalized(&gamma)
        )
    );
}

#[test]
fn glob_patterns_are_relative_to_the_root_and_star_never_crosses_a_separator() {
    let temp = tempfile::tempdir().unwrap();
    let root_file = temp.path().join("root.rs");
    let nested_file = temp.path().join("nested/child.rs");
    let deep_file = temp.path().join("nested/deep/grandchild.rs");
    write(&root_file, b"root");
    write(&nested_file, b"child");
    write(&deep_file, b"grandchild");

    assert_eq!(
        text(glob_files(request(temp.path(), "*.rs"))),
        format!(
            "{}\n\n(Complete: all 1 file shown.)",
            normalized(&root_file)
        )
    );
    assert_eq!(
        text(glob_files(request(temp.path(), "**/*.rs"))),
        format!(
            "{}\n{}\n{}\n\n(Complete: all 3 files shown.)",
            normalized(&nested_file),
            normalized(&deep_file),
            normalized(&root_file)
        )
    );
}

#[test]
fn glob_includes_file_symlinks_without_following_directory_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let search_root = temp.path().join("search");
    let external_file = temp.path().join("external.data");
    let external_directory = temp.path().join("external-directory");
    std::fs::create_dir_all(&search_root).unwrap();
    write(&external_file, b"linked file");
    write(
        &external_directory.join("nested.txt"),
        b"must not be followed",
    );

    let file_link = search_root.join("linked.txt");
    let directory_link = search_root.join("linked-directory");
    let broken_link = search_root.join("broken.txt");
    if !create_file_symlink(&external_file, &file_link) {
        return;
    }
    if !create_directory_symlink(&external_directory, &directory_link) {
        return;
    }
    if !create_file_symlink(&temp.path().join("missing.data"), &broken_link) {
        return;
    }

    for filter_mode in [FilterMode::Project, FilterMode::All] {
        let mut input = request(&search_root, "**/*.txt");
        input.filter_mode = Some(filter_mode);
        assert_eq!(
            text(glob_files(input)),
            format!(
                "{}/linked.txt\n\n(Complete: all 1 file shown.)",
                normalized(&search_root)
            )
        );
    }
}

#[cfg(unix)]
#[test]
fn glob_lists_non_utf8_filenames_canonically_without_dropping_legal_neighbors() {
    use std::os::unix::ffi::OsStringExt;

    let temp = tempfile::tempdir().unwrap();
    let invalid = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"bad-\xFF.txt".to_vec()));
    let legal = temp.path().join("legal.txt");
    // APFS rejects file names that are not valid UTF-8; the fixture is only
    // creatable on filesystems that accept arbitrary bytes, e.g. ext4 (2026-07-16).
    if std::fs::write(&invalid, b"invalid name").is_err() {
        eprintln!("skipping: this filesystem rejects non-UTF-8 file names");
        return;
    }
    write(&legal, b"legal name");

    let invalid_display = format!("{}/~fastctx~b6261642dff2e747874~", normalized(temp.path()));
    let mut input = request(temp.path(), "*");
    input.filter_mode = Some(FilterMode::All);
    assert_eq!(
        text(glob_files(input)),
        format!(
            "{}\n{}\n\n(Complete: all 2 files shown.)",
            normalized(&legal),
            invalid_display
        )
    );
}

#[test]
fn glob_pagination_partial_complete_and_offset_exhaustion_are_exact() {
    let temp = tempfile::tempdir().unwrap();
    let alpha = temp.path().join("alpha.txt");
    let beta = temp.path().join("beta.txt");
    let gamma = temp.path().join("gamma.txt");
    write(&alpha, b"a");
    write(&beta, b"b");
    write(&gamma, b"g");

    let mut first = request(temp.path(), "*.txt");
    first.limit = Some(2);
    assert_eq!(
        text(glob_files(first)),
        format!(
            "{}\n{}\n\n(Partial: files 1-2 of 3 shown. Continue with offset=2.)",
            normalized(&alpha),
            normalized(&beta)
        )
    );

    let mut last = request(temp.path(), "*.txt");
    last.offset = Some(2);
    last.limit = Some(2);
    assert_eq!(
        text(glob_files(last)),
        format!(
            "{}\n\n(Complete: file 3 of 3 shown; end of results.)",
            normalized(&gamma)
        )
    );

    let mut exhausted = request(temp.path(), "*.txt");
    exhausted.offset = Some(3);
    assert_eq!(
        text(glob_files(exhausted)),
        "(Complete: no files at offset=3; only 3 files exist.)"
    );
}

#[test]
fn glob_zero_and_single_results_use_terminal_singulars() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        text(glob_files(request(temp.path(), "*.rs"))),
        "(Complete: no files matched.)"
    );
    let file = temp.path().join("only.txt");
    write(&file, b"x");
    assert_eq!(
        text(glob_files(request(temp.path(), "*.txt"))),
        format!("{}\n\n(Complete: all 1 file shown.)", normalized(&file))
    );

    let mut exhausted = request(temp.path(), "*.txt");
    exhausted.offset = Some(1);
    assert_eq!(
        text(glob_files(exhausted)),
        "(Complete: no files at offset=1; only 1 file exists.)"
    );
}

#[test]
fn glob_invalid_inputs_and_non_directory_paths_use_exact_messages() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    assert_eq!(
        error_text(glob_files(request(&missing, "*"))),
        format!(
            "Path does not exist: {}\nNote: the session working directory is {}.",
            normalized(&missing),
            cwd()
        )
    );

    let file = temp.path().join("file.txt");
    write(&file, b"x");
    assert_eq!(
        error_text(glob_files(request(&file, "*"))),
        format!("Path is not a directory: {}", normalized(&file))
    );

    let error = error_text(glob_files(request(temp.path(), "[")));
    assert!(error.starts_with("Invalid glob pattern:"));
    assert!(error.ends_with("Use forms like \"**/*.rs\" or \"src/**/*.ts\"."));

    for limit in [0, 1_001] {
        let mut invalid_limit = request(temp.path(), "*");
        invalid_limit.limit = Some(limit);
        assert_eq!(
            error_text(glob_files(invalid_limit)),
            format!("Invalid limit value: {limit}. Expected an integer from 1 to 1000.")
        );
    }
}

#[test]
fn glob_relative_existing_path_gives_the_absolute_path_to_retry() {
    let mut input = request(std::path::Path::new("src"), "*.rs");
    input.path = Some("src".to_string());
    assert_eq!(
        error_text(glob_files(input)),
        format!(
            "Path does not exist: src\nNote: the session working directory is {}. Use the absolute path {}/src.",
            cwd(),
            cwd()
        )
    );
}

#[test]
fn glob_never_returns_directories() {
    let temp = tempfile::tempdir().unwrap();
    let nested_file = temp.path().join("directory/file.txt");
    write(&nested_file, b"x");
    assert_eq!(
        text(glob_files(request(temp.path(), "**/*"))),
        format!(
            "{}\n\n(Complete: all 1 file shown.)",
            normalized(&nested_file)
        )
    );
}

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

#[cfg(unix)]
fn create_file_symlink(target: &std::path::Path, link: &std::path::Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

#[cfg(unix)]
fn create_directory_symlink(target: &std::path::Path, link: &std::path::Path) -> bool {
    std::os::unix::fs::symlink(target, link).unwrap();
    true
}

#[cfg(windows)]
fn create_file_symlink(target: &std::path::Path, link: &std::path::Path) -> bool {
    create_windows_symlink(|| std::os::windows::fs::symlink_file(target, link))
}

#[cfg(windows)]
fn create_directory_symlink(target: &std::path::Path, link: &std::path::Path) -> bool {
    create_windows_symlink(|| std::os::windows::fs::symlink_dir(target, link))
}

#[cfg(windows)]
fn create_windows_symlink(create: impl FnOnce() -> std::io::Result<()>) -> bool {
    match create() {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("failed to create test symlink: {error}"),
    }
}
