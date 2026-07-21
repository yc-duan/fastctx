//! Stability capture, common-golden reconciliation, and manifest construction.

use crate::fixture::FixtureGuard;
use crate::mcp;
use crate::model::{
    BudgetCertificate, CaptureLog, CaptureLogEntry, CaseFiles, CaseStability,
    DeterminismCertificate, ExpectedMeta, FixtureReadback, FixtureSpec, OracleStability,
    PlatformStability, RunStatus, SortCertificate, canonical_display, load_cases, read_json,
    replace_root, sha256, sha256_file, substitute_root, write_bytes, write_json,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct CaptureOptions {
    pub binary: PathBuf,
    pub assets: PathBuf,
    pub fixture_root: PathBuf,
    pub platform: String,
    pub oracle: String,
    pub runs: usize,
    pub seed: u64,
    pub timeout: Duration,
}

#[derive(Default)]
struct Accumulator {
    raw_stdout_sha256: Vec<String>,
    normalized_sha256: Vec<String>,
    statuses: Vec<RunStatus>,
    replacement_counts: Vec<usize>,
    maximum_response_tokens: usize,
}

const AUDITED_PLATFORMS: [&str; 4] = ["windows-x64", "linux-x64", "macos-x64", "macos-arm64"];
const EXPECTED_COMMIT: &str = "64a6a45f88e65a2c0305e36673fa5e3f99d95384";
const EXPECTED_TREE: &str = "21efd928f328a5adb063f182fb8655626889fb3a";
const EXPECTED_SOURCE_ARCHIVE: &str =
    "dc314bfb011c9bfb12f8c55bb639e47e1fd1053e040c980eed9c0c49b43f7dd3";
const EXPECTED_CARGO_LOCK: &str =
    "ac793ebb95f5f62f62f44db067d0c1ef0779a618aef25cbc490ca35f1ec0e33f";
const SOURCE_BUILD_COMMAND: &str =
    "FASTCTX_DISTRIBUTION=github-release cargo build --locked --release";
const CAPTURE_ENVIRONMENT_PROFILE: &str = "fresh-home-c-utf8-utc-no-git-v1";
const REQUIRED_CASE_IDS: [&str; 23] = [
    "glob-all-files",
    "glob-modified-page",
    "glob-path-many-page-one",
    "glob-path-many-page-two",
    "glob-path-one",
    "glob-path-zero",
    "grep-glob-filter",
    "grep-ignore-project",
    "grep-multi-content-page",
    "grep-multi-count",
    "grep-multi-files",
    "grep-multi-summary",
    "grep-single-content-context",
    "grep-single-count",
    "grep-single-files",
    "grep-single-multiline-crlf",
    "grep-single-only-matching",
    "grep-single-page-one",
    "grep-single-page-two",
    "grep-single-summary",
    "grep-single-zero-content",
    "grep-single-zero-files",
    "grep-type-rust",
];

pub fn run(options: CaptureOptions) -> Result<(), String> {
    validate_options(&options)?;
    let version = mcp::binary_version(&options.binary, options.timeout)?;
    if version != "fastctx 0.1.1" {
        return Err(format!(
            "oracle binary version must be exactly `fastctx 0.1.1`, got {version:?}"
        ));
    }
    let binary_bytes = fs::read(&options.binary).map_err(|error| {
        format!(
            "cannot read oracle binary {}: {error}",
            options.binary.display()
        )
    })?;
    let capture_executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve the capture executable: {error}"))?;
    let capture_executable_bytes = fs::read(&capture_executable).map_err(|error| {
        format!(
            "cannot read capture executable {}: {error}",
            capture_executable.display()
        )
    })?;
    let spec: FixtureSpec = read_json(&options.assets.join("fixture-spec.json"))?;
    let fixture = FixtureGuard::materialize(&options.fixture_root, &spec)?;
    let readback = fixture.verify_immutable()?;
    let display_root = canonical_display(fixture.root())?;
    if !display_root.is_ascii() {
        return Err(format!(
            "canonical fixture root must be ASCII, got {display_root:?}"
        ));
    }
    let cases = load_cases(&options.assets)?;
    validate_cases(&cases, &spec)?;
    validate_root_noncollision(&cases, &spec, &display_root)?;
    let order = shuffled_case_indices(cases.len(), options.seed);
    let ordered_case_ids = order
        .iter()
        .map(|index| cases[*index].request.case_id.clone())
        .collect::<Vec<_>>();
    let mut accumulators = cases
        .iter()
        .map(|case| (case.request.case_id.clone(), Accumulator::default()))
        .collect::<BTreeMap<_, _>>();
    let mut representative_stdin = Vec::new();
    let mut representative_stdout = Vec::new();
    let mut representative_stderr = Vec::new();

    for run_index in 0..options.runs {
        for case_index in &order {
            let case = &cases[*case_index];
            let mut arguments = case.request.arguments.clone();
            substitute_root(&mut arguments, &display_root);
            let home = CaptureHome::create(fixture.root(), &options.platform, &options.oracle)?;
            let invocation = mcp::invoke(
                &options.binary,
                fixture.root(),
                home.path(),
                &case.environment.variables,
                &case.request.tool,
                arguments,
                options.timeout,
            )?;
            home.finish()?;
            if invocation.is_error || invocation.content_kind != "text" {
                return Err(format!(
                    "case {} is not an ordinary successful text response: isError={}, kind={}",
                    case.request.case_id, invocation.is_error, invocation.content_kind
                ));
            }
            let response_tokens = tiktoken_rs::o200k_base_singleton()
                .encode_ordinary(&invocation.text)
                .len();
            let (normalized, replacement_count) = replace_root(&invocation.text, &display_root)?;
            let accumulator = accumulators
                .get_mut(&case.request.case_id)
                .expect("every loaded case has an accumulator");
            accumulator
                .raw_stdout_sha256
                .push(sha256(&invocation.stdout));
            accumulator
                .normalized_sha256
                .push(sha256(normalized.as_bytes()));
            accumulator.statuses.push(RunStatus {
                exit_code: invocation.exit_status.code().unwrap_or(-1),
                is_error: invocation.is_error,
                content_kind: invocation.content_kind,
            });
            accumulator.replacement_counts.push(replacement_count);
            accumulator.maximum_response_tokens =
                accumulator.maximum_response_tokens.max(response_tokens);
            if run_index == 0 {
                representative_stdin.extend_from_slice(&invocation.stdin);
                representative_stdout.extend_from_slice(&invocation.stdout);
                representative_stderr.extend_from_slice(&invocation.stderr);
            }
            fixture.verify_immutable()?;
        }
    }

    for case in &cases {
        let accumulator = accumulators
            .remove(&case.request.case_id)
            .expect("every loaded case has an accumulator");
        let unique = accumulator
            .normalized_sha256
            .iter()
            .collect::<BTreeSet<_>>()
            .len();
        if unique != 1 {
            return Err(format!(
                "case {} is ineligible: {} normalized hashes across {} runs",
                case.request.case_id, unique, options.runs
            ));
        }
        if accumulator
            .replacement_counts
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != 1
        {
            return Err(format!(
                "case {} produced unstable root replacement counts",
                case.request.case_id
            ));
        }
        let oracle_tokens = accumulator.maximum_response_tokens;
        let slack = spec.token_budget.saturating_sub(oracle_tokens);
        if slack < 256 {
            return Err(format!(
                "case {} has only {slack} tokens of budget slack",
                case.request.case_id
            ));
        }
        merge_certificate(case, &readback, spec.token_budget, oracle_tokens, slack)?;
        merge_stability(
            case,
            &options.platform,
            &options.oracle,
            OracleStability {
                runs: options.runs,
                raw_stdout_sha256: accumulator.raw_stdout_sha256,
                normalized_sha256: accumulator.normalized_sha256,
                statuses: accumulator.statuses,
                replacement_counts: accumulator.replacement_counts,
                unique_normalized_hashes: unique,
                maximum_response_tokens: oracle_tokens,
                minimum_budget_slack: slack,
            },
        )?;
    }

    let final_readback = fixture.verify_immutable()?;

    let raw_directory = options.assets.join("raw").join(&options.platform);
    let stdin_path = raw_directory.join(format!("{}.stdin.jsonl", options.oracle));
    let stdout_path = raw_directory.join(format!("{}.stdout.jsonl", options.oracle));
    let stderr_path = raw_directory.join(format!("{}.stderr.bin", options.oracle));
    write_bytes(&stdin_path, &representative_stdin)?;
    write_bytes(&stdout_path, &representative_stdout)?;
    write_bytes(&stderr_path, &representative_stderr)?;
    merge_capture_log(
        &options,
        &display_root,
        sha256(&binary_bytes),
        binary_bytes.len() as u64,
        version,
        sha256(&capture_executable_bytes),
        capture_executable_bytes.len() as u64,
        final_readback.fixture_tree_sha256,
        2 + options.runs * cases.len(),
        ordered_case_ids,
        sha256(&representative_stdin),
        sha256(&representative_stdout),
        sha256(&representative_stderr),
    )?;
    write_oracle_binary_hashes(&options.assets)?;
    fixture.finish()?;
    Ok(())
}

fn validate_options(options: &CaptureOptions) -> Result<(), String> {
    if options.runs != 32 {
        return Err(format!(
            "the audited v0.1.1 ceremony requires exactly 32 runs, got {}",
            options.runs
        ));
    }
    if !options.binary.is_absolute() || !options.binary.is_file() {
        return Err(format!(
            "oracle binary must be an absolute regular-file path: {}",
            options.binary.display()
        ));
    }
    if !options.assets.is_dir() {
        return Err(format!(
            "compatibility asset directory does not exist: {}",
            options.assets.display()
        ));
    }
    if !AUDITED_PLATFORMS.contains(&options.platform.as_str()) {
        return Err(format!("invalid platform id: {}", options.platform));
    }
    if !matches!(options.oracle.as_str(), "source-built" | "release") {
        return Err(format!("invalid oracle kind: {}", options.oracle));
    }
    Ok(())
}

fn validate_cases(cases: &[CaseFiles], spec: &FixtureSpec) -> Result<(), String> {
    let actual_case_ids = cases
        .iter()
        .map(|case| case.request.case_id.as_str())
        .collect::<Vec<_>>();
    if actual_case_ids != REQUIRED_CASE_IDS {
        return Err("case inventory differs from the audited v0.1.1 matrix".to_string());
    }
    let budget = spec.token_budget.to_string();
    for case in cases {
        validate_request_schema(case)?;
        let serialized = serde_json::to_string(&case.request.arguments)
            .map_err(|error| format!("cannot inspect case {}: {error}", case.request.case_id))?;
        if !serialized.contains(crate::model::ROOT_TOKEN) {
            return Err(format!(
                "case {} request does not contain the explicit root token",
                case.request.case_id
            ));
        }
        let expected_environment = [
            ("FASTCTX_GLOB_TOKEN_BUDGET".to_string(), budget.clone()),
            ("FASTCTX_GREP_TOKEN_BUDGET".to_string(), budget.clone()),
            ("FASTCTX_TOKEN_BUDGET".to_string(), budget.clone()),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        if case.environment.variables != expected_environment {
            return Err(format!(
                "case {} does not set the exact isolated token-budget environment {}",
                case.request.case_id, spec.token_budget
            ));
        }
    }
    Ok(())
}

fn validate_request_schema(case: &CaseFiles) -> Result<(), String> {
    let arguments = case.request.arguments.as_object().ok_or_else(|| {
        format!(
            "case {} arguments must be a JSON object",
            case.request.case_id
        )
    })?;
    let allowed: BTreeSet<&str> = match case.request.tool.as_str() {
        "glob" => ["pattern", "path", "filter_mode", "sort", "offset", "limit"]
            .into_iter()
            .collect(),
        "grep" => [
            "pattern",
            "path",
            "glob",
            "type",
            "output_mode",
            "case_insensitive",
            "line_numbers",
            "only_matching",
            "before_context",
            "after_context",
            "context",
            "multiline",
            "head_limit",
            "offset",
        ]
        .into_iter()
        .collect(),
        _ => return Err(format!("unsupported tool in case {}", case.request.case_id)),
    };
    if let Some(unknown) = arguments
        .keys()
        .find(|name| !allowed.contains(name.as_str()))
    {
        return Err(format!(
            "case {} contains unknown v0.1.1 argument {unknown}",
            case.request.case_id
        ));
    }
    if arguments
        .get("pattern")
        .and_then(|value| value.as_str())
        .is_none()
    {
        return Err(format!(
            "case {} has no string pattern",
            case.request.case_id
        ));
    }
    for name in ["path", "glob", "type"] {
        if arguments
            .get(name)
            .is_some_and(|value| value.as_str().is_none())
        {
            return Err(format!(
                "case {} argument {name} is not a string",
                case.request.case_id
            ));
        }
    }
    for name in [
        "case_insensitive",
        "line_numbers",
        "only_matching",
        "multiline",
    ] {
        if arguments
            .get(name)
            .is_some_and(|value| value.as_bool().is_none())
        {
            return Err(format!(
                "case {} argument {name} is not boolean",
                case.request.case_id
            ));
        }
    }
    for name in [
        "before_context",
        "after_context",
        "context",
        "head_limit",
        "offset",
        "limit",
    ] {
        if arguments
            .get(name)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(format!(
                "case {} argument {name} is not a nonnegative integer",
                case.request.case_id
            ));
        }
    }
    if arguments
        .get("limit")
        .and_then(|value| value.as_u64())
        .is_some_and(|limit| !(1..=1_000).contains(&limit))
    {
        return Err(format!(
            "case {} glob limit is invalid",
            case.request.case_id
        ));
    }
    for (name, choices) in [
        ("filter_mode", &["project", "all"][..]),
        ("sort", &["path", "modified"][..]),
        (
            "output_mode",
            &["content", "files_with_matches", "count", "summary"][..],
        ),
    ] {
        if let Some(value) = arguments.get(name) {
            let Some(value) = value.as_str() else {
                return Err(format!(
                    "case {} argument {name} is not a string",
                    case.request.case_id
                ));
            };
            if !choices.contains(&value) {
                return Err(format!(
                    "case {} argument {name} has unsupported value {value}",
                    case.request.case_id
                ));
            }
        }
    }
    Ok(())
}

fn validate_root_noncollision(
    cases: &[CaseFiles],
    spec: &FixtureSpec,
    display_root: &str,
) -> Result<(), String> {
    for file in &spec.files {
        if file.text.contains(display_root) || file.text.contains(crate::model::ROOT_TOKEN) {
            return Err(format!(
                "fixture file {} collides with the audited root replacement namespace",
                file.path
            ));
        }
    }
    for case in cases {
        let request = serde_json::to_string(&case.request.arguments)
            .map_err(|error| format!("cannot inspect case {}: {error}", case.request.case_id))?;
        if request.contains(display_root) {
            return Err(format!(
                "case {} contains the concrete fixture root before substitution",
                case.request.case_id
            ));
        }
        if case
            .environment
            .variables
            .values()
            .any(|value| value.contains(display_root) || value.contains(crate::model::ROOT_TOKEN))
        {
            return Err(format!(
                "case {} environment collides with the root replacement namespace",
                case.request.case_id
            ));
        }
    }
    Ok(())
}

fn shuffled_case_indices(length: usize, mut state: u64) -> Vec<usize> {
    let mut indices = (0..length).collect::<Vec<_>>();
    for cursor in (1..length).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let selected = (state as usize) % (cursor + 1);
        indices.swap(cursor, selected);
    }
    indices
}

fn merge_stability(
    case: &CaseFiles,
    platform: &str,
    oracle: &str,
    oracle_stability: OracleStability,
) -> Result<(), String> {
    let path = case.directory.join("stability.json");
    let mut stability = if path.exists() {
        read_json::<CaseStability>(&path)?
    } else {
        CaseStability {
            schema: 1,
            case_id: case.request.case_id.clone(),
            platforms: BTreeMap::new(),
            common_normalized_sha256: None,
        }
    };
    if stability.schema != 1 || stability.case_id != case.request.case_id {
        return Err(format!(
            "stability ledger identity mismatch for {}",
            case.request.case_id
        ));
    }
    stability
        .platforms
        .entry(platform.to_string())
        .or_insert_with(PlatformStability::default)
        .oracles
        .insert(oracle.to_string(), oracle_stability);
    stability.common_normalized_sha256 = None;
    write_json(&path, &stability)
}

fn merge_certificate(
    case: &CaseFiles,
    readback: &FixtureReadback,
    limit: usize,
    oracle_tokens: usize,
    slack: usize,
) -> Result<(), String> {
    let sort_kind = match case.request.tool.as_str() {
        "glob"
            if case
                .request
                .arguments
                .get("sort")
                .and_then(|value| value.as_str())
                == Some("modified") =>
        {
            "modified_then_native_path"
        }
        "glob" => "native_path",
        "grep"
            if case
                .request
                .arguments
                .get("path")
                .and_then(|value| value.as_str())
                .is_some_and(|path| path == crate::model::ROOT_TOKEN) =>
        {
            "modified_then_native_path"
        }
        "grep" => "single_file",
        _ => return Err(format!("unsupported tool in case {}", case.request.case_id)),
    };
    let mut file_entries = readback
        .entries
        .iter()
        .filter(|entry| entry.kind == "file")
        .collect::<Vec<_>>();
    if sort_kind == "modified_then_native_path" {
        file_entries.sort_by(|left, right| {
            right
                .mtime_unix_seconds
                .cmp(&left.mtime_unix_seconds)
                .then_with(|| left.path.as_bytes().cmp(right.path.as_bytes()))
        });
    } else {
        file_entries.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    }
    let readback_keys = file_entries
        .iter()
        .map(|entry| {
            format!(
                "mtime={:020};path_hex={}",
                entry.mtime_unix_seconds,
                hex::encode(entry.path.as_bytes())
            )
        })
        .collect::<Vec<_>>();
    let all_total_keys_unique =
        readback_keys.iter().collect::<BTreeSet<_>>().len() == readback_keys.len();
    if !all_total_keys_unique {
        return Err(format!(
            "case {} has duplicate total sort keys",
            case.request.case_id
        ));
    }
    let path = case.directory.join("determinism-certificate.json");
    let mut certificate = DeterminismCertificate {
        schema: 1,
        case_id: case.request.case_id.clone(),
        fixture_tree_sha256: readback.fixture_tree_sha256.clone(),
        all_components_safe_utf8: readback.all_components_safe_utf8,
        all_contents_strict_utf8: readback.all_contents_strict_utf8,
        forbidden_features_found: readback.forbidden_features_found.clone(),
        sort: SortCertificate {
            kind: sort_kind.to_string(),
            readback_keys,
            all_total_keys_unique,
        },
        request_is_success_path: true,
        immutable_capture_root: true,
        budget: BudgetCertificate {
            limit,
            oracle_tokens,
            slack,
        },
    };
    if path.exists() {
        let previous: DeterminismCertificate = read_json(&path)?;
        if previous.schema != certificate.schema
            || previous.case_id != certificate.case_id
            || previous.fixture_tree_sha256 != certificate.fixture_tree_sha256
            || previous.all_components_safe_utf8 != certificate.all_components_safe_utf8
            || previous.all_contents_strict_utf8 != certificate.all_contents_strict_utf8
            || previous.forbidden_features_found != certificate.forbidden_features_found
            || previous.sort.kind != certificate.sort.kind
            || previous.sort.readback_keys != certificate.sort.readback_keys
            || previous.sort.all_total_keys_unique != certificate.sort.all_total_keys_unique
            || !previous.request_is_success_path
            || !previous.immutable_capture_root
            || previous.budget.limit != limit
        {
            return Err(format!(
                "existing determinism certificate disagrees for {}",
                case.request.case_id
            ));
        }
        certificate.budget.oracle_tokens = previous
            .budget
            .oracle_tokens
            .max(certificate.budget.oracle_tokens);
        certificate.budget.slack = limit.saturating_sub(certificate.budget.oracle_tokens);
        if certificate.budget.slack < 256 {
            return Err(format!(
                "case {} has only {} tokens of cross-arm budget slack",
                case.request.case_id, certificate.budget.slack
            ));
        }
    }
    write_json(&path, &certificate)
}

#[allow(clippy::too_many_arguments)]
fn merge_capture_log(
    options: &CaptureOptions,
    display_root: &str,
    binary_sha256: String,
    binary_size: u64,
    binary_version: String,
    capture_harness_sha256: String,
    capture_harness_size: u64,
    fixture_tree_sha256: String,
    immutable_readback_checks: usize,
    case_order: Vec<String>,
    stdin_sha256: String,
    stdout_sha256: String,
    stderr_sha256: String,
) -> Result<(), String> {
    let path = options.assets.join("provenance/capture-log.json");
    let mut log = if path.exists() {
        read_json::<CaptureLog>(&path)?
    } else {
        CaptureLog {
            schema: 1,
            captures: BTreeMap::new(),
        }
    };
    if log.schema != 1 {
        return Err(format!("unsupported capture log schema {}", log.schema));
    }
    log.captures.insert(
        format!("{}/{}", options.platform, options.oracle),
        CaptureLogEntry {
            platform: options.platform.clone(),
            oracle: options.oracle.clone(),
            fixture_root: display_root.to_string(),
            binary_sha256,
            binary_size,
            binary_version,
            capture_harness_sha256,
            capture_harness_size,
            fixture_tree_sha256,
            immutable_readback_checks,
            fresh_home_per_invocation: true,
            environment_profile: CAPTURE_ENVIRONMENT_PROFILE.to_string(),
            isolated_git_parent: true,
            runs_per_case: options.runs,
            seed: options.seed,
            case_order,
            stdin_sha256,
            stdout_sha256,
            stderr_sha256,
        },
    );
    write_json(&path, &log)
}

fn write_oracle_binary_hashes(assets: &Path) -> Result<(), String> {
    let log: CaptureLog = read_json(&assets.join("provenance/capture-log.json"))?;
    let mut lines = String::new();
    for (name, entry) in log.captures {
        lines.push_str(&format!("{}  {}\n", entry.binary_sha256, name));
    }
    write_bytes(
        &assets.join("provenance/oracle-binaries.sha256"),
        lines.as_bytes(),
    )
}

pub fn finalize(assets: &Path, platforms: &[String]) -> Result<(), String> {
    if platforms.iter().map(String::as_str).collect::<Vec<_>>() != AUDITED_PLATFORMS {
        return Err(format!(
            "finalization requires the canonical audited platform order: {}",
            AUDITED_PLATFORMS.join(", ")
        ));
    }
    let fixture_spec: FixtureSpec = read_json(&assets.join("fixture-spec.json"))?;
    let cases = load_cases(assets)?;
    validate_cases(&cases, &fixture_spec)?;
    let fixture_readback = derive_fixture_readback(&fixture_spec)?;
    let capture_log: CaptureLog = read_json(&assets.join("provenance/capture-log.json"))?;
    validate_capture_log(
        &capture_log,
        platforms,
        &cases,
        &fixture_readback.fixture_tree_sha256,
    )?;
    let mut common_hashes = BTreeMap::new();

    for case in &cases {
        let path = case.directory.join("stability.json");
        let mut stability: CaseStability = read_json(&path)?;
        let mut hashes = BTreeSet::new();
        let mut maximum_response_tokens = 0;
        for platform in platforms {
            let platform_stability = stability.platforms.get(platform).ok_or_else(|| {
                format!(
                    "case {} has no {platform} stability ledger",
                    case.request.case_id
                )
            })?;
            for oracle in ["source-built", "release"] {
                let ledger = platform_stability.oracles.get(oracle).ok_or_else(|| {
                    format!(
                        "case {} has no {platform}/{oracle} stability ledger",
                        case.request.case_id
                    )
                })?;
                validate_ledger(
                    &case.request.case_id,
                    platform,
                    oracle,
                    ledger,
                    fixture_spec.token_budget,
                )?;
                hashes.insert(ledger.normalized_sha256[0].clone());
                maximum_response_tokens =
                    maximum_response_tokens.max(ledger.maximum_response_tokens);
            }
        }
        if hashes.len() != 1 {
            return Err(format!(
                "case {} disagrees across platform/oracle arms: {hashes:?}",
                case.request.case_id
            ));
        }
        let common_hash = hashes.into_iter().next().expect("one hash was required");
        stability.common_normalized_sha256 = Some(common_hash.clone());
        write_json(&path, &stability)?;
        let certificate_path = case.directory.join("determinism-certificate.json");
        let mut certificate: DeterminismCertificate = read_json(&certificate_path)?;
        if certificate.schema != 1
            || certificate.case_id != case.request.case_id
            || certificate.fixture_tree_sha256 != fixture_readback.fixture_tree_sha256
            || !certificate.all_components_safe_utf8
            || !certificate.all_contents_strict_utf8
            || !certificate.forbidden_features_found.is_empty()
            || !certificate.request_is_success_path
            || !certificate.immutable_capture_root
            || !certificate.sort.all_total_keys_unique
            || certificate.budget.limit != fixture_spec.token_budget
        {
            return Err(format!(
                "determinism certificate is incomplete or disagrees for {}",
                case.request.case_id
            ));
        }
        certificate.budget.oracle_tokens = maximum_response_tokens;
        certificate.budget.slack = fixture_spec
            .token_budget
            .checked_sub(maximum_response_tokens)
            .ok_or_else(|| format!("case {} exceeds its token budget", case.request.case_id))?;
        if certificate.budget.slack < 256 {
            return Err(format!(
                "case {} has only {} finalized tokens of budget slack",
                case.request.case_id, certificate.budget.slack
            ));
        }
        write_json(&certificate_path, &certificate)?;
        common_hashes.insert(case.request.case_id.clone(), common_hash);
    }

    let observations = extract_all_observations(assets, platforms, &capture_log)?;
    for case in &cases {
        let arms = observations.get(&case.request.case_id).ok_or_else(|| {
            format!(
                "no representative raw observation for {}",
                case.request.case_id
            )
        })?;
        if arms.len() != platforms.len() * 2 {
            return Err(format!(
                "case {} has {} raw arms, expected {}",
                case.request.case_id,
                arms.len(),
                platforms.len() * 2
            ));
        }
        let first = &arms[0];
        if arms
            .iter()
            .any(|arm| arm.text != first.text || arm.meta != first.meta)
        {
            return Err(format!(
                "case {} representative raw transcripts do not agree",
                case.request.case_id
            ));
        }
        if sha256(first.text.as_bytes()) != common_hashes[&case.request.case_id] {
            return Err(format!(
                "case {} raw transcript hash does not match stability ledger",
                case.request.case_id
            ));
        }
        write_bytes(&case.directory.join("expected.text"), first.text.as_bytes())?;
        write_json(&case.directory.join("expected.meta.json"), &first.meta)?;
    }
    write_generator_source_hash(assets)?;
    write_manifest(assets, platforms, &cases, &capture_log)?;
    Ok(())
}

fn validate_capture_log(
    capture_log: &CaptureLog,
    platforms: &[String],
    cases: &[CaseFiles],
    fixture_tree_sha256: &str,
) -> Result<(), String> {
    if capture_log.schema != 1 {
        return Err(format!(
            "unsupported capture log schema {}",
            capture_log.schema
        ));
    }
    let expected_keys = platforms
        .iter()
        .flat_map(|platform| {
            ["source-built", "release"]
                .into_iter()
                .map(move |oracle| format!("{platform}/{oracle}"))
        })
        .collect::<BTreeSet<_>>();
    let actual_keys = capture_log
        .captures
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    if actual_keys != expected_keys {
        return Err(format!(
            "capture log arms differ from the audited platform/oracle matrix: {actual_keys:?}"
        ));
    }
    let mut fixture_roots = BTreeSet::new();
    for (key, entry) in &capture_log.captures {
        if key != &format!("{}/{}", entry.platform, entry.oracle)
            || !platforms.contains(&entry.platform)
            || !matches!(entry.oracle.as_str(), "source-built" | "release")
        {
            return Err(format!("capture log identity mismatch for {key}"));
        }
        let order = shuffled_case_indices(cases.len(), entry.seed)
            .into_iter()
            .map(|index| cases[index].request.case_id.clone())
            .collect::<Vec<_>>();
        if entry.seed != 0x0FAC_C011
            || entry.case_order != order
            || entry.case_order.iter().collect::<BTreeSet<_>>().len() != cases.len()
            || entry.binary_version != "fastctx 0.1.1"
            || entry.runs_per_case != 32
            || entry.immutable_readback_checks != 2 + 32 * cases.len()
            || !entry.fresh_home_per_invocation
            || entry.environment_profile != CAPTURE_ENVIRONMENT_PROFILE
            || !entry.isolated_git_parent
            || entry.binary_size == 0
            || entry.capture_harness_size == 0
            || !is_sha256(&entry.binary_sha256)
            || !is_sha256(&entry.capture_harness_sha256)
            || !is_sha256(&entry.fixture_tree_sha256)
            || !is_sha256(&entry.stdin_sha256)
            || !is_sha256(&entry.stdout_sha256)
            || !is_sha256(&entry.stderr_sha256)
            || entry.fixture_tree_sha256 != fixture_tree_sha256
            || !valid_recorded_fixture_root(&entry.platform, &entry.fixture_root)
            || !fixture_roots.insert(entry.fixture_root.clone())
        {
            return Err(format!(
                "capture log evidence is incomplete or invalid for {key}"
            ));
        }
    }
    for platform in platforms {
        let source = &capture_log.captures[&format!("{platform}/source-built")];
        let release = &capture_log.captures[&format!("{platform}/release")];
        if source.capture_harness_sha256 != release.capture_harness_sha256
            || source.capture_harness_size != release.capture_harness_size
        {
            return Err(format!(
                "source/release arms did not use the same capture executable on {platform}"
            ));
        }
    }
    Ok(())
}

fn derive_fixture_readback(spec: &FixtureSpec) -> Result<FixtureReadback, String> {
    let parent = tempfile::Builder::new()
        .prefix("fastctx-v011-finalize-")
        .tempdir()
        .map_err(|error| format!("cannot create fixture qualification directory: {error}"))?;
    fs::create_dir(parent.path().join(".git"))
        .map_err(|error| format!("cannot create fixture qualification .git marker: {error}"))?;
    let root = parent.path().join("fastctx-v011-finalize");
    let fixture = FixtureGuard::materialize(&root, spec)?;
    let readback = fixture.verify_immutable()?;
    fixture.finish()?;
    Ok(readback)
}

fn valid_recorded_fixture_root(platform: &str, root: &str) -> bool {
    if root.is_empty()
        || root == crate::model::ROOT_TOKEN
        || !root.is_ascii()
        || root.contains('\\')
    {
        return false;
    }
    let components = if platform == "windows-x64" {
        let bytes = root.as_bytes();
        if bytes.len() < 4
            || !bytes[0].is_ascii_alphabetic()
            || bytes[1] != b':'
            || bytes[2] != b'/'
        {
            return false;
        }
        root[3..].split('/').collect::<Vec<_>>()
    } else {
        if !root.starts_with('/') {
            return false;
        }
        root[1..].split('/').collect::<Vec<_>>()
    };
    components.len() >= 2
        && components
            .iter()
            .all(|component| !component.is_empty() && !matches!(*component, "." | ".."))
        && components
            .last()
            .is_some_and(|component| component.starts_with("fastctx-v011"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_ledger(
    case_id: &str,
    platform: &str,
    oracle: &str,
    ledger: &OracleStability,
    token_budget: usize,
) -> Result<(), String> {
    if ledger.runs != 32
        || ledger.raw_stdout_sha256.len() != 32
        || ledger.normalized_sha256.len() != 32
        || ledger.statuses.len() != 32
        || ledger.replacement_counts.len() != 32
        || ledger.unique_normalized_hashes != 1
        || ledger.minimum_budget_slack < 256
        || ledger
            .maximum_response_tokens
            .checked_add(ledger.minimum_budget_slack)
            != Some(token_budget)
        || ledger
            .normalized_sha256
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != 1
    {
        return Err(format!(
            "invalid 32-run ledger for {case_id} {platform}/{oracle}"
        ));
    }
    if ledger
        .statuses
        .iter()
        .any(|status| status.exit_code != 0 || status.is_error || status.content_kind != "text")
    {
        return Err(format!(
            "non-success status in {case_id} {platform}/{oracle}"
        ));
    }
    if ledger
        .replacement_counts
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        != 1
    {
        return Err(format!(
            "unstable root replacement count in {case_id} {platform}/{oracle}"
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Observation {
    text: String,
    meta: ExpectedMeta,
}

fn extract_all_observations(
    assets: &Path,
    platforms: &[String],
    capture_log: &CaptureLog,
) -> Result<BTreeMap<String, Vec<Observation>>, String> {
    let mut observations = BTreeMap::<String, Vec<Observation>>::new();
    for platform in platforms {
        for oracle in ["source-built", "release"] {
            let key = format!("{platform}/{oracle}");
            let entry = capture_log
                .captures
                .get(&key)
                .ok_or_else(|| format!("capture log has no {key} entry"))?;
            let raw_directory = assets.join("raw").join(platform);
            let stdin_path = raw_directory.join(format!("{oracle}.stdin.jsonl"));
            let stdout_path = raw_directory.join(format!("{oracle}.stdout.jsonl"));
            let stderr_path = raw_directory.join(format!("{oracle}.stderr.bin"));
            let raw_stdin = fs::read(&stdin_path)
                .map_err(|error| format!("cannot read {}: {error}", stdin_path.display()))?;
            let raw_stdout = fs::read(&stdout_path)
                .map_err(|error| format!("cannot read {}: {error}", stdout_path.display()))?;
            let raw_stderr = fs::read(&stderr_path)
                .map_err(|error| format!("cannot read {}: {error}", stderr_path.display()))?;
            if sha256(&raw_stdin) != entry.stdin_sha256 {
                return Err(format!("raw stdin hash mismatch for {key}"));
            }
            if sha256(&raw_stdout) != entry.stdout_sha256 {
                return Err(format!("raw stdout hash mismatch for {key}"));
            }
            if sha256(&raw_stderr) != entry.stderr_sha256 || !raw_stderr.is_empty() {
                return Err(format!(
                    "raw stderr is nonempty or has the wrong hash for {key}"
                ));
            }
            let stdin_frames = parse_jsonl(&raw_stdin, &format!("{key} stdin"))?;
            let stdout_frames = parse_jsonl(&raw_stdout, &format!("{key} stdout"))?;
            if stdin_frames.len() != entry.case_order.len() * 3
                || stdout_frames.len() != entry.case_order.len() * 2
            {
                return Err(format!("raw protocol frame count mismatch for {key}"));
            }
            for (position, case_id) in entry.case_order.iter().enumerate() {
                let input = &stdin_frames[position * 3..position * 3 + 3];
                let output = &stdout_frames[position * 2..position * 2 + 2];
                validate_protocol_frames(assets, case_id, &entry.fixture_root, input, output)?;
                let response = &output[1];
                let result = response
                    .get("result")
                    .ok_or_else(|| format!("raw response for {case_id} has no result"))?;
                let is_error = result
                    .get("isError")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let content = result
                    .get("content")
                    .and_then(|value| value.as_array())
                    .ok_or_else(|| format!("raw response for {case_id} has no content"))?;
                if content.len() != 1 {
                    return Err(format!("raw response for {case_id} is not single-content"));
                }
                let content_kind = content[0]
                    .get("type")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| format!("raw response for {case_id} has no content type"))?;
                let text = content[0]
                    .get("text")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| format!("raw response for {case_id} has no text"))?;
                let (text, replacement_count) = replace_root(text, &entry.fixture_root)?;
                let normalized_text_sha256 = sha256(text.as_bytes());
                observations
                    .entry(case_id.clone())
                    .or_default()
                    .push(Observation {
                        text,
                        meta: ExpectedMeta {
                            schema: 1,
                            case_id: case_id.clone(),
                            is_error,
                            content_kind: content_kind.to_string(),
                            replacement_count,
                            normalized_text_sha256,
                        },
                    });
            }
        }
    }
    Ok(observations)
}

fn parse_jsonl(bytes: &[u8], label: &str) -> Result<Vec<serde_json::Value>, String> {
    if bytes.is_empty() || !bytes.ends_with(b"\n") {
        return Err(format!("{label} is empty or lacks a final newline"));
    }
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_slice(line)
                .map_err(|error| format!("{label} frame {} is not JSON: {error}", index + 1))
        })
        .collect()
}

fn validate_protocol_frames(
    assets: &Path,
    case_id: &str,
    fixture_root: &str,
    input: &[serde_json::Value],
    output: &[serde_json::Value],
) -> Result<(), String> {
    let valid_initialize = input[0].get("jsonrpc").and_then(|value| value.as_str()) == Some("2.0")
        && input[0].get("id").and_then(|value| value.as_i64()) == Some(1)
        && input[0].get("method").and_then(|value| value.as_str()) == Some("initialize");
    let valid_initialized = input[1].get("jsonrpc").and_then(|value| value.as_str()) == Some("2.0")
        && input[1].get("id").is_none()
        && input[1].get("method").and_then(|value| value.as_str())
            == Some("notifications/initialized");
    let request: crate::model::CaseRequest =
        read_json(&assets.join("cases").join(case_id).join("request.json"))?;
    let mut expected_arguments = request.arguments;
    substitute_root(&mut expected_arguments, fixture_root);
    let valid_call = input[2].get("jsonrpc").and_then(|value| value.as_str()) == Some("2.0")
        && input[2].get("id").and_then(|value| value.as_i64()) == Some(2)
        && input[2].get("method").and_then(|value| value.as_str()) == Some("tools/call")
        && input[2]
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(|value| value.as_str())
            == Some(request.tool.as_str())
        && input[2]
            .get("params")
            .and_then(|params| params.get("arguments"))
            == Some(&expected_arguments);
    let valid_initialize_response = output[0].get("jsonrpc").and_then(|value| value.as_str())
        == Some("2.0")
        && output[0].get("id").and_then(|value| value.as_i64()) == Some(1)
        && output[0].get("result").is_some()
        && output[0].get("error").is_none();
    let valid_call_response = output[1].get("jsonrpc").and_then(|value| value.as_str())
        == Some("2.0")
        && output[1].get("id").and_then(|value| value.as_i64()) == Some(2)
        && output[1].get("result").is_some()
        && output[1].get("error").is_none();
    if !(valid_initialize
        && valid_initialized
        && valid_call
        && valid_initialize_response
        && valid_call_response)
    {
        return Err(format!("raw JSON-RPC protocol mismatch for case {case_id}"));
    }
    Ok(())
}

fn write_generator_source_hash(assets: &Path) -> Result<(), String> {
    let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![
        manifest_directory.join("Cargo.toml"),
        manifest_directory.join("Cargo.lock"),
        manifest_directory.join("README.md"),
    ];
    let mut sources = fs::read_dir(manifest_directory.join("src"))
        .map_err(|error| format!("cannot list capture source: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("cannot inspect capture source: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    sources.sort();
    files.extend(sources);
    let mut lines = String::new();
    for file in files {
        lines.push_str(&format!(
            "{}  {}\n",
            sha256_file(&file)?,
            file.strip_prefix(manifest_directory)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/")
        ));
    }
    let verifier = manifest_directory
        .parent()
        .expect("capture package is below tools")
        .join("verify-v011-assets.py");
    lines.push_str(&format!(
        "{}  ../verify-v011-assets.py\n",
        sha256_file(&verifier)?
    ));
    write_bytes(
        &assets.join("provenance/generator-source.sha256"),
        lines.as_bytes(),
    )
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceProvenance {
    schema: u32,
    tag: String,
    commit: String,
    tree: String,
    version: String,
    source_archive_sha256: String,
    cargo_lock_sha256: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseAssetMetadata {
    schema: u32,
    release_id: u64,
    tag: String,
    published_at: String,
    assets: BTreeMap<String, ReleaseAssetEvidence>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReleaseAssetEvidence {
    asset_id: u64,
    bytes: u64,
    sha256: String,
    url: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ToolchainsProvenance {
    schema: u32,
    platforms: BTreeMap<String, ToolchainEvidence>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ToolchainEvidence {
    target: String,
    runner: String,
    rustc: String,
    cargo: String,
    source_build: String,
    release_asset: String,
}

#[derive(Serialize)]
struct Manifest {
    schema: u32,
    source: SourceProvenance,
    runs_per_platform_oracle: usize,
    required_platforms: Vec<String>,
    fixture_spec: ManifestFile,
    release_assets: BTreeMap<String, String>,
    release_asset_metadata: ReleaseAssetMetadata,
    toolchains: ToolchainsProvenance,
    generator: ManifestGenerator,
    captures: BTreeMap<String, CaptureLogEntry>,
    cases: BTreeMap<String, ManifestCase>,
    files: BTreeMap<String, ManifestFile>,
}

#[derive(Serialize)]
struct ManifestCase {
    fixture_tree_sha256: String,
    common_normalized_sha256: String,
    minimum_budget_slack: usize,
    replacement_counts: BTreeMap<String, usize>,
    readback_sort_keys: Vec<String>,
    stability: CaseStability,
    assets: BTreeMap<String, ManifestFile>,
}

#[derive(Serialize)]
struct ManifestGenerator {
    has_fastctx_dependency: bool,
    source_ledger_sha256: String,
    sources: BTreeMap<String, String>,
    verifier_sha256: String,
}

#[derive(Clone, Serialize)]
struct ManifestFile {
    sha256: String,
    bytes: u64,
}

fn write_manifest(
    assets: &Path,
    platforms: &[String],
    cases: &[CaseFiles],
    capture_log: &CaptureLog,
) -> Result<(), String> {
    let source: SourceProvenance = read_json(&assets.join("provenance/source.json"))?;
    if source.schema != 1
        || source.tag != "v0.1.1"
        || source.commit != EXPECTED_COMMIT
        || source.tree != EXPECTED_TREE
        || source.version != "0.1.1"
        || source.source_archive_sha256 != EXPECTED_SOURCE_ARCHIVE
        || source.cargo_lock_sha256 != EXPECTED_CARGO_LOCK
    {
        return Err("source provenance does not identify the exact v0.1.1 source".to_string());
    }
    let release_assets = parse_sha256_ledger(&assets.join("provenance/release-assets.sha256"))?;
    if release_assets != expected_release_assets() {
        return Err("release asset ledger differs from the audited v0.1.1 release".to_string());
    }
    let release_asset_metadata: ReleaseAssetMetadata =
        read_json(&assets.join("provenance/release-assets.json"))?;
    validate_release_metadata(&release_asset_metadata, &release_assets)?;
    let cargo_lock_ledger = parse_sha256_ledger(&assets.join("provenance/cargo-lock.sha256"))?;
    if cargo_lock_ledger
        != [(
            "v0.1.1-Cargo.lock".to_string(),
            EXPECTED_CARGO_LOCK.to_string(),
        )]
        .into_iter()
        .collect()
    {
        return Err("source Cargo.lock ledger differs from the audited source".to_string());
    }
    let source_tree_ledger =
        parse_sha256_ledger(&assets.join("provenance/oracle-source-tree.sha256"))?;
    if source_tree_ledger
        != [(
            "git-archive-v0.1.1.tar".to_string(),
            EXPECTED_SOURCE_ARCHIVE.to_string(),
        )]
        .into_iter()
        .collect()
    {
        return Err("source archive ledger differs from the audited source".to_string());
    }
    let oracle_binary_ledger =
        parse_sha256_ledger(&assets.join("provenance/oracle-binaries.sha256"))?;
    let expected_oracle_binary_ledger = capture_log
        .captures
        .iter()
        .map(|(key, entry)| (key.clone(), entry.binary_sha256.clone()))
        .collect::<BTreeMap<_, _>>();
    if oracle_binary_ledger != expected_oracle_binary_ledger {
        return Err("oracle binary ledger differs from the capture log".to_string());
    }
    let generator_sources =
        parse_sha256_ledger(&assets.join("provenance/generator-source.sha256"))?;
    validate_generator_sources(&generator_sources)?;
    let verifier_sha256 = generator_sources
        .get("../verify-v011-assets.py")
        .cloned()
        .ok_or_else(|| "generator source ledger omits the verifier".to_string())?;
    let toolchains: ToolchainsProvenance = read_json(&assets.join("provenance/toolchains.json"))?;
    validate_toolchains(&toolchains, platforms)?;
    let fixture_spec = file_evidence(&assets.join("fixture-spec.json"))?;
    let mut manifest_cases = BTreeMap::new();
    for case in cases {
        let stability: CaseStability = read_json(&case.directory.join("stability.json"))?;
        let certificate: DeterminismCertificate =
            read_json(&case.directory.join("determinism-certificate.json"))?;
        let mut replacement_counts = BTreeMap::new();
        for platform in platforms {
            for oracle in ["source-built", "release"] {
                let ledger = &stability.platforms[platform].oracles[oracle];
                replacement_counts
                    .insert(format!("{platform}/{oracle}"), ledger.replacement_counts[0]);
            }
        }
        let mut case_assets = BTreeMap::new();
        for name in [
            "request.json",
            "env.json",
            "determinism-certificate.json",
            "stability.json",
            "expected.text",
            "expected.meta.json",
        ] {
            case_assets.insert(name.to_string(), file_evidence(&case.directory.join(name))?);
        }
        manifest_cases.insert(
            case.request.case_id.clone(),
            ManifestCase {
                fixture_tree_sha256: certificate.fixture_tree_sha256,
                common_normalized_sha256: stability
                    .common_normalized_sha256
                    .clone()
                    .ok_or_else(|| format!("case {} is not finalized", case.request.case_id))?,
                minimum_budget_slack: certificate.budget.slack,
                replacement_counts,
                readback_sort_keys: certificate.sort.readback_keys,
                stability,
                assets: case_assets,
            },
        );
    }
    let mut paths = Vec::new();
    collect_files(assets, assets, &mut paths)?;
    paths.retain(|path| path != "manifest.json");
    paths.sort();
    let mut files = BTreeMap::new();
    for relative in paths {
        let full = assets.join(&relative);
        let metadata = fs::metadata(&full)
            .map_err(|error| format!("cannot inspect {}: {error}", full.display()))?;
        files.insert(
            relative,
            ManifestFile {
                sha256: sha256_file(&full)?,
                bytes: metadata.len(),
            },
        );
    }
    let manifest = Manifest {
        schema: 1,
        source,
        runs_per_platform_oracle: 32,
        required_platforms: platforms.to_vec(),
        fixture_spec,
        release_assets,
        release_asset_metadata,
        toolchains,
        generator: ManifestGenerator {
            has_fastctx_dependency: false,
            source_ledger_sha256: sha256_file(&assets.join("provenance/generator-source.sha256"))?,
            sources: generator_sources,
            verifier_sha256,
        },
        captures: capture_log.captures.clone(),
        cases: manifest_cases,
        files,
    };
    write_json(&assets.join("manifest.json"), &manifest)
}

fn expected_release_assets() -> BTreeMap<String, String> {
    [
        (
            "SHA256SUMS",
            "953c55ec9b050bef0c15e4ad5a990c033c2e83e0c3384a418a7adab09b7b3abe",
        ),
        (
            "fastctx-aarch64-apple-darwin.tar.gz",
            "8a801f7da81400f73676737d4273ee6f392f1fcc4d7ceb2c276cfbcb58647229",
        ),
        (
            "fastctx-x86_64-apple-darwin.tar.gz",
            "5170941b234dd1556dd52a38a8d30f8dd922593b39fd463f50df53d497391bb3",
        ),
        (
            "fastctx-x86_64-pc-windows-msvc.zip",
            "fb71e0db34293fbbc34673839fb22befa3bed08954f42743c8d3252a0a6ace21",
        ),
        (
            "fastctx-x86_64-unknown-linux-gnu.tar.gz",
            "583d1b1e0d6768f3213c48d4a14b46bae57606891324b1acbac66b9e38757b1d",
        ),
    ]
    .into_iter()
    .map(|(name, digest)| (name.to_string(), digest.to_string()))
    .collect()
}

fn expected_release_details() -> BTreeMap<String, (u64, u64)> {
    [
        ("SHA256SUMS", (482_598_815, 410)),
        (
            "fastctx-aarch64-apple-darwin.tar.gz",
            (482_598_820, 24_845_992),
        ),
        (
            "fastctx-x86_64-apple-darwin.tar.gz",
            (482_598_817, 25_008_433),
        ),
        (
            "fastctx-x86_64-pc-windows-msvc.zip",
            (482_598_816, 25_807_596),
        ),
        (
            "fastctx-x86_64-unknown-linux-gnu.tar.gz",
            (482_598_819, 25_190_282),
        ),
    ]
    .into_iter()
    .map(|(name, detail)| (name.to_string(), detail))
    .collect()
}

fn validate_release_metadata(
    metadata: &ReleaseAssetMetadata,
    release_assets: &BTreeMap<String, String>,
) -> Result<(), String> {
    if metadata.schema != 1
        || metadata.release_id != 356_398_977
        || metadata.tag != "v0.1.1"
        || metadata.published_at != "2026-07-19T17:46:17Z"
        || metadata.assets.keys().collect::<BTreeSet<_>>()
            != release_assets.keys().collect::<BTreeSet<_>>()
    {
        return Err("release metadata identity or asset set differs".to_string());
    }
    let details = expected_release_details();
    for (name, digest) in release_assets {
        let evidence = &metadata.assets[name];
        let (asset_id, bytes) = details[name];
        let expected_url =
            format!("https://github.com/yc-duan/fastctx/releases/download/v0.1.1/{name}");
        if evidence.asset_id != asset_id
            || evidence.bytes != bytes
            || evidence.sha256 != *digest
            || evidence.url != expected_url
        {
            return Err(format!("release metadata differs for {name}"));
        }
    }
    Ok(())
}

fn validate_generator_sources(sources: &BTreeMap<String, String>) -> Result<(), String> {
    let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut expected_labels = [
        "Cargo.toml".to_string(),
        "Cargo.lock".to_string(),
        "README.md".to_string(),
        "../verify-v011-assets.py".to_string(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let source_directory = manifest_directory.join("src");
    for entry in fs::read_dir(&source_directory)
        .map_err(|error| format!("cannot list {}: {error}", source_directory.display()))?
    {
        let path = entry
            .map_err(|error| format!("cannot inspect capture source: {error}"))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("capture source name is not UTF-8: {}", path.display()))?;
            expected_labels.insert(format!("src/{name}"));
        }
    }
    if sources.keys().cloned().collect::<BTreeSet<_>>() != expected_labels {
        return Err("generator source ledger inventory differs from capture source".to_string());
    }
    for (label, digest) in sources {
        let path = manifest_directory.join(label);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "cannot inspect generator source {}: {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || sha256_file(&path)? != *digest
        {
            return Err(format!("generator source evidence differs for {label}"));
        }
    }
    Ok(())
}

fn validate_toolchains(
    toolchains: &ToolchainsProvenance,
    platforms: &[String],
) -> Result<(), String> {
    let expected = [
        (
            "windows-x64",
            "x86_64-pc-windows-msvc",
            "fastctx-x86_64-pc-windows-msvc.zip",
        ),
        (
            "linux-x64",
            "x86_64-unknown-linux-gnu",
            "fastctx-x86_64-unknown-linux-gnu.tar.gz",
        ),
        (
            "macos-x64",
            "x86_64-apple-darwin",
            "fastctx-x86_64-apple-darwin.tar.gz",
        ),
        (
            "macos-arm64",
            "aarch64-apple-darwin",
            "fastctx-aarch64-apple-darwin.tar.gz",
        ),
    ]
    .into_iter()
    .map(|(platform, target, asset)| (platform, (target, asset)))
    .collect::<BTreeMap<_, _>>();
    if toolchains.schema != 1
        || toolchains
            .platforms
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != platforms
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
    {
        return Err("toolchain provenance platform matrix differs".to_string());
    }
    for platform in platforms {
        let evidence = &toolchains.platforms[platform];
        let (target, release_asset) = expected[platform.as_str()];
        if evidence.target != target
            || evidence.runner.is_empty()
            || evidence.rustc.is_empty()
            || evidence.cargo.is_empty()
            || evidence.source_build != SOURCE_BUILD_COMMAND
            || evidence.release_asset != release_asset
        {
            return Err(format!("toolchain provenance differs for {platform}"));
        }
    }
    Ok(())
}

fn file_evidence(path: &Path) -> Result<ManifestFile, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "manifest asset is not a regular file: {}",
            path.display()
        ));
    }
    Ok(ManifestFile {
        sha256: sha256_file(path)?,
        bytes: metadata.len(),
    })
}

fn parse_sha256_ledger(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut output = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let Some((digest, name)) = line.split_once("  ") else {
            return Err(format!(
                "malformed SHA-256 ledger line {} in {}",
                index + 1,
                path.display()
            ));
        };
        if !is_sha256(digest)
            || name.is_empty()
            || output
                .insert(name.to_string(), digest.to_string())
                .is_some()
        {
            return Err(format!(
                "invalid SHA-256 ledger line {} in {}",
                index + 1,
                path.display()
            ));
        }
    }
    if output.is_empty() {
        return Err(format!("SHA-256 ledger is empty: {}", path.display()));
    }
    Ok(output)
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<String>) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot list {}: {error}", directory.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| format!("cannot inspect asset entry: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    for path in entries {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "compatibility asset tree contains a symlink: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_files(root, &path, output)?;
        } else if metadata.is_file() {
            output.push(
                path.strip_prefix(root)
                    .map_err(|error| format!("asset escaped root: {error}"))?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        } else {
            return Err(format!(
                "compatibility asset tree contains a special file: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

struct CaptureHome {
    path: PathBuf,
    cleaned: bool,
}

impl CaptureHome {
    fn create(fixture_root: &Path, platform: &str, oracle: &str) -> Result<Self, String> {
        let name = fixture_root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "fixture root has no UTF-8 name".to_string())?;
        let path = fixture_root.with_file_name(format!("{name}-home-{platform}-{oracle}"));
        if path.exists() {
            return Err(format!(
                "capture home already exists and will not be overwritten: {}",
                path.display()
            ));
        }
        fs::create_dir(&path)
            .map_err(|error| format!("cannot create capture home {}: {error}", path.display()))?;
        Ok(Self {
            path,
            cleaned: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn finish(mut self) -> Result<(), String> {
        fs::remove_dir_all(&self.path).map_err(|error| {
            format!(
                "cannot remove isolated capture home {}: {error}",
                self.path.display()
            )
        })?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for CaptureHome {
    fn drop(&mut self) {
        if !self.cleaned && self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{shuffled_case_indices, validate_request_schema};
    use crate::model::{CaseEnvironment, CaseFiles, CaseRequest};
    use std::collections::BTreeMap;

    #[test]
    fn case_shuffle_is_seeded_complete_and_stable() {
        let first = shuffled_case_indices(23, 0x0FAC_C011);
        let second = shuffled_case_indices(23, 0x0FAC_C011);
        assert_eq!(first, second);
        let mut sorted = first.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..23).collect::<Vec<_>>());
        assert_ne!(first, (0..23).collect::<Vec<_>>());
    }

    #[test]
    fn frozen_request_schema_rejects_unknown_fields() {
        let directory = tempfile::tempdir().unwrap();
        let case = |arguments| CaseFiles {
            directory: directory.path().to_path_buf(),
            request: CaseRequest {
                schema: 1,
                case_id: "grep-type-rust".to_string(),
                tool: "grep".to_string(),
                arguments,
            },
            environment: CaseEnvironment {
                schema: 1,
                variables: BTreeMap::new(),
            },
        };
        assert!(
            validate_request_schema(&case(serde_json::json!({
                "pattern": "needle",
                "path": "{{ROOT}}",
                "file_type": "rust"
            })))
            .unwrap_err()
            .contains("unknown v0.1.1 argument")
        );
        validate_request_schema(&case(serde_json::json!({
            "pattern": "needle",
            "path": "{{ROOT}}",
            "type": "rust"
        })))
        .unwrap();
    }
}
