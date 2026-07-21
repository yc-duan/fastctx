//! On-disk schemas shared by fixture certification, capture, and finalization.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const ROOT_TOKEN: &str = "{{ROOT}}";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureSpec {
    pub schema: u32,
    pub token_budget: usize,
    pub files: Vec<FixtureFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureFile {
    pub path: String,
    pub text: String,
    pub mtime_unix_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseRequest {
    pub schema: u32,
    pub case_id: String,
    pub tool: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseEnvironment {
    pub schema: u32,
    pub variables: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct CaseFiles {
    pub directory: PathBuf,
    pub request: CaseRequest,
    pub environment: CaseEnvironment,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureReadback {
    pub schema: u32,
    pub fixture_tree_sha256: String,
    pub entries: Vec<FixtureEntry>,
    pub all_components_safe_utf8: bool,
    pub all_contents_strict_utf8: bool,
    pub forbidden_features_found: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureEntry {
    pub path: String,
    pub kind: String,
    pub sha256: String,
    pub bytes: u64,
    pub mtime_unix_seconds: u64,
    pub readonly: bool,
    pub hard_link_count: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeterminismCertificate {
    pub schema: u32,
    pub case_id: String,
    pub fixture_tree_sha256: String,
    pub all_components_safe_utf8: bool,
    pub all_contents_strict_utf8: bool,
    pub forbidden_features_found: Vec<String>,
    pub sort: SortCertificate,
    pub request_is_success_path: bool,
    pub immutable_capture_root: bool,
    pub budget: BudgetCertificate,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SortCertificate {
    pub kind: String,
    pub readback_keys: Vec<String>,
    pub all_total_keys_unique: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetCertificate {
    pub limit: usize,
    pub oracle_tokens: usize,
    pub slack: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseStability {
    pub schema: u32,
    pub case_id: String,
    pub platforms: BTreeMap<String, PlatformStability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub common_normalized_sha256: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformStability {
    pub oracles: BTreeMap<String, OracleStability>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OracleStability {
    pub runs: usize,
    pub raw_stdout_sha256: Vec<String>,
    pub normalized_sha256: Vec<String>,
    pub statuses: Vec<RunStatus>,
    pub replacement_counts: Vec<usize>,
    pub unique_normalized_hashes: usize,
    #[serde(default)]
    pub maximum_response_tokens: usize,
    #[serde(default)]
    pub minimum_budget_slack: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunStatus {
    pub exit_code: i32,
    pub is_error: bool,
    pub content_kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedMeta {
    pub schema: u32,
    pub case_id: String,
    pub is_error: bool,
    pub content_kind: String,
    pub replacement_count: usize,
    pub normalized_text_sha256: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureLog {
    pub schema: u32,
    pub captures: BTreeMap<String, CaptureLogEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureLogEntry {
    pub platform: String,
    pub oracle: String,
    pub fixture_root: String,
    pub binary_sha256: String,
    pub binary_size: u64,
    pub binary_version: String,
    #[serde(default)]
    pub capture_harness_sha256: String,
    #[serde(default)]
    pub capture_harness_size: u64,
    #[serde(default)]
    pub fixture_tree_sha256: String,
    #[serde(default)]
    pub immutable_readback_checks: usize,
    #[serde(default)]
    pub fresh_home_per_invocation: bool,
    #[serde(default)]
    pub environment_profile: String,
    #[serde(default)]
    pub isolated_git_parent: bool,
    pub runs_per_case: usize,
    pub seed: u64,
    pub case_order: Vec<String>,
    pub stdin_sha256: String,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
}

pub fn load_cases(assets: &Path) -> Result<Vec<CaseFiles>, String> {
    let root = assets.join("cases");
    let mut directories = fs::read_dir(&root)
        .map_err(|error| format!("cannot list {}: {error}", root.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("cannot read a case directory entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    directories.retain(|path| path.is_dir());
    directories.sort();
    let mut cases = Vec::with_capacity(directories.len());
    for directory in directories {
        let request: CaseRequest = read_json(&directory.join("request.json"))?;
        let environment: CaseEnvironment = read_json(&directory.join("env.json"))?;
        let directory_name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("case directory is not safe UTF-8: {}", directory.display()))?;
        if request.case_id != directory_name {
            return Err(format!(
                "case id {} does not match directory {directory_name}",
                request.case_id
            ));
        }
        if request.schema != 1 || environment.schema != 1 {
            return Err(format!("unsupported schema in case {}", request.case_id));
        }
        if !matches!(request.tool.as_str(), "grep" | "glob") {
            return Err(format!(
                "case {} uses unsupported tool {}",
                request.case_id, request.tool
            ));
        }
        cases.push(CaseFiles {
            directory,
            request,
            environment,
        });
    }
    if cases.is_empty() {
        return Err(format!("{} contains no cases", root.display()));
    }
    Ok(cases)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("cannot serialize {}: {error}", path.display()))?;
    bytes.push(b'\n');
    write_bytes(path, &bytes)
}

pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("capture")
    ));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| format!("cannot replace {}: {error}", path.display()))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        format!(
            "cannot move {} to {}: {error}",
            temporary.display(),
            path.display()
        )
    })
}

pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|bytes| sha256(&bytes))
        .map_err(|error| format!("cannot hash {}: {error}", path.display()))
}

pub fn canonical_display(path: &Path) -> Result<String, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("cannot canonicalize {}: {error}", path.display()))?;
    let mut display = canonical.to_string_lossy().replace('\\', "/");
    if let Some(rest) = display.strip_prefix("//?/UNC/") {
        display = format!("//{rest}");
    } else if let Some(rest) = display.strip_prefix("//?/") {
        display = rest.to_string();
    }
    Ok(display)
}

pub fn replace_root(text: &str, root: &str) -> Result<(String, usize), String> {
    if root.is_empty() || root == ROOT_TOKEN {
        return Err("fixture root replacement source is invalid".to_string());
    }
    let count = text.match_indices(root).count();
    Ok((text.replace(root, ROOT_TOKEN), count))
}

pub fn substitute_root(value: &mut serde_json::Value, root: &str) {
    match value {
        serde_json::Value::String(text) => {
            *text = text.replace(ROOT_TOKEN, root);
        }
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

#[cfg(test)]
mod tests {
    use super::{ROOT_TOKEN, replace_root, substitute_root};

    #[test]
    fn root_replacement_is_the_only_normalization() {
        let source = "C:/fastctx-v011/a.txt\r\nC:/fastctx-v011/b.txt";
        let (normalized, count) = replace_root(source, "C:/fastctx-v011").unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            normalized,
            format!("{ROOT_TOKEN}/a.txt\r\n{ROOT_TOKEN}/b.txt")
        );
    }

    #[test]
    fn request_root_substitution_visits_nested_values_only() {
        let mut value = serde_json::json!({
            "path": "{{ROOT}}/a.txt",
            "nested": ["{{ROOT}}", 1, false]
        });
        substitute_root(&mut value, "/fixture");
        assert_eq!(value["path"], "/fixture/a.txt");
        assert_eq!(value["nested"][0], "/fixture");
        assert_eq!(value["nested"][1], 1);
    }
}
