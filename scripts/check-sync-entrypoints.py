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
    "write_unpublished": {
        "crates/fastctx-platform/src/durable_file.rs",
        "crates/fastctx-sync/src/blocks/files.rs",
    },
    "delete_unreferenced": {
        "crates/fastctx-platform/src/durable_file.rs",
        "crates/fastctx-sync/src/blocks/files.rs",
    },
    "from_authenticated": {
        "crates/fastctx-net/src/sync_access/mod.rs",
    },
    "enter_typed_write": {
        "crates/fastctx-sync/src/store/boundary.rs",
        "crates/fastctx-sync/src/scheduler/sqlite.rs",
    },
    "enter_migration": {
        "crates/fastctx-storage/src/guard.rs",
        "crates/fastctx-storage/tests/storage_drill/guards.rs",
        "crates/fastctx-node/src/conversation_store/transfer/triggers.rs",
    },
    "transaction_control": {
        "crates/fastctx-storage/src/authorization.rs",
        "crates/fastctx-storage/src/writer.rs",
    },
    "preserve_step": {
        "crates/fastctx-sync/src/migration.rs",
        "crates/fastctx-node/src/run_facts/sql.rs",
    },
    "initialize_carrier": {
        "crates/fastctx-sync/src/migration.rs",
        "crates/fastctx-sync/src/store/schema.rs",
    },
}


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
            if any(symbol in source for symbol in CALLERS) or "migration" in source:
                check_source(path.relative_to(ROOT).as_posix(), source)
    print("sync entry points: migration preservation and transaction control have only registered callers")


if __name__ == "__main__":
    try:
        check()
    except (ValueError, OSError) as error:
        sys.exit(f"sync entry point check refused: {error}")
