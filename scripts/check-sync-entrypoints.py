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
        "crates/fastctx-sync/src/store/format_upgrade.rs",
        "crates/fastctx-node/src/record_database/format.rs",
        "crates/fastctx-hub/src/store/format.rs",
        "crates/fastctx-sync/tests/sync_drill/format_upgrade.rs",
    },
    "prepare_format_upgrade": {
        "crates/fastctx-sync/src/store/format_upgrade.rs",
        "crates/fastctx-node/src/record_database/format.rs",
        "crates/fastctx-hub/src/store/format.rs",
        "crates/fastctx-sync/tests/sync_drill/format_upgrade.rs",
    },
    "IndexValidationPermit": {
        "crates/fastctx-sync/src/blocks/metadata_custody.rs",
    },
    "SqliteBackend": {
        "crates/fastctx-sync/src/store/engine/mod.rs",
        "crates/fastctx-sync/src/store/engine/pending.rs",
        "crates/fastctx-sync/src/store/engine/prepare.rs",
        "crates/fastctx-sync/src/store/engine/supply.rs",
        "crates/fastctx-sync/tests/sync_drill/pending_queue.rs",
        "crates/fastctx-sync/tests/sync_drill/pending.rs",
    },
    "SqliteBlocks": {
        "crates/fastctx-sync/src/blocks/mod.rs",
        "crates/fastctx-sync/src/blocks/cleanup.rs",
        "crates/fastctx-sync/src/blocks/sqlite.rs",
        "crates/fastctx-sync/src/blocks/gc.rs",
        "crates/fastctx-sync/src/blocks/work.rs",
        "crates/fastctx-sync/src/blocks/metadata_work.rs",
        "crates/fastctx-sync/src/blocks/metadata_custody.rs",
        "crates/fastctx-sync/src/blocks/root_store.rs",
        "crates/fastctx-sync/src/blocks/step_writer.rs",
        "crates/fastctx-sync/src/blocks/source_state.rs",
        "crates/fastctx-sync/src/blocks/source_state/current.rs",
        "crates/fastctx-sync/src/blocks/reconcile/mod.rs",
        "crates/fastctx-sync/src/blocks/reconcile/steps.rs",
        "crates/fastctx-sync/src/blocks/engine_reads.rs",
        "crates/fastctx-sync/src/blocks/source_supply.rs",
        "crates/fastctx-sync/src/blocks/supply/runner.rs",
        "crates/fastctx-sync/tests/sync_drill/supply.rs",
        "crates/fastctx-sync/tests/sync_drill/timeline_hold.rs",
        "crates/fastctx-sync/tests/sync_drill/holding.rs",
        "crates/fastctx-sync/src/store/sources/mod.rs",
        "crates/fastctx-sync/src/store/timelines/mod.rs",
        "crates/fastctx-sync/src/store/timelines/pages.rs",
        "crates/fastctx-sync/src/store/scopes.rs",
        "crates/fastctx-sync/src/blocks/adopted.rs",
        "crates/fastctx-sync/src/blocks/authority_supply.rs",
        "crates/fastctx-sync/src/blocks/tasks.rs",
        "crates/fastctx-hub/src/foundation/mod.rs",
        "crates/fastctx-hub/src/foundation/identity.rs",
        "crates/fastctx-sync/src/store/control.rs",
        "crates/fastctx-sync/src/store/directory/mod.rs",
        "crates/fastctx-sync/tests/sync_drill/directory.rs",
        "crates/fastctx-sync/tests/sync_drill/blocks.rs",
        "crates/fastctx-node/src/foundation/mod.rs",
        "crates/fastctx-node/src/foundation/tasks.rs",
        "crates/fastctx-node/src/foundation/maintenance.rs",
        "crates/fastctx-sync/tests/sync_drill/block_recovery.rs",
        "crates/fastctx-sync/tests/sync_drill/root_usage.rs",
        "crates/fastctx-sync/tests/sync_drill/source_steps.rs",
        "crates/fastctx-sync/tests/sync_drill/source_ingest.rs",
        "crates/fastctx-sync/tests/sync_drill/canonical_producer.rs",
        "crates/fastctx-sync/tests/sync_drill/timeline_install.rs",
        "crates/fastctx-sync/tests/sync_drill/metadata.rs",
        "crates/fastctx-sync/tests/sync_drill/pending_basis.rs",
        "crates/fastctx-sync/tests/sync_drill/pending_queue.rs",
        "crates/fastctx-sync/tests/sync_drill/pending.rs",
    },
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
        "crates/fastctx-net/tests/sync_access_drill/fixture.rs",
    },
    "enter_typed_write": {
        "crates/fastctx-sync/src/store/boundary.rs",
        "crates/fastctx-sync/src/store/format_upgrade.rs",
        "crates/fastctx-sync/src/store/engine/pending.rs",
        "crates/fastctx-sync/src/blocks/cleanup.rs",
        "crates/fastctx-sync/src/scheduler/sqlite.rs",
        "crates/fastctx-sync/src/blocks/sqlite.rs",
        "crates/fastctx-sync/src/blocks/gc.rs",
        "crates/fastctx-sync/src/blocks/work.rs",
        "crates/fastctx-sync/src/blocks/metadata_work.rs",
        "crates/fastctx-sync/src/blocks/root_store.rs",
        "crates/fastctx-sync/src/blocks/step_writer.rs",
        "crates/fastctx-sync/src/store/sources/commit.rs",
        "crates/fastctx-sync/src/store/sources/open.rs",
        "crates/fastctx-sync/src/store/timelines/install.rs",
        "crates/fastctx-sync/src/store/timelines/rebuild.rs",
        "crates/fastctx-sync/src/store/scopes/advance.rs",
        "crates/fastctx-sync/src/blocks/reconcile/mod.rs",
        "crates/fastctx-sync/src/blocks/reconcile/steps.rs",
        "crates/fastctx-sync/src/blocks/supply/mod.rs",
        "crates/fastctx-sync/src/blocks/supply/cleanup.rs",
        "crates/fastctx-sync/src/blocks/supply/invalidate.rs",
        "crates/fastctx-sync/src/blocks/supply/timeline.rs",
        "crates/fastctx-sync/src/store/scopes.rs",
        "crates/fastctx-sync/src/blocks/adopted.rs",
        "crates/fastctx-sync/src/store/control.rs",
        "crates/fastctx-sync/src/store/directory/mod.rs",
        "crates/fastctx-sync/src/store/scopes/install.rs",
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
