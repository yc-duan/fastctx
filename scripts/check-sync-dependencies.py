#!/usr/bin/env python3
"""Keep synchronization contracts below their engine and network adapters."""
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
LOCAL = {
    "fastctx-types": set(),
    "fastctx-platform": {"fastctx-types"},
    "fastctx-storage": {"fastctx-platform"},
    "fastctx-sync": {"fastctx-types", "fastctx-platform", "fastctx-storage"},
}
# Who may hold a storage connection at all, and who may open an asset library
# through fastctx-content (sync-v2-impl-design 11): a crate added to either is
# a decision, made here.
STORAGE_USERS = {
    "fastctx-sync", "fastctx-content", "fastctx-hub", "fastctx-node", "fastctx-net", "fastctx-jobs",
}
CONTENT_USERS = {"fastctx-hub", "fastctx-node"}
# Who may open SQLite itself, past the storage layer: none other may make a
# connection of its own to any database, an asset library's among them.
RUSQLITE_USERS = {"fastctx-storage", "fastctx-gui"}
# The modules of storage users that open a raw SQLite connection of their own
# through the storage layer's re-export, each for a file that is not an asset
# library: a module added here is a decision, made here (S11). Drills and
# tests open what they like.
RAW_OPENERS = {
    "crates/fastctx-hub/src/assets/keys.rs",
    "crates/fastctx-hub/src/moving/snapshot.rs",
    "crates/fastctx-hub/src/store/reading.rs",
    "crates/fastctx-hub/src/store/format.rs",
    "crates/fastctx-node/src/conversation_store/bundle.rs",
    "crates/fastctx-node/src/conversation_store/transfer/exporting.rs",
    "crates/fastctx-node/src/conversation_store/transfer/importing.rs",
    "crates/fastctx-node/src/conversation_store/transfer/release.rs",
    "crates/fastctx-node/src/conversation_store/transfer/status.rs",
    "crates/fastctx-node/src/timers/headers.rs",
    "crates/fastctx-sync/src/registry/derived.rs",
}
# A raw connection opened, or SQLite's connection type under another name.
RAW_OPENING = re.compile(r"\bConnection\s*::\s*open\w*\b|\bConnection\s+as\s+[A-Za-z_]\w*|\btype\s+\w+\s*=\s*[\w:]*\bConnection\b")


def check_raw_openers():
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        relative = path.relative_to(ROOT).as_posix()
        package = relative.split("/")[1]
        if package in RUSQLITE_USERS or "target" in path.parts or any(part in relative for part in ("test", "drill")):
            continue
        if RAW_OPENING.search(path.read_text(encoding="utf-8")) and relative not in RAW_OPENERS:
            raise ValueError(f"unregistered raw SQLite connection: {relative}")


def sections(manifest):
    for name in ("dependencies", "dev-dependencies", "build-dependencies"):
        yield from manifest.get(name, {}).items()
    for target in manifest.get("target", {}).values():
        yield from sections(target)


def dependencies(path, workspace):
    manifest = tomllib.loads(path.read_text(encoding="utf-8"))
    for alias, entry in sections(manifest):
        entry = entry if isinstance(entry, dict) else {"version": entry}
        dependency_root = path.parent
        if entry.get("workspace"):
            dependency_root = ROOT
            inherited = workspace.get("dependencies", {})[alias]
            inherited = inherited if isinstance(inherited, dict) else {"version": inherited}
            entry = inherited | entry
        dependency = entry.get("package", alias)
        if "path" in entry:
            target = (dependency_root / entry["path"]).resolve() / "Cargo.toml"
            dependency = tomllib.loads(target.read_text(encoding="utf-8"))["package"]["name"]
        yield dependency


def check():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    for package, allowed in LOCAL.items():
        for dependency in dependencies(ROOT / "crates" / package / "Cargo.toml", workspace):
            if dependency.startswith("fastctx-") and dependency not in allowed:
                raise ValueError(f"forbidden foundation dependency: {package} -> {dependency}")
            if package == "fastctx-sync" and dependency in {"reqwest", "hyper", "quinn", "rustls", "tungstenite"}:
                raise ValueError(f"network adapters belong outside fastctx-sync: {dependency}")
    manifests = sorted((ROOT / "crates").glob("*/Cargo.toml")) + [ROOT / "gui/src-tauri/Cargo.toml"]
    for path in manifests:
        if not path.is_file():
            continue
        package = tomllib.loads(path.read_text(encoding="utf-8"))["package"]["name"]
        for dependency in dependencies(path, workspace):
            if dependency == "fastctx-storage" and package not in STORAGE_USERS:
                raise ValueError(f"unlisted storage user: {package}")
            if dependency == "fastctx-content" and package not in CONTENT_USERS:
                raise ValueError(f"unlisted asset library user: {package}")
            if dependency in {"rusqlite", "libsqlite3-sys"} and package not in RUSQLITE_USERS:
                raise ValueError(f"unlisted direct SQLite user: {package}")
    check_raw_openers()
    print("sync dependencies: types/platform/storage/engine direction preserved; storage and asset library users listed")


if __name__ == "__main__":
    try:
        check()
    except (ValueError, OSError, KeyError) as error:
        sys.exit(f"sync dependency check refused: {error}")
