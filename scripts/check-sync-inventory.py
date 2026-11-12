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
    kinds = dict(re.findall(r'kind!\(\s*"([^"]+)",\s*(\w+),', (registry / "kinds.rs").read_text(encoding="utf-8")))
    service_counts = {}
    for category, classification in [("runtime", "Runtime"), ("derived", "Derived")]:
        service = rows(registry / f"{category}_tables.tsv")
        service_counts[category] = len(service)
        for record in service:
            expected = 3 if category == "runtime" else 6
            if len(record) != expected or any(not value for value in record):
                raise ValueError("service table registration has incomplete fields")
            table, kind, owner = record[:3]
            if table in tables or kinds.get(kind) != classification or not (ROOT / owner).is_file():
                raise ValueError(f"invalid current {category} table: {record}")
            if category == "derived":
                sources, version, rebuild = record[3:]
                if not version.isdigit() or int(version) == 0:
                    raise ValueError(f"invalid derived schema version: {record}")
                file, anchor = rebuild.rsplit("::", 1)
                if file != owner or anchor not in (ROOT / file).read_text(encoding="utf-8"):
                    raise ValueError(f"missing derived rebuilder: {record}")
            tables.add(table)
    # The asset libraries' tables are registered with the file that creates
    # them: the same name may be a table of both library versions.
    asset_tables = set()
    for record in rows(registry / "asset_tables.tsv"):
        if len(record) != 2 or any(not value for value in record):
            raise ValueError("asset table registration has incomplete fields")
        table, owner = record
        if (table, owner) in asset_tables or not owner.startswith("crates/fastctx-content/src/") or not (ROOT / owner).is_file():
            raise ValueError(f"invalid asset table: {record}")
        asset_tables.add((table, owner))
    created = set()
    # A table however its name is quoted, in Rust and in the SQL it loads:
    # a quoted name whole, whatever it holds (S13).
    creating = re.compile(
        r"CREATE\s+(?:TEMP\s+|TEMPORARY\s+)?TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?"
        r"(?:\\?\"((?:[^\"\\]|\\[^\"])+)\\?\"|`([^`]+)`|\[([^\]]+)\]|([A-Za-z_][A-Za-z0-9_]*))",
        re.I,
    )
    for path in sorted((ROOT / "crates").rglob("*.rs")) + sorted((ROOT / "crates").rglob("*.sql")):
        name = path.as_posix()
        if "/src/" not in name or "/fastctx-sync/" in name or any(x in path.name for x in ("test", "drill")):
            continue
        owner = path.relative_to(ROOT).as_posix()
        for groups in creating.findall(path.read_text(encoding="utf-8")):
            table = next(group for group in groups if group).lower()
            if "/fastctx-content/" in name:
                if (table, owner) not in asset_tables:
                    raise ValueError(f"unregistered asset table {table}: {owner}")
                created.add((table, owner))
            elif table not in tables and "temp." + table not in tables:
                raise ValueError(f"unregistered production table {table}: {owner}")
    if asset_tables - created:
        raise ValueError(f"registered asset tables no longer created: {sorted(asset_tables - created)}")
    # The registry crate's own tables: the transitional carrier and the
    # format-upgrade journal, which is registered as a runtime table.
    sources = ROOT / "crates/fastctx-sync/src"
    connection = sqlite3.connect(":memory:")
    connection.executescript((sources / "migration/legacy_pins.sql").read_text()
                             + (sources / "format_upgrade/schema.sql").read_text())
    actual = {r[0] for r in connection.execute("SELECT name FROM sqlite_schema WHERE type='table'")}
    declared = {"sync_legacy_pins", "sync_format_upgrades"}
    if actual != declared:
        raise ValueError(f"the registry crate's tables differ from their closed set: {actual ^ declared}")
    if "sync_format_upgrades" not in tables:
        raise ValueError("the format-upgrade journal is not a registered runtime table")
    carriers = rows(registry / "transitional.tsv")
    approved = {("sync_legacy_pins", "Asset", "migration.legacy-pins",
                 "crates/fastctx-sync/src/migration.rs", "Migration",
                 "Canonical import and reference verification, then empty and drop")}
    if len(carriers) != len({tuple(row) for row in carriers}) or not {tuple(row) for row in carriers} <= approved:
        raise ValueError("unregistered transitional carrier")
    if {row[0] for row in carriers} != actual & {row[0] for row in approved}:
        raise ValueError("a transitional carrier must leave the inventory with its table")
    print(f"sync inventory: {len(entries)} entries; {service_counts['runtime']} current runtime tables; {service_counts['derived']} derived tables; {len(asset_tables)} asset tables; {len(actual)} registry tables; baseline evidence verified")

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", type=Path, default=ROOT / "crates/fastctx-sync/src/registry")
    args = parser.parse_args()
    try:
        check(args.registry)
    except (ValueError, OSError) as error:
        parser.exit(1, f"sync inventory refused: {error}\n")
