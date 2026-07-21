#!/usr/bin/env python3
"""Read-only verifier for the frozen FastCtx v0.1.1 compatibility evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import stat
import subprocess
import sys
import tempfile
import tomllib
from typing import Any

ROOT_TOKEN = "{{ROOT}}"
EXPECTED_COMMIT = "64a6a45f88e65a2c0305e36673fa5e3f99d95384"
EXPECTED_TREE = "21efd928f328a5adb063f182fb8655626889fb3a"
EXPECTED_SOURCE_ARCHIVE = "dc314bfb011c9bfb12f8c55bb639e47e1fd1053e040c980eed9c0c49b43f7dd3"
EXPECTED_CARGO_LOCK = "ac793ebb95f5f62f62f44db067d0c1ef0779a618aef25cbc490ca35f1ec0e33f"
ORACLES = ("source-built", "release")
PLATFORMS = ("windows-x64", "linux-x64", "macos-x64", "macos-arm64")
PLATFORM_TARGETS = {
    "windows-x64": "x86_64-pc-windows-msvc",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "macos-x64": "x86_64-apple-darwin",
    "macos-arm64": "aarch64-apple-darwin",
}
PLATFORM_RELEASE_ASSETS = {
    "windows-x64": "fastctx-x86_64-pc-windows-msvc.zip",
    "linux-x64": "fastctx-x86_64-unknown-linux-gnu.tar.gz",
    "macos-x64": "fastctx-x86_64-apple-darwin.tar.gz",
    "macos-arm64": "fastctx-aarch64-apple-darwin.tar.gz",
}
REQUIRED_CASES = (
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
)
EXPECTED_RELEASE_ASSETS = {
    "fastctx-x86_64-pc-windows-msvc.zip": "fb71e0db34293fbbc34673839fb22befa3bed08954f42743c8d3252a0a6ace21",
    "fastctx-x86_64-unknown-linux-gnu.tar.gz": "583d1b1e0d6768f3213c48d4a14b46bae57606891324b1acbac66b9e38757b1d",
    "fastctx-x86_64-apple-darwin.tar.gz": "5170941b234dd1556dd52a38a8d30f8dd922593b39fd463f50df53d497391bb3",
    "fastctx-aarch64-apple-darwin.tar.gz": "8a801f7da81400f73676737d4273ee6f392f1fcc4d7ceb2c276cfbcb58647229",
    "SHA256SUMS": "953c55ec9b050bef0c15e4ad5a990c033c2e83e0c3384a418a7adab09b7b3abe",
}
EXPECTED_RELEASE_DETAILS = {
    "SHA256SUMS": (482598815, 410),
    "fastctx-aarch64-apple-darwin.tar.gz": (482598820, 24845992),
    "fastctx-x86_64-apple-darwin.tar.gz": (482598817, 25008433),
    "fastctx-x86_64-pc-windows-msvc.zip": (482598816, 25807596),
    "fastctx-x86_64-unknown-linux-gnu.tar.gz": (482598819, 25190282),
}
HEX_64 = re.compile(r"^[0-9a-f]{64}$")


class VerificationError(Exception):
    pass


def fail(message: str) -> None:
    raise VerificationError(message)


def read_bytes(path: pathlib.Path) -> bytes:
    try:
        return path.read_bytes()
    except OSError as error:
        fail(f"cannot read {path}: {error}")


def read_json(path: pathlib.Path) -> Any:
    raw = read_bytes(path)
    try:
        return json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"cannot parse {path}: {error}")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def is_sha256(value: Any) -> bool:
    return isinstance(value, str) and HEX_64.fullmatch(value) is not None


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def relative_files(root: pathlib.Path) -> list[str]:
    files: list[str] = []
    for path in sorted(root.rglob("*")):
        try:
            info = path.lstat()
        except OSError as error:
            fail(f"cannot inspect {path}: {error}")
        relative = path.relative_to(root).as_posix()
        require(not stat.S_ISLNK(info.st_mode), f"asset tree contains a symlink: {relative}")
        if stat.S_ISDIR(info.st_mode):
            continue
        require(stat.S_ISREG(info.st_mode), f"asset tree contains a special file: {relative}")
        if relative != "manifest.json":
            files.append(relative)
    return files


def verify_manifest(assets: pathlib.Path) -> tuple[dict[str, Any], list[str]]:
    manifest = read_json(assets / "manifest.json")
    require(isinstance(manifest, dict), "manifest root is not an object")
    require(manifest.get("schema") == 1, "manifest schema must be 1")
    source = manifest.get("source")
    require(isinstance(source, dict), "manifest source provenance is missing")
    require(source.get("schema") == 1, "manifest source schema must be 1")
    require(source.get("tag") == "v0.1.1", "manifest tag is not v0.1.1")
    require(source.get("commit") == EXPECTED_COMMIT, "manifest commit is not the frozen commit")
    require(source.get("tree") == EXPECTED_TREE, "manifest tree is not the frozen tree")
    require(source.get("version") == "0.1.1", "manifest version is not 0.1.1")
    require(source.get("source_archive_sha256") == EXPECTED_SOURCE_ARCHIVE, "manifest source archive hash differs")
    require(source.get("cargo_lock_sha256") == EXPECTED_CARGO_LOCK, "manifest Cargo.lock hash differs")
    require(manifest.get("runs_per_platform_oracle") == 32, "manifest must require 32 runs")
    platforms = manifest.get("required_platforms")
    require(platforms == list(PLATFORMS), "manifest platform matrix/order is not the audited four-target set")
    generator = manifest.get("generator")
    require(isinstance(generator, dict), "manifest generator evidence is missing")
    require(generator.get("has_fastctx_dependency") is False, "capture harness dependency flag is not false")
    require(is_sha256(generator.get("source_ledger_sha256")), "generator source ledger hash is invalid")
    require(is_sha256(generator.get("verifier_sha256")), "verifier source hash is invalid")
    require(manifest.get("release_assets") == EXPECTED_RELEASE_ASSETS, "manifest release asset hashes differ from the official v0.1.1 set")
    declared = manifest.get("files")
    require(isinstance(declared, dict), "manifest files map is missing")
    actual = relative_files(assets)
    require(sorted(declared) == actual, "manifest file inventory differs from the asset tree")
    for relative in actual:
        metadata = declared[relative]
        require(isinstance(metadata, dict), f"manifest file metadata is invalid: {relative}")
        data = read_bytes(assets / relative)
        require(metadata.get("sha256") == sha256(data), f"manifest SHA-256 mismatch: {relative}")
        require(metadata.get("bytes") == len(data), f"manifest byte length mismatch: {relative}")
    fixture_spec = manifest.get("fixture_spec")
    require(isinstance(fixture_spec, dict), "manifest fixture-spec evidence is missing")
    fixture_bytes = read_bytes(assets / "fixture-spec.json")
    require(fixture_spec == {"sha256": sha256(fixture_bytes), "bytes": len(fixture_bytes)}, "manifest fixture-spec evidence differs")
    return manifest, platforms


def safe_component(component: str) -> bool:
    return (
        bool(component)
        and component not in (".", "..")
        and not component.startswith(("~fastctx~b", "~fastctx~w"))
        and "\\" not in component
        and all(ord(character) >= 32 and ord(character) != 127 and character not in "\u2028\u2029" for character in component)
    )


def verify_fixture(assets: pathlib.Path) -> tuple[dict[str, Any], str, list[dict[str, Any]]]:
    spec = read_json(assets / "fixture-spec.json")
    require(isinstance(spec, dict), "fixture spec root is not an object")
    require(spec.get("schema") == 1, "fixture schema must be 1")
    budget = spec.get("token_budget")
    require(isinstance(budget, int) and budget >= 256, "fixture token budget is invalid")
    files = spec.get("files")
    require(isinstance(files, list) and files, "fixture contains no files")
    paths: set[str] = set()
    mtimes: list[int] = []
    for item in files:
        require(isinstance(item, dict), "fixture file entry is not an object")
        relative = item.get("path")
        text = item.get("text")
        mtime = item.get("mtime_unix_seconds")
        require(isinstance(relative, str), "fixture path is not a string")
        pure = pathlib.PurePosixPath(relative)
        require(not pure.is_absolute() and all(safe_component(part) for part in pure.parts), f"unsafe fixture path: {relative!r}")
        require(relative not in paths, f"duplicate fixture path: {relative}")
        require(isinstance(text, str), f"fixture content is not strict UTF-8 text: {relative}")
        text.encode("utf-8", "strict")
        require(ROOT_TOKEN not in text, f"fixture text collides with the root token: {relative}")
        require(isinstance(mtime, int) and mtime >= 0, f"invalid fixture mtime: {relative}")
        paths.add(relative)
        mtimes.append(mtime)
    require(len(set(mtimes)) == len(mtimes), "fixture mtimes are not unique")
    ordered_mtimes = sorted(mtimes)
    require(all(right - left >= 10 for left, right in zip(ordered_mtimes, ordered_mtimes[1:])), "fixture mtimes differ by less than 10 seconds")

    with tempfile.TemporaryDirectory(prefix="verify-v011-") as parent:
        root = pathlib.Path(parent) / "fastctx-v011-fixture"
        root.mkdir()
        for item in files:
            target = root / pathlib.PurePosixPath(item["path"])
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(item["text"].encode("utf-8"))
            if os.name != "nt":
                target.chmod(0o644)
            os.utime(target, (item["mtime_unix_seconds"], item["mtime_unix_seconds"]))
        entries: list[dict[str, Any]] = []
        for path in sorted(root.rglob("*"), key=lambda value: value.relative_to(root).as_posix()):
            relative = path.relative_to(root).as_posix()
            info = path.lstat()
            require(all(safe_component(part) for part in pathlib.PurePosixPath(relative).parts), f"unsafe readback component: {relative}")
            require(not stat.S_ISLNK(info.st_mode), f"fixture readback contains symlink: {relative}")
            if path.is_dir():
                entries.append(
                    {
                        "path": relative,
                        "kind": "directory",
                        "sha256": sha256(b""),
                        "bytes": 0,
                        "mtime_unix_seconds": 0,
                        "readonly": not bool(info.st_mode & stat.S_IWUSR),
                        "hard_link_count": 1,
                    }
                )
                continue
            require(path.is_file(), f"fixture readback contains special file: {relative}")
            if os.name == "nt":
                require(bool(info.st_mode & stat.S_IWUSR), f"fixture file is readonly: {relative}")
            else:
                require(stat.S_IMODE(info.st_mode) == 0o644, f"fixture mode differs from 0644: {relative}")
            data = path.read_bytes()
            data.decode("utf-8", "strict")
            expected = next(item for item in files if item["path"] == relative)
            require(data == expected["text"].encode("utf-8"), f"fixture bytes changed: {relative}")
            require(info.st_mtime_ns // 1_000_000_000 == expected["mtime_unix_seconds"], f"fixture mtime changed: {relative}")
            require(info.st_nlink == 1, f"fixture hardlink detected: {relative}")
            entries.append(
                {
                    "path": relative,
                    "kind": "file",
                    "sha256": sha256(data),
                    "bytes": len(data),
                    "mtime_unix_seconds": expected["mtime_unix_seconds"],
                    "readonly": not bool(info.st_mode & stat.S_IWUSR),
                    "hard_link_count": 1,
                }
            )
        actual_files = {entry["path"] for entry in entries if entry["kind"] == "file"}
        require(actual_files == paths, "fixture readback file inventory differs from fixture spec")
    encoded = json.dumps(entries, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    return spec, sha256(encoded), entries


def expected_sort_certificate(request: dict[str, Any], entries: list[dict[str, Any]]) -> tuple[str, list[str]]:
    arguments = request["arguments"]
    if request["tool"] == "glob" and arguments.get("sort") == "modified":
        kind = "modified_then_native_path"
    elif request["tool"] == "glob":
        kind = "native_path"
    elif request["tool"] == "grep" and arguments.get("path") == ROOT_TOKEN:
        kind = "modified_then_native_path"
    else:
        kind = "single_file"
    files = [entry for entry in entries if entry["kind"] == "file"]
    if kind == "modified_then_native_path":
        files.sort(key=lambda entry: (-entry["mtime_unix_seconds"], entry["path"].encode("utf-8")))
    else:
        files.sort(key=lambda entry: entry["path"].encode("utf-8"))
    keys = [
        f"mtime={entry['mtime_unix_seconds']:020d};path_hex={entry['path'].encode('utf-8').hex()}"
        for entry in files
    ]
    return kind, keys


def file_evidence(path: pathlib.Path) -> dict[str, Any]:
    data = read_bytes(path)
    return {"sha256": sha256(data), "bytes": len(data)}


def verify_request_schema(case_id: str, request: dict[str, Any]) -> None:
    arguments = request.get("arguments")
    require(isinstance(arguments, dict), f"request arguments are not an object: {case_id}")
    if request.get("tool") == "glob":
        allowed = {"pattern", "path", "filter_mode", "sort", "offset", "limit"}
    else:
        allowed = {
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
        }
    require(set(arguments) <= allowed, f"request contains an unknown v0.1.1 argument: {case_id}")
    require(isinstance(arguments.get("pattern"), str), f"request pattern is not a string: {case_id}")
    for name in ("path", "glob", "type"):
        require(name not in arguments or isinstance(arguments[name], str), f"request {name} is not a string: {case_id}")
    for name in ("case_insensitive", "line_numbers", "only_matching", "multiline"):
        require(name not in arguments or isinstance(arguments[name], bool), f"request {name} is not boolean: {case_id}")
    for name in ("before_context", "after_context", "context", "head_limit", "offset", "limit"):
        require(name not in arguments or isinstance(arguments[name], int) and not isinstance(arguments[name], bool) and arguments[name] >= 0, f"request {name} is not a nonnegative integer: {case_id}")
    if "limit" in arguments:
        require(1 <= arguments["limit"] <= 1_000, f"request limit is outside 1..1000: {case_id}")
    enums = {
        "filter_mode": {"project", "all"},
        "sort": {"path", "modified"},
        "output_mode": {"content", "files_with_matches", "count", "summary"},
    }
    for name, choices in enums.items():
        require(name not in arguments or arguments[name] in choices, f"request {name} value is invalid: {case_id}")


def verify_case_coverage(requests: dict[str, dict[str, Any]]) -> None:
    glob_requests = [request["arguments"] for request in requests.values() if request["tool"] == "glob"]
    grep_requests = [request["arguments"] for request in requests.values() if request["tool"] == "grep"]
    require({request.get("filter_mode", "project") for request in glob_requests} == {"project", "all"}, "glob corpus does not cover project and all filtering")
    require({request.get("sort", "path") for request in glob_requests} == {"path", "modified"}, "glob corpus does not cover both sort modes")
    require(any(request.get("offset", 0) > 0 for request in glob_requests), "glob corpus has no offset page")
    require(any(request.get("limit", 100) < 100 for request in glob_requests), "glob corpus has no bounded page")
    single = [request for request in grep_requests if request.get("path") != ROOT_TOKEN]
    multiple = [request for request in grep_requests if request.get("path") == ROOT_TOKEN]
    expected_modes = {"content", "files_with_matches", "count", "summary"}
    require({request.get("output_mode", "files_with_matches") for request in single} >= expected_modes, "single-file grep corpus misses an output mode")
    require({request.get("output_mode", "files_with_matches") for request in multiple} >= expected_modes, "multi-file grep corpus misses an output mode")
    require(any(request.get("context", 0) > 0 or request.get("before_context", 0) > 0 or request.get("after_context", 0) > 0 for request in grep_requests), "grep corpus has no context case")
    require(any(request.get("only_matching") is True for request in grep_requests), "grep corpus has no only-matching case")
    require(any(request.get("multiline") is True for request in grep_requests), "grep corpus has no multiline case")
    require(any("glob" in request for request in grep_requests), "grep corpus has no glob filter")
    require(requests["grep-type-rust"]["arguments"].get("type") == "rust", "grep type-filter case does not use the v0.1.1 `type` field")


def verify_cases(
    assets: pathlib.Path,
    manifest: dict[str, Any],
    platforms: list[str],
    spec: dict[str, Any],
    tree_hash: str,
    entries: list[dict[str, Any]],
) -> dict[str, dict[str, bytes]]:
    case_root = assets / "cases"
    case_entries = sorted(case_root.iterdir())
    require(all(path.is_dir() and not path.is_symlink() for path in case_entries), "cases contains a non-directory or symlink entry")
    directories = case_entries
    declared_cases = manifest.get("cases")
    require(isinstance(declared_cases, dict), "manifest cases map is missing")
    require([path.name for path in directories] == list(REQUIRED_CASES), "case inventory differs from the audited v0.1.1 matrix")
    require(sorted(declared_cases) == list(REQUIRED_CASES), "manifest case inventory differs")
    expected: dict[str, dict[str, bytes]] = {}
    requests: dict[str, dict[str, Any]] = {}
    for directory in directories:
        case_id = directory.name
        request = read_json(directory / "request.json")
        environment = read_json(directory / "env.json")
        certificate = read_json(directory / "determinism-certificate.json")
        stability = read_json(directory / "stability.json")
        meta = read_json(directory / "expected.meta.json")
        text = read_bytes(directory / "expected.text")
        require(isinstance(request, dict), f"request is not an object: {case_id}")
        require(isinstance(environment, dict), f"environment is not an object: {case_id}")
        require(isinstance(certificate, dict), f"certificate is not an object: {case_id}")
        require(isinstance(stability, dict), f"stability ledger is not an object: {case_id}")
        require(isinstance(meta, dict), f"expected metadata is not an object: {case_id}")
        require(request.get("schema") == 1 and request.get("case_id") == case_id, f"request identity mismatch: {case_id}")
        require(request.get("tool") in ("grep", "glob"), f"unsupported tool in {case_id}")
        verify_request_schema(case_id, request)
        requests[case_id] = request
        request_blob = json.dumps(request.get("arguments"), ensure_ascii=False)
        require(ROOT_TOKEN in request_blob, f"request lacks root token: {case_id}")
        require(request_blob.count(ROOT_TOKEN) >= 1, f"request root token count is invalid: {case_id}")
        require(not any(name in request.get("arguments", {}) for name in ("encoding", "fallback_encoding")), f"encoding case entered frozen corpus: {case_id}")
        require(environment.get("schema") == 1, f"environment schema mismatch: {case_id}")
        variables = environment.get("variables")
        require(isinstance(variables, dict), f"environment variables missing: {case_id}")
        expected_environment = {
            "FASTCTX_GLOB_TOKEN_BUDGET": str(spec["token_budget"]),
            "FASTCTX_GREP_TOKEN_BUDGET": str(spec["token_budget"]),
            "FASTCTX_TOKEN_BUDGET": str(spec["token_budget"]),
        }
        require(variables == expected_environment, f"isolated budget environment differs: {case_id}")
        require(not any(ROOT_TOKEN in value for value in variables.values()), f"environment collides with root token: {case_id}")

        require(certificate.get("schema") == 1 and certificate.get("case_id") == case_id, f"certificate identity mismatch: {case_id}")
        require(certificate.get("fixture_tree_sha256") == tree_hash, f"fixture certificate hash mismatch: {case_id}")
        require(certificate.get("all_components_safe_utf8") == all(all(safe_component(part) for part in pathlib.PurePosixPath(item["path"]).parts) for item in spec["files"]), f"unsafe path certificate: {case_id}")
        require(certificate.get("all_contents_strict_utf8") is True, f"content certificate failed: {case_id}")
        require(certificate.get("forbidden_features_found") == [], f"forbidden feature certificate failed: {case_id}")
        require(certificate.get("request_is_success_path") is True, f"non-success request certificate: {case_id}")
        require(certificate.get("immutable_capture_root") is True, f"mutable fixture certificate: {case_id}")
        sort_certificate = certificate.get("sort")
        require(isinstance(sort_certificate, dict), f"sort certificate missing: {case_id}")
        expected_sort_kind, expected_keys = expected_sort_certificate(request, entries)
        require(sort_certificate.get("kind") == expected_sort_kind, f"sort kind differs from request: {case_id}")
        require(sort_certificate.get("all_total_keys_unique") is True, f"sort keys not unique: {case_id}")
        keys = sort_certificate.get("readback_keys")
        require(keys == expected_keys, f"readback sort keys were not rederived from the fixture: {case_id}")
        require(isinstance(keys, list), f"readback sort keys are not a list: {case_id}")
        require(len(set(keys)) == len(keys), f"duplicate readback keys: {case_id}")
        budget = certificate.get("budget")
        require(isinstance(budget, dict), f"budget certificate missing: {case_id}")
        require(budget.get("limit") == spec["token_budget"], f"certificate budget mismatch: {case_id}")
        require(budget.get("slack") == budget.get("limit") - budget.get("oracle_tokens"), f"certificate slack arithmetic mismatch: {case_id}")
        require(budget.get("slack") >= 256, f"certificate budget slack below 256: {case_id}")

        require(stability.get("schema") == 1 and stability.get("case_id") == case_id, f"stability identity mismatch: {case_id}")
        common_hash = stability.get("common_normalized_sha256")
        require(is_sha256(common_hash), f"common stability hash invalid: {case_id}")
        maximum_response_tokens = 0
        minimum_budget_slack = spec["token_budget"]
        arm_replacement_counts: dict[str, int] = {}
        for platform in platforms:
            platform_ledger = stability.get("platforms", {}).get(platform)
            require(isinstance(platform_ledger, dict), f"missing {platform} ledger: {case_id}")
            oracles = platform_ledger.get("oracles")
            require(isinstance(oracles, dict), f"missing oracle map: {case_id} {platform}")
            for oracle in ORACLES:
                ledger = oracles.get(oracle)
                require(isinstance(ledger, dict), f"missing {platform}/{oracle} ledger: {case_id}")
                require(ledger.get("runs") == 32, f"run count mismatch: {case_id} {platform}/{oracle}")
                for field in ("raw_stdout_sha256", "normalized_sha256", "statuses", "replacement_counts"):
                    require(isinstance(ledger.get(field), list) and len(ledger[field]) == 32, f"ledger field length mismatch: {case_id} {platform}/{oracle}/{field}")
                require(all(is_sha256(value) for value in ledger["raw_stdout_sha256"]), f"raw response hash is invalid: {case_id} {platform}/{oracle}")
                normalized = ledger["normalized_sha256"]
                require(all(is_sha256(value) for value in normalized), f"normalized hash is invalid: {case_id} {platform}/{oracle}")
                require(len(set(normalized)) == 1 and normalized[0] == common_hash, f"unstable normalized hash: {case_id} {platform}/{oracle}")
                require(ledger.get("unique_normalized_hashes") == 1, f"unique hash count mismatch: {case_id} {platform}/{oracle}")
                require(len(set(ledger["replacement_counts"])) == 1, f"replacement count instability: {case_id} {platform}/{oracle}")
                require(all(status == {"exit_code": 0, "is_error": False, "content_kind": "text"} for status in ledger["statuses"]), f"non-success stability status: {case_id} {platform}/{oracle}")
                response_tokens = ledger.get("maximum_response_tokens")
                slack = ledger.get("minimum_budget_slack")
                require(isinstance(response_tokens, int) and response_tokens >= 0, f"response token count is invalid: {case_id} {platform}/{oracle}")
                require(isinstance(slack, int) and slack >= 256, f"budget slack is invalid: {case_id} {platform}/{oracle}")
                require(response_tokens + slack == spec["token_budget"], f"budget ledger arithmetic differs: {case_id} {platform}/{oracle}")
                maximum_response_tokens = max(maximum_response_tokens, response_tokens)
                minimum_budget_slack = min(minimum_budget_slack, slack)
                arm_replacement_counts[f"{platform}/{oracle}"] = ledger["replacement_counts"][0]
        require(budget.get("oracle_tokens") == maximum_response_tokens, f"certificate does not record the maximum cross-arm response tokens: {case_id}")
        require(budget.get("slack") == minimum_budget_slack, f"certificate does not record minimum cross-arm slack: {case_id}")
        require(meta.get("schema") == 1 and meta.get("case_id") == case_id, f"expected metadata identity mismatch: {case_id}")
        require(meta.get("is_error") is False and meta.get("content_kind") == "text", f"expected metadata is not ordinary success: {case_id}")
        require(meta.get("normalized_text_sha256") == sha256(text), f"expected text hash mismatch: {case_id}")
        require(common_hash == sha256(text), f"golden differs from stability hash: {case_id}")
        manifest_case = declared_cases[case_id]
        require(isinstance(manifest_case, dict), f"manifest case evidence missing: {case_id}")
        require(manifest_case.get("fixture_tree_sha256") == tree_hash, f"manifest fixture tree differs: {case_id}")
        require(manifest_case.get("common_normalized_sha256") == common_hash, f"manifest case hash mismatch: {case_id}")
        require(manifest_case.get("minimum_budget_slack") == budget["slack"], f"manifest slack mismatch: {case_id}")
        require(manifest_case.get("replacement_counts") == arm_replacement_counts, f"manifest replacement counts differ: {case_id}")
        require(manifest_case.get("readback_sort_keys") == expected_keys, f"manifest sort keys differ: {case_id}")
        require(manifest_case.get("stability") == stability, f"manifest stability projection differs: {case_id}")
        expected_assets = {
            name: file_evidence(directory / name)
            for name in (
                "request.json",
                "env.json",
                "determinism-certificate.json",
                "stability.json",
                "expected.text",
                "expected.meta.json",
            )
        }
        require(manifest_case.get("assets") == expected_assets, f"manifest case asset hashes differ: {case_id}")
        require(set(arm_replacement_counts.values()) == {meta.get("replacement_count")}, f"golden replacement count differs across arms: {case_id}")
        expected[case_id] = {"text": text, "meta": json.dumps(meta, sort_keys=True).encode("utf-8")}
    verify_case_coverage(requests)
    return expected


def substitute_root(value: Any, root: str) -> Any:
    if isinstance(value, str):
        return value.replace(ROOT_TOKEN, root)
    if isinstance(value, list):
        return [substitute_root(item, root) for item in value]
    if isinstance(value, dict):
        return {key: substitute_root(item, root) for key, item in value.items()}
    return value


def shuffled_case_order(case_ids: list[str], seed: int) -> list[str]:
    indices = list(range(len(case_ids)))
    state = seed
    mask = (1 << 64) - 1
    for cursor in range(len(indices) - 1, 0, -1):
        state ^= (state << 13) & mask
        state ^= state >> 7
        state ^= (state << 17) & mask
        state &= mask
        selected = state % (cursor + 1)
        indices[cursor], indices[selected] = indices[selected], indices[cursor]
    return [case_ids[index] for index in indices]


def parse_jsonl_frames(data: bytes, label: str) -> tuple[list[dict[str, Any]], list[bytes]]:
    require(bool(data) and data.endswith(b"\n"), f"{label} is empty or lacks a final newline")
    raw_frames = data.splitlines(keepends=True)
    frames: list[dict[str, Any]] = []
    for index, raw in enumerate(raw_frames, 1):
        require(raw not in (b"\n", b"\r\n"), f"{label} contains a blank frame at {index}")
        try:
            frame = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            fail(f"{label} frame {index} is malformed: {error}")
        require(isinstance(frame, dict), f"{label} frame {index} is not an object")
        frames.append(frame)
    return frames, raw_frames


def valid_capture_root(platform: str, root: Any) -> bool:
    if not isinstance(root, str) or not root or root == ROOT_TOKEN or not root.isascii() or "\\" in root:
        return False
    if platform == "windows-x64":
        match = re.fullmatch(r"[A-Za-z]:/(.+)", root)
        if match is None:
            return False
        parts = match.group(1).split("/")
    else:
        if not root.startswith("/"):
            return False
        parts = root[1:].split("/")
    return len(parts) >= 2 and all(part not in ("", ".", "..") for part in parts) and parts[-1].startswith("fastctx-v011")


def verify_raw_transcripts(
    assets: pathlib.Path,
    manifest: dict[str, Any],
    platforms: list[str],
    expected: dict[str, dict[str, bytes]],
    fixture_tree_sha256: str,
) -> None:
    capture_log = read_json(assets / "provenance" / "capture-log.json")
    require(isinstance(capture_log, dict), "capture log root is not an object")
    require(capture_log.get("schema") == 1, "capture log schema must be 1")
    captures = capture_log.get("captures")
    require(isinstance(captures, dict), "capture log arms are missing")
    require(captures == manifest.get("captures"), "manifest capture log projection differs")
    expected_keys = {f"{platform}/{oracle}" for platform in platforms for oracle in ORACLES}
    require(set(captures) == expected_keys, "capture log matrix differs from the audited arms")
    case_ids = sorted(expected)
    fixture_spec = read_json(assets / "fixture-spec.json")
    require(isinstance(fixture_spec, dict) and isinstance(fixture_spec.get("files"), list), "fixture spec is invalid during raw replay")
    fixture_roots: set[str] = set()
    for platform in platforms:
        platform_harness: tuple[str, int] | None = None
        for oracle in ORACLES:
            key = f"{platform}/{oracle}"
            entry = captures.get(key)
            require(isinstance(entry, dict), f"capture log has no {key}")
            require(entry.get("platform") == platform and entry.get("oracle") == oracle, f"capture identity mismatch: {key}")
            require(entry.get("binary_version") == "fastctx 0.1.1", f"binary version mismatch: {key}")
            require(is_sha256(entry.get("binary_sha256")) and isinstance(entry.get("binary_size"), int) and entry["binary_size"] > 0, f"binary evidence invalid: {key}")
            require(is_sha256(entry.get("capture_harness_sha256")) and isinstance(entry.get("capture_harness_size"), int) and entry["capture_harness_size"] > 0, f"capture harness evidence invalid: {key}")
            require(entry.get("fixture_tree_sha256") == fixture_tree_sha256, f"fixture tree evidence differs: {key}")
            require(entry.get("immutable_readback_checks") == 2 + 32 * len(case_ids), f"immutable readback count mismatch: {key}")
            require(entry.get("fresh_home_per_invocation") is True, f"capture HOME isolation missing: {key}")
            require(entry.get("environment_profile") == "fresh-home-c-utf8-utc-no-git-v1", f"capture environment isolation differs: {key}")
            require(entry.get("isolated_git_parent") is True, f"capture Git-context isolation missing: {key}")
            require(entry.get("runs_per_case") == 32, f"capture run count mismatch: {key}")
            root = entry.get("fixture_root")
            require(valid_capture_root(platform, root), f"capture root invalid: {key}")
            require(root not in fixture_roots, f"capture root was reused by multiple arms: {key}")
            fixture_roots.add(root)
            require(not any(root in item["text"] for item in fixture_spec["files"]), f"capture root collides with fixture text: {key}")
            current_harness = (entry["capture_harness_sha256"], entry["capture_harness_size"])
            if platform_harness is None:
                platform_harness = current_harness
            else:
                require(current_harness == platform_harness, f"source/release used different capture executables: {platform}")
            raw_dir = assets / "raw" / platform
            stdin = read_bytes(raw_dir / f"{oracle}.stdin.jsonl")
            stdout = read_bytes(raw_dir / f"{oracle}.stdout.jsonl")
            stderr = read_bytes(raw_dir / f"{oracle}.stderr.bin")
            require(sha256(stdin) == entry.get("stdin_sha256"), f"stdin transcript hash mismatch: {key}")
            require(sha256(stdout) == entry.get("stdout_sha256"), f"stdout transcript hash mismatch: {key}")
            require(sha256(stderr) == entry.get("stderr_sha256"), f"stderr transcript hash mismatch: {key}")
            require(stderr == b"", f"ordinary capture emitted stderr: {key}")
            stdin_frames, _ = parse_jsonl_frames(stdin, f"{key} stdin")
            stdout_frames, stdout_raw_frames = parse_jsonl_frames(stdout, f"{key} stdout")
            order = entry.get("case_order")
            require(isinstance(order, list), f"capture case order missing: {key}")
            require(entry.get("seed") == 0xFACC_011, f"capture seed mismatch: {key}")
            require(order == shuffled_case_order(case_ids, entry["seed"]), f"capture case order mismatch: {key}")
            require(len(order) == len(set(order)) == len(expected), f"capture case order is not a permutation: {key}")
            require(len(stdin_frames) == len(order) * 3, f"stdin frame count mismatch: {key}")
            require(len(stdout_frames) == len(order) * 2, f"stdout frame count mismatch: {key}")
            for position, case_id in enumerate(order):
                initialize, initialized, call = stdin_frames[position * 3 : position * 3 + 3]
                initialize_response, response = stdout_frames[position * 2 : position * 2 + 2]
                require(initialize == {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "clientInfo": {"name": "compat-v011-capture", "version": "1.0"},
                    },
                }, f"initialize request mismatch: {key}/{case_id}")
                require(initialized == {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}, f"initialized notification mismatch: {key}/{case_id}")
                require(initialize_response.get("jsonrpc") == "2.0" and initialize_response.get("id") == 1 and "result" in initialize_response and "error" not in initialize_response, f"initialize response mismatch: {key}/{case_id}")
                request = read_json(assets / "cases" / case_id / "request.json")
                expected_call = {
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {"name": request["tool"], "arguments": substitute_root(request["arguments"], root)},
                }
                require(call == expected_call, f"raw tools/call request mismatch: {key}/{case_id}")
                require(response.get("jsonrpc") == "2.0" and response.get("id") == 2 and "result" in response and "error" not in response, f"tools/call response identity mismatch: {key}/{case_id}")
                result = response.get("result")
                require(isinstance(result, dict), f"tools/call result is not an object: {key}/{case_id}")
                content = result.get("content")
                require(isinstance(content, list) and len(content) == 1 and isinstance(content[0], dict), f"raw response content shape differs: {key}/{case_id}")
                require(result.get("isError", False) is False and content[0].get("type") == "text", f"raw response is not ordinary text: {key}/{case_id}")
                text = content[0].get("text")
                require(isinstance(text, str), f"raw response text missing: {key}/{case_id}")
                replacement_count = text.count(root)
                normalized = text.replace(root, ROOT_TOKEN).encode("utf-8")
                require(normalized == expected[case_id]["text"], f"raw replay differs from golden: {key}/{case_id}")
                meta = read_json(assets / "cases" / case_id / "expected.meta.json")
                require(replacement_count == meta["replacement_count"], f"raw replacement count differs: {key}/{case_id}")
                stability = read_json(assets / "cases" / case_id / "stability.json")
                require(isinstance(stability, dict), f"stability ledger is not an object: {case_id}")
                ledger = stability["platforms"][platform]["oracles"][oracle]
                invocation_stdout = b"".join(stdout_raw_frames[position * 2 : position * 2 + 2])
                require(sha256(invocation_stdout) == ledger["raw_stdout_sha256"][0], f"representative raw stdout hash differs from run zero: {key}/{case_id}")
                require(sha256(normalized) == ledger["normalized_sha256"][0], f"representative normalized hash differs from run zero: {key}/{case_id}")


def parse_sha256_lines(path: pathlib.Path) -> dict[str, str]:
    output: dict[str, str] = {}
    for number, raw_line in enumerate(read_bytes(path).decode("utf-8").splitlines(), 1):
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", raw_line)
        require(match is not None, f"malformed SHA-256 line {path}:{number}")
        digest, name = match.groups()
        require(name not in output, f"duplicate SHA-256 label in {path}: {name}")
        output[name] = digest
    require(bool(output), f"empty SHA-256 ledger: {path}")
    return output


def verify_provenance(assets: pathlib.Path, manifest: dict[str, Any], repository: pathlib.Path) -> None:
    provenance = assets / "provenance"
    for name in (
        "source.json",
        "capture-log.json",
        "oracle-source-tree.sha256",
        "cargo-lock.sha256",
        "oracle-binaries.sha256",
        "release-assets.sha256",
        "release-assets.json",
        "generator-source.sha256",
        "toolchains.json",
    ):
        require((provenance / name).is_file(), f"missing provenance file: {name}")
    source = read_json(provenance / "source.json")
    require(source == {
        "schema": 1,
        "tag": "v0.1.1",
        "commit": EXPECTED_COMMIT,
        "tree": EXPECTED_TREE,
        "version": "0.1.1",
        "source_archive_sha256": EXPECTED_SOURCE_ARCHIVE,
        "cargo_lock_sha256": EXPECTED_CARGO_LOCK,
    }, "source provenance identity differs from the audited source")
    require(manifest.get("source") == source, "manifest source projection differs")
    oracle_hashes = parse_sha256_lines(provenance / "oracle-binaries.sha256")
    require(set(oracle_hashes) == set(manifest["captures"]), "oracle binary ledger arm set differs")
    for key, capture in manifest["captures"].items():
        require(oracle_hashes.get(key) == capture["binary_sha256"], f"oracle binary hash ledger mismatch: {key}")
    release_assets = parse_sha256_lines(provenance / "release-assets.sha256")
    require(release_assets == EXPECTED_RELEASE_ASSETS, "release asset ledger differs from official v0.1.1 hashes")
    require(manifest.get("release_assets") == release_assets, "manifest release asset projection differs")
    release_metadata = read_json(provenance / "release-assets.json")
    require(release_metadata.get("schema") == 1 and release_metadata.get("release_id") == 356398977, "release metadata identity differs")
    require(release_metadata.get("tag") == "v0.1.1" and release_metadata.get("published_at") == "2026-07-19T17:46:17Z", "release metadata tag/time differs")
    metadata_assets = release_metadata.get("assets")
    require(isinstance(metadata_assets, dict) and set(metadata_assets) == set(EXPECTED_RELEASE_ASSETS), "release metadata asset set differs")
    for name, expected_digest in EXPECTED_RELEASE_ASSETS.items():
        evidence = metadata_assets[name]
        require(isinstance(evidence, dict), f"release metadata is invalid: {name}")
        require(evidence.get("sha256") == expected_digest, f"release metadata hash differs: {name}")
        expected_asset_id, expected_size = EXPECTED_RELEASE_DETAILS[name]
        require(evidence.get("asset_id") == expected_asset_id, f"release asset id differs: {name}")
        require(evidence.get("bytes") == expected_size, f"release asset size differs: {name}")
        require(evidence.get("url") == f"https://github.com/yc-duan/fastctx/releases/download/v0.1.1/{name}", f"release asset URL differs: {name}")
    require(manifest.get("release_asset_metadata") == release_metadata, "manifest release metadata projection differs")
    generator_path = provenance / "generator-source.sha256"
    generator_sources = parse_sha256_lines(generator_path)
    capture = repository / "tools" / "compat-v011-capture"
    expected_generator_labels = {"Cargo.toml", "Cargo.lock", "README.md", "../verify-v011-assets.py"} | {
        f"src/{path.name}" for path in (capture / "src").glob("*.rs")
    }
    require(set(generator_sources) == expected_generator_labels, "generator source ledger inventory differs from current source")
    for label, digest in generator_sources.items():
        unresolved_source_path = capture / label
        require(not unresolved_source_path.is_symlink(), f"generator source is a symlink: {label}")
        source_path = unresolved_source_path.resolve(strict=True)
        require(source_path.is_file(), f"generator source is not a regular file: {label}")
        require(sha256(read_bytes(source_path)) == digest, f"generator source hash differs: {label}")
    generator = manifest.get("generator")
    require(isinstance(generator, dict), "manifest generator projection is missing")
    require(generator.get("sources") == generator_sources, "manifest generator source projection differs")
    require(generator.get("source_ledger_sha256") == sha256(read_bytes(generator_path)), "manifest generator ledger hash differs")
    require(generator.get("verifier_sha256") == generator_sources["../verify-v011-assets.py"], "manifest verifier hash differs")
    lock_hashes = parse_sha256_lines(provenance / "cargo-lock.sha256")
    require(lock_hashes == {"v0.1.1-Cargo.lock": EXPECTED_CARGO_LOCK}, "source Cargo.lock ledger differs")
    tree_hashes = parse_sha256_lines(provenance / "oracle-source-tree.sha256")
    require(tree_hashes == {"git-archive-v0.1.1.tar": EXPECTED_SOURCE_ARCHIVE}, "source archive ledger differs")
    toolchains = read_json(provenance / "toolchains.json")
    require(toolchains.get("schema") == 1, "toolchain provenance schema differs")
    platform_toolchains = toolchains.get("platforms")
    require(isinstance(platform_toolchains, dict) and set(platform_toolchains) == set(PLATFORMS), "toolchain provenance platform set differs")
    for platform, target in PLATFORM_TARGETS.items():
        evidence = platform_toolchains[platform]
        require(isinstance(evidence, dict), f"toolchain evidence missing: {platform}")
        require(evidence.get("target") == target, f"toolchain target differs: {platform}")
        for field in ("runner", "rustc", "cargo"):
            require(isinstance(evidence.get(field), str) and evidence[field], f"toolchain {field} missing: {platform}")
        require(evidence.get("source_build") == "FASTCTX_DISTRIBUTION=github-release cargo build --locked --release", f"source build command differs: {platform}")
        require(evidence.get("release_asset") == PLATFORM_RELEASE_ASSETS[platform], f"release asset association differs: {platform}")
    require(manifest.get("toolchains") == toolchains, "manifest toolchain projection differs")


def verify_capture_independence(repository: pathlib.Path) -> None:
    capture = repository / "tools" / "compat-v011-capture"
    cargo = tomllib.loads(read_bytes(capture / "Cargo.toml").decode("utf-8"))
    dependency_tables: list[tuple[str, dict[str, Any]]] = []

    def collect_dependency_tables(value: Any, path: str = "") -> None:
        if not isinstance(value, dict):
            return
        for name, child in value.items():
            child_path = f"{path}.{name}" if path else name
            if name in ("dependencies", "dev-dependencies", "build-dependencies"):
                require(isinstance(child, dict), f"capture {child_path} is not a table")
                dependency_tables.append((child_path, child))
            else:
                collect_dependency_tables(child, child_path)

    collect_dependency_tables(cargo)
    for table_name, dependencies in dependency_tables:
        for name, value in dependencies.items():
            package = value.get("package", name) if isinstance(value, dict) else name
            require(name != "fastctx" and package != "fastctx", f"capture {table_name} declares a FastCtx dependency")
            if isinstance(value, dict):
                require("path" not in value, f"capture dependency {name} is path-coupled")
    lock = read_bytes(capture / "Cargo.lock").decode("utf-8")
    require('name = "fastctx"' not in lock, "capture lockfile contains FastCtx")
    source_text = "\n".join(path.read_text(encoding="utf-8") for path in sorted((capture / "src").glob("*.rs")))
    require("fastctx::" not in source_text, "capture source imports FastCtx")
    try:
        completed = subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--offline",
                "--no-deps",
                "--format-version",
                "1",
                "--manifest-path",
                str(capture / "Cargo.toml"),
            ],
            cwd=repository,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        fail(f"cannot run cargo metadata for capture independence: {error}")
    require(completed.returncode == 0, f"cargo metadata failed for capture harness: {completed.stderr.decode('utf-8', 'replace').strip()}")
    metadata = json.loads(completed.stdout)
    packages = metadata.get("packages", [])
    require(not any(package.get("name") == "fastctx" for package in packages), "capture dependency graph contains FastCtx")
    capture_package = next((package for package in packages if package.get("name") == "compat-v011-capture"), None)
    require(isinstance(capture_package, dict), "capture package missing from cargo metadata")
    require(not any(dependency.get("path") for dependency in capture_package.get("dependencies", [])), "capture cargo metadata contains a path dependency")


def verify_daily_ci_policy(repository: pathlib.Path, assets: pathlib.Path) -> None:
    forbidden_suffixes = (".exe", ".dll", ".dylib", ".so", ".zip", ".tar", ".tar.gz", ".tgz")
    for relative in relative_files(assets):
        lower = relative.lower()
        require(not lower.endswith(forbidden_suffixes), f"old binary or release archive entered static assets: {relative}")
        data = read_bytes(assets / relative)
        require(not data.startswith((b"MZ", b"\x7fELF", b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf")), f"executable payload entered static assets: {relative}")
    workflows = repository / ".github" / "workflows"
    for workflow in sorted(workflows.glob("*.yml")) + sorted(workflows.glob("*.yaml")):
        text = workflow.read_text(encoding="utf-8")
        require("compat-v011-capture" not in text, f"daily workflow invokes the one-time capture harness: {workflow.name}")
        require("releases/download/v0.1.1" not in text, f"daily workflow downloads the old release: {workflow.name}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assets", type=pathlib.Path, default=pathlib.Path("tests/compat/v0_1_1"))
    parser.add_argument("--repository", type=pathlib.Path, default=pathlib.Path("."))
    arguments = parser.parse_args()
    assets = arguments.assets.resolve(strict=True)
    repository = arguments.repository.resolve(strict=True)
    manifest, platforms = verify_manifest(assets)
    spec, tree_hash, entries = verify_fixture(assets)
    expected = verify_cases(assets, manifest, platforms, spec, tree_hash, entries)
    verify_raw_transcripts(assets, manifest, platforms, expected, tree_hash)
    verify_provenance(assets, manifest, repository)
    verify_capture_independence(repository)
    verify_daily_ci_policy(repository, assets)
    print(json.dumps({"status": "pass", "cases": len(expected), "platforms": platforms}, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except VerificationError as error:
        print(f"verify-v011-assets: {error}", file=sys.stderr)
        raise SystemExit(1)
    except (IndexError, KeyError, TypeError, ValueError) as error:
        print(f"verify-v011-assets: invalid asset structure: {error}", file=sys.stderr)
        raise SystemExit(1)
