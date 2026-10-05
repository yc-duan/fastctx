//! A health check for the index lifecycle and its filesystem boundary.

use std::fs;
use std::path::Path;
use std::thread;

use fastctx_file_search::Search;
use fastctx_types::file_search::FileSearchResult;

fn ready(search: &Search, query: &str) -> Vec<String> {
    loop {
        if let FileSearchResult::Matches { ready: true, files, .. } = search.query(query.into(), 100).unwrap() {
            return files.into_iter().map(|file| file.relative).collect();
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn live_scope_and_lifetime() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join(".git/info")).unwrap();
    write(&root, ".gitignore", "ignored/\n*.cache\n!keep.cache\n");
    write(&root, ".git/info/exclude", "excluded.txt\n");
    write(&root, "src/.gitignore", "private.rs\n");
    for path in [
        "main.rs",
        "src/domain.rs",
        "src/main.rs",
        "src/模型 🦀.rs",
        "src/private.rs",
        ".hidden.rs",
        "node_modules/no.rs",
        "target/no.rs",
        "ignored/no.rs",
        "excluded.txt",
        "drop.cache",
        "keep.cache",
    ] {
        write(&root, path, "");
    }
    let search = Search::new(vec![root.to_str().unwrap().into(), root.join("src").to_str().unwrap().into()]).unwrap();
    let all = ready(&search, "");
    assert!(
        !all.iter().any(|path| path.contains("no.rs")
            || path.contains("private")
            || path == "excluded.txt"
            || path == "drop.cache"),
        "{all:?}"
    );
    assert!(all.contains(&"keep.cache".into()));
    assert!(all.contains(&".hidden.rs".into()));
    assert_eq!(ready(&search, "模型"), ["模型 🦀.rs"]);
    let main = ready(&search, "main");
    assert_eq!(main.len(), 3);
    assert!(main[0].ends_with("main.rs"));
    assert!(main[1].ends_with("main.rs"));
    write(&root, ".gitignore", "ignored/\n*.cache\n!keep.cache\nmain.rs\n");
    assert_eq!(ready(&search, "main.rs"), ["domain.rs"]);
    write(&root, "fresh.rs", "");
    while ready(&search, "fresh.rs").is_empty() {
        thread::yield_now();
    }
    fs::remove_file(root.join("fresh.rs")).unwrap();
    assert!(ready(&search, "fresh.rs").is_empty());
    drop(search);
    assert!(Search::new(vec!["relative".into()]).is_err());
    assert!(Search::new(vec![root.join("missing").to_str().unwrap().into()]).is_err());

    let replaced = temp.path().join("replaced");
    fs::create_dir(&replaced).unwrap();
    let search = Search::new(vec![replaced.to_str().unwrap().into()]).unwrap();
    assert!(ready(&search, "").is_empty());
    fs::remove_dir(&replaced).unwrap();
    fs::create_dir(&replaced).unwrap();
    write(&replaced, "first.rs", "");
    // Removing the root can produce a real NotFound response while the OS
    // delivers the replacement event. Once indexed again, its watch must
    // observe subsequent changes inside that new directory too.
    loop {
        if let Ok(FileSearchResult::Matches { ready: true, files, .. }) = search.query("first".into(), 10)
            && !files.is_empty()
        {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
    write(&replaced, "second.rs", "");
    while ready(&search, "second").is_empty() {
        thread::yield_now();
    }
    write(&replaced, "target", "");
    while ready(&search, "target").is_empty() {
        thread::yield_now();
    }
    write(&replaced, ".ignore", "[z-a]\n");
    while search.query("second".into(), 10).is_ok() {
        thread::yield_now();
    }
    fs::remove_file(replaced.join(".ignore")).unwrap();
    loop {
        if let Ok(FileSearchResult::Matches { ready: true, files, .. }) = search.query("second".into(), 10)
            && !files.is_empty()
        {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
}
