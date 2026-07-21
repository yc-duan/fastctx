mod common;

use common::{McpSession, normalized};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/compat/v0_1_1");
const ROOT_TOKEN: &str = "{{ROOT}}";

#[derive(Deserialize)]
struct FixtureSpec {
    schema: u32,
    token_budget: usize,
    files: Vec<FixtureFile>,
}

#[derive(Deserialize)]
struct FixtureFile {
    path: String,
    text: String,
    mtime_unix_seconds: u64,
}

#[derive(Deserialize)]
struct CaseRequest {
    schema: u32,
    case_id: String,
    tool: String,
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct CaseEnvironment {
    schema: u32,
    variables: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct ExpectedMeta {
    schema: u32,
    case_id: String,
    is_error: bool,
    content_kind: String,
    replacement_count: usize,
    normalized_text_sha256: String,
}

#[test]
fn current_fastctx_matches_the_frozen_v011_ordinary_success_corpus() {
    let assets = Path::new(ASSETS);
    let spec: FixtureSpec = read_json(&assets.join("fixture-spec.json"));
    assert_eq!(spec.schema, 1);
    let temp = tempfile::Builder::new()
        .prefix("fastctx-v011-current-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap();
    fs::create_dir(temp.path().join(".git")).unwrap();
    let fixture_root = temp.path().join("fixture");
    materialize(&fixture_root, &spec);
    let display_root = normalized(&fixture_root);

    let mut case_directories = fs::read_dir(assets.join("cases"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    case_directories.sort();
    assert!(!case_directories.is_empty());
    for directory in case_directories {
        let request: CaseRequest = read_json(&directory.join("request.json"));
        let environment: CaseEnvironment = read_json(&directory.join("env.json"));
        let expected_meta: ExpectedMeta = read_json(&directory.join("expected.meta.json"));
        let expected_text = fs::read_to_string(directory.join("expected.text")).unwrap();
        assert_eq!(request.schema, 1, "{}", request.case_id);
        assert_eq!(environment.schema, 1, "{}", request.case_id);
        assert_eq!(expected_meta.schema, 1, "{}", request.case_id);
        assert_eq!(expected_meta.case_id, request.case_id);
        assert!(!expected_meta.is_error, "{}", request.case_id);
        assert_eq!(expected_meta.content_kind, "text", "{}", request.case_id);
        assert_eq!(
            expected_meta.normalized_text_sha256,
            hex::encode(Sha256::digest(expected_text.as_bytes())),
            "{}",
            request.case_id
        );
        assert_eq!(
            environment.variables.get("FASTCTX_TOKEN_BUDGET"),
            Some(&spec.token_budget.to_string()),
            "{}",
            request.case_id
        );

        let mut arguments = request.arguments;
        substitute_root(&mut arguments, &display_root);
        let case_home = temp.path().join(format!("home-{}", request.case_id));
        fs::create_dir(&case_home).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
        command.arg("serve");
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("FASTCTX_")
                || text.starts_with("LC_")
                || text.starts_with("GIT_")
                || text.starts_with("XDG_")
                || matches!(
                    text.as_ref(),
                    "LANG"
                        | "LANGUAGE"
                        | "TZ"
                        | "HOME"
                        | "USERPROFILE"
                        | "HOMEDRIVE"
                        | "HOMEPATH"
                        | "APPDATA"
                        | "LOCALAPPDATA"
                        | "TMPDIR"
                        | "TMP"
                        | "TEMP"
                )
            {
                command.env_remove(name);
            }
        }
        command
            .env("HOME", &case_home)
            .env("USERPROFILE", &case_home)
            .env("APPDATA", &case_home)
            .env("LOCALAPPDATA", &case_home)
            .env("XDG_CONFIG_HOME", &case_home)
            .env("XDG_CACHE_HOME", &case_home)
            .env("XDG_DATA_HOME", &case_home)
            .env("TMPDIR", &case_home)
            .env("TMP", &case_home)
            .env("TEMP", &case_home)
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("TZ", "UTC")
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env("FASTCTX_NO_PARENT_WATCH", "1");
        for (name, value) in environment.variables {
            command.env(name, value);
        }
        let mut session = McpSession::start(command);
        let response = session.call(&request.tool, arguments);
        assert_eq!(
            response.get("jsonrpc").and_then(serde_json::Value::as_str),
            Some("2.0"),
            "{}",
            request.case_id
        );
        assert!(response.get("error").is_none(), "{}", request.case_id);
        let result = response
            .get("result")
            .and_then(serde_json::Value::as_object)
            .unwrap_or_else(|| panic!("{}: response has no result object", request.case_id));
        assert_eq!(
            result.get("isError").and_then(serde_json::Value::as_bool),
            Some(false),
            "{}",
            request.case_id
        );
        let content = result
            .get("content")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("{}: response has no content array", request.case_id));
        assert_eq!(content.len(), 1, "{}", request.case_id);
        assert_eq!(
            content[0].get("type").and_then(serde_json::Value::as_str),
            Some(expected_meta.content_kind.as_str()),
            "{}",
            request.case_id
        );
        let text = content[0]
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("{}: response content is not text", request.case_id));
        let replacement_count = text.match_indices(&display_root).count();
        let normalized = text.replace(&display_root, ROOT_TOKEN);
        assert_eq!(
            replacement_count, expected_meta.replacement_count,
            "{}",
            request.case_id
        );
        assert_eq!(normalized, expected_text, "{}", request.case_id);
        session.close();
    }
}

fn materialize(root: &Path, spec: &FixtureSpec) {
    fs::create_dir(root).unwrap();
    for file in &spec.files {
        let path = root.join(&file.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, file.text.as_bytes()).unwrap();
        set_regular_permissions(&path);
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(file.mtime_unix_seconds);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }
}

#[cfg(unix)]
fn set_regular_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
}

#[cfg(not(unix))]
#[allow(clippy::permissions_set_readonly_false)]
fn set_regular_permissions(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).unwrap();
}

fn substitute_root(value: &mut serde_json::Value, root: &str) {
    match value {
        serde_json::Value::String(text) => *text = text.replace(ROOT_TOKEN, root),
        serde_json::Value::Array(values) => {
            for value in values {
                substitute_root(value, root);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                substitute_root(value, root);
            }
        }
        _ => {}
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> T {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
