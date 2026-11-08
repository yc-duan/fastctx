#!/usr/bin/env python3
"""Keep synchronization contracts below their engine and network adapters."""
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
LOCAL = {
    "fastctx-types": set(),
    "fastctx-platform": {"fastctx-types"},
    "fastctx-storage": {"fastctx-platform"},
    "fastctx-sync": {"fastctx-types", "fastctx-platform", "fastctx-storage"},
}


def sections(manifest):
    for name in ("dependencies", "dev-dependencies", "build-dependencies"):
        yield from manifest.get(name, {}).items()
    for target in manifest.get("target", {}).values():
        yield from sections(target)


def check():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    for package, allowed in LOCAL.items():
        path = ROOT / "crates" / package / "Cargo.toml"
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
            if dependency.startswith("fastctx-") and dependency not in allowed:
                raise ValueError(f"forbidden foundation dependency: {package} -> {dependency}")
            if package == "fastctx-sync" and dependency in {"reqwest", "hyper", "quinn", "rustls", "tungstenite"}:
                raise ValueError(f"network adapters belong outside fastctx-sync: {dependency}")
    print("sync dependencies: types/platform/storage/engine direction preserved")


if __name__ == "__main__":
    try:
        check()
    except (ValueError, OSError, KeyError) as error:
        sys.exit(f"sync dependency check refused: {error}")
