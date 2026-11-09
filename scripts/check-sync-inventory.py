#!/usr/bin/env python3
"""Keep the legacy transition inventory closed and verify added source evidence."""
import argparse
import hashlib
from pathlib import Path
import re
import sqlite3

ROOT = Path(__file__).resolve().parents[1]
INVENTORIES = ("legacy.tsv", "legacy_families.tsv", "legacy_files.tsv", "legacy_browser.tsv")

FROZEN = {'legacy_baseline.tsv': 'd07f1dd37e4348ac574fb3357a399cc7fce76b0cf3246e3f0b9b36988ce58614', 'baseline_sources.tsv': '184c389ae42648ae53922007aee830769a12c6c2e752648c98367a89a130704c'}

def rows(path):
    return [line.split("\t") for line in path.read_text(encoding="utf-8").splitlines()
            if line and not line.startswith("#")]

def check(registry):
    for name, expected in FROZEN.items():
        # Git checks out text with either newline convention.
        raw = (registry / name).read_bytes().replace(b"\r\n", b"\n")
        if hashlib.sha256(raw).hexdigest() != expected:
            raise ValueError(f"frozen baseline changed: {name}")
    baseline = {(r[0], r[1]): r[2:] for r in rows(registry / "legacy_baseline.tsv")}
    sources = dict(rows(registry / "baseline_sources.tsv"))
    additions = {}
    for record in rows(registry / "legacy_additions.tsv"):
        if len(record) != 6:
            raise ValueError("added legacy entries need six evidence fields")
        surface, selector, source, blob, file, anchor = record
        if sources.get(source) != blob:
            raise ValueError(f"source is not in the frozen baseline: {source}")
        evidence = (registry / file).resolve()
        if not evidence.is_relative_to((registry / "evidence").resolve()):
            raise ValueError("source evidence must be inside registry/evidence")
        data = evidence.read_bytes()
        actual = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
        if actual != blob or not anchor or anchor.encode() not in data:
            raise ValueError(f"invalid original source evidence: {selector}")
        additions[(surface, selector)] = source
    known = set()
    entries = []
    for name in INVENTORIES:
        for record in rows(registry / name):
            if len(record) != 7:
                raise ValueError(f"inventory needs seven fields: {name}")
            surface, selector, classification, owner, stage, decision, note = record
            key = (surface, selector)
            if key in known:
                raise ValueError(f"duplicate inventory key: {key}")
            known.add(key)
            if key in baseline:
                if owner != baseline[key][0]:
                    raise ValueError(f"legacy owner changed without preserving origin: {key}")
            elif key not in additions:
                raise ValueError(f"new legacy entry lacks baseline source evidence: {key}")
            if not (ROOT / owner).exists():
                raise ValueError(f"missing owner: {owner}")
            if decision not in ("Confirmed", "PendingUser"):
                raise ValueError(f"unknown decision state: {decision}")
            entries.append(record)
    if additions.keys() - known:
        raise ValueError("unused additions evidence")
    tables = {r[1] for r in entries if r[0] in ("Table", "Migration")}
    for path in (ROOT / "crates").rglob("*.rs"):
        name = path.as_posix()
        if "/src/" not in name or "/fastctx-sync/" in name or any(x in path.name for x in ("test", "drill")):
            continue
        for table in re.findall(r"CREATE\s+(?:TABLE|TEMP\s+TABLE)\s+(?:IF\s+NOT\s+EXISTS\s+)?([a-z_][a-z0-9_]*)", path.read_text(encoding="utf-8"), re.I):
            if table not in tables and "temp." + table not in tables:
                raise ValueError(f"unregistered production table {table}: {path.relative_to(ROOT)}")
    schema = ROOT / "crates/fastctx-sync/src/store"
    connection = sqlite3.connect(":memory:")
    connection.executescript("".join((schema / name).read_text() for name in (
        "schema.sql", "schema_timeline_index.sql", "legacy_pins.sql")))
    actual = {r[0] for r in connection.execute("SELECT name FROM sqlite_schema WHERE type='table'")}
    declared = set(re.findall(r'"(sync_[a-z_]+)"', (schema / "schema_tables.rs").read_text()))
    if actual != declared:
        raise ValueError(f"shadow schema differs from its closed set: {actual ^ declared}")
    carriers = rows(registry / "transitional.tsv")
    approved = {("sync_legacy_pins", "Asset", "migration.legacy-pins",
                 "crates/fastctx-sync/src/migration.rs", "Migration",
                 "Canonical import and reference verification, then empty and drop")}
    if len(carriers) != len({tuple(row) for row in carriers}) or not {tuple(row) for row in carriers} <= approved:
        raise ValueError("unregistered transitional carrier")
    if {row[0] for row in carriers} != actual & {row[0] for row in approved}:
        raise ValueError("a transitional carrier must leave the inventory with its table")
    print(f"sync inventory: {len(entries)} entries; {len(actual)} shadow tables; baseline evidence verified")

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", type=Path, default=ROOT / "crates/fastctx-sync/src/registry")
    args = parser.parse_args()
    try:
        check(args.registry)
    except (ValueError, OSError) as error:
        parser.exit(1, f"sync inventory refused: {error}\n")
