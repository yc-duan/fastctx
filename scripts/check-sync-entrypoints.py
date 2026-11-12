#!/usr/bin/env python3
"""Restrict migration and transaction privileges to their registered callers."""
import importlib.util
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("sync_wire", ROOT / "scripts/check-sync-wire.py")
wire = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wire)
CALLERS = {
    "SqliteUpgradeJournal": {
        "crates/fastctx-hub/src/store/format.rs",
        "crates/fastctx-node/src/record_database/format.rs",
        "crates/fastctx-sync/src/format_upgrade.rs",
    },
    "prepare_format_upgrade": {
        "crates/fastctx-hub/src/store/format.rs",
        "crates/fastctx-node/src/record_database/format.rs",
        "crates/fastctx-sync/src/format_upgrade.rs",
    },
    "write_unpublished": {
        "crates/fastctx-platform/src/durable_file.rs",
    },
    "delete_unreferenced": {
        "crates/fastctx-platform/src/durable_file.rs",
        "crates/fastctx-content/src/v2/collect.rs",
    },
    "enter_migration": {
        "crates/fastctx-node/src/conversation_store/transfer/triggers.rs",
        "crates/fastctx-storage/src/guard.rs",
        "crates/fastctx-storage/tests/storage_drill/guards.rs",
    },
    "transaction_control": {
        "crates/fastctx-storage/src/authorization.rs",
        "crates/fastctx-storage/src/writer.rs",
    },
    "preserve_step": {
        "crates/fastctx-node/src/run_facts/sql.rs",
        "crates/fastctx-sync/src/migration.rs",
    },
    "initialize_carrier": {
        "crates/fastctx-sync/src/migration.rs",
    },
}


# The asset libraries' files are named only in the modules that define them,
# and opened only by registered callers (sync-v2-impl-design 11); the content
# crate itself, its tests included, owns them.
ASSET_FILE_NAMES = {
    '"content.sqlite3"': "crates/fastctx-content/src/lib.rs",
    '"objects.sqlite3"': "crates/fastctx-content/src/objects.rs",
    '"assets.sqlite3"': "crates/fastctx-content/src/v2/mod.rs",
    '"asset-bytes.sqlite3"': "crates/fastctx-content/src/v2/mod.rs",
    '"asset-objects"': "crates/fastctx-content/src/v2/mod.rs",
}
ASSET_OPENERS = {
    "crates/fastctx-node/src/lib.rs",
    "crates/fastctx-node/src/assets.rs",
    "crates/fastctx-hub/src/assets/owners.rs",
}
ASSET_OPENING = re.compile(
    r"\b(?:Content|Objects|Library)\s*::\s*open(?:_hub)?\b|\b(?:OBJECTS_DIRECTORY|OBJECTS_FILE|BYTES_FILE)\b"
    r"|\bfastctx_content\s*::\s*(?:v2\s*::\s*)?FILE\b"
)


def check_assets(path, source):
    for literal, owner in ASSET_FILE_NAMES.items():
        if literal in source and path != owner and not path.startswith("crates/fastctx-content/"):
            raise ValueError(f"an asset library file named outside its module: {path}")
    if path.startswith("crates/fastctx-content/") or "fastctx_content" not in source:
        return
    if ASSET_OPENING.search(wire.code(source)) and path not in ASSET_OPENERS:
        raise ValueError(f"unregistered asset library opener: {path}")


def check_source(path, source):
    tokens = set(re.findall(r"\b[A-Za-z_]\w*\b", wire.code(source)))
    for symbol, owners in CALLERS.items():
        if symbol in tokens and path not in owners:
            raise ValueError(f"unregistered privileged entry point {symbol}: {path}")
    if path == "crates/fastctx-storage/src/guard.rs" and "suspend" in tokens:
        raise ValueError("blanket guard suspension cannot return")
    masked = wire.code(source)
    if re.search(r"\bmigration\s*::\s*\*", masked) or re.search(r"\bmigration\s*::\s*\{[^}]*\*", masked):
        raise ValueError(f"migration privileges require named imports: {path}")


def check():
    for directory in (ROOT / "crates", ROOT / "gui/src-tauri"):
        for path in directory.rglob("*.rs"):
            if "target" in path.parts:
                continue
            source = path.read_text(encoding="utf-8")
            relative = path.relative_to(ROOT).as_posix()
            if any(symbol in source for symbol in CALLERS) or "migration" in source:
                check_source(relative, source)
            check_assets(relative, source)
    print("sync entry points: migration preservation, transaction control and asset library opening have only registered callers")


if __name__ == "__main__":
    try:
        check()
    except (ValueError, OSError) as error:
        sys.exit(f"sync entry point check refused: {error}")
