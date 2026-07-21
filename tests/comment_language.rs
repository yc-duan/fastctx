use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

#[test]
fn tracked_line_leading_code_comments_do_not_contain_cjk_text() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();

    for path in tracked_code_files(root) {
        let source = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("failed to read {}: {error}", path.display());
        });
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let mut in_block_comment = false;
        for (index, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            let comment = match extension {
                "rs" | "js" => leading_c_style_comment(trimmed, &mut in_block_comment),
                "ps1" => leading_powershell_comment(trimmed, &mut in_block_comment),
                _ => None,
            };
            if comment.is_some_and(contains_cjk) {
                violations.push(format!("{}:{}", path.display(), index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "CJK text is not allowed in line-leading code comments; translate: {}",
        violations.join(", ")
    );
}

#[test]
fn comment_guard_includes_cjk_punctuation_without_treating_literals_as_comments() {
    let mut in_block_comment = false;
    let comment = leading_c_style_comment("/// Codex 16k，FastCtx 13.6k。", &mut in_block_comment);
    assert!(comment.is_some_and(contains_cjk));

    let mut in_block_comment = false;
    let comment = leading_powershell_comment("# 日本語", &mut in_block_comment);
    assert!(comment.is_some_and(contains_cjk));

    let mut in_block_comment = false;
    assert!(leading_c_style_comment("let fixture = \"中文\";", &mut in_block_comment).is_none());
}

fn tracked_code_files(root: &Path) -> Vec<PathBuf> {
    match Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files", "-z", "--", "src", "tests", "build.rs", "scripts", "packages",
        ])
        .output()
    {
        Ok(output) if output.status.success() => output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| tracked_path(root, path))
            .filter(|path| is_code_file(path))
            .filter(|path| path.is_file())
            .collect(),
        _ => fallback_code_files(root),
    }
}

#[cfg(unix)]
fn tracked_path(root: &Path, path: &[u8]) -> PathBuf {
    root.join(OsString::from_vec(path.to_vec()))
}

#[cfg(not(unix))]
fn tracked_path(root: &Path, path: &[u8]) -> PathBuf {
    root.join(std::str::from_utf8(path).expect("git returned a non-UTF-8 tracked path"))
}

fn fallback_code_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for directory in ["src", "tests", "scripts", "packages"] {
        collect_code_files(&root.join(directory), &mut files);
    }
    let build_script = root.join("build.rs");
    if build_script.is_file() {
        files.push(build_script);
    }
    files.sort();
    files
}

fn collect_code_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let file_type = entry
            .file_type()
            .expect("failed to inspect directory entry");
        let path = entry.path();
        if file_type.is_dir() {
            collect_code_files(&path, files);
        } else if file_type.is_file() && is_code_file(&path) {
            files.push(path);
        }
    }
}

fn is_code_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("rs" | "js" | "ps1")
    )
}

fn leading_c_style_comment<'a>(line: &'a str, in_block: &mut bool) -> Option<&'a str> {
    if *in_block {
        if let Some(end) = line.find("*/") {
            *in_block = false;
            return Some(&line[..end]);
        }
        return Some(line);
    }
    if let Some(comment) = line.strip_prefix("//") {
        return Some(comment);
    }
    let comment = line.strip_prefix("/*")?;
    if let Some(end) = comment.find("*/") {
        Some(&comment[..end])
    } else {
        *in_block = true;
        Some(comment)
    }
}

fn leading_powershell_comment<'a>(line: &'a str, in_block: &mut bool) -> Option<&'a str> {
    if *in_block {
        if let Some(end) = line.find("#>") {
            *in_block = false;
            return Some(&line[..end]);
        }
        return Some(line);
    }
    if let Some(comment) = line.strip_prefix('#') {
        return Some(comment);
    }
    let comment = line.strip_prefix("<#")?;
    if let Some(end) = comment.find("#>") {
        Some(&comment[..end])
    } else {
        *in_block = true;
        Some(comment)
    }
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(
            character as u32,
            0x2E80..=0x303F
                | 0x3040..=0x30FF
                | 0x3100..=0x318F
                | 0x31A0..=0x31EF
                | 0x3200..=0x33FF
                | 0x3400..=0x4DBF
                | 0x4E00..=0x9FFF
                | 0xAC00..=0xD7AF
                | 0xF900..=0xFAFF
                | 0xFE30..=0xFE4F
                | 0xFF00..=0xFFEF
                | 0x20000..=0x2FA1F
        )
    })
}
