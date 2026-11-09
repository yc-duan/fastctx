#!/usr/bin/env python3
"""Require an explicit purpose and capability for new world protocol variants."""

import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "crates/fastctx-sync/src/registry"
ROOTS = {
    "crates/fastctx-types/src/calls.rs": ["Call", "Reply"],
    "crates/fastctx-types/src/ipc.rs": ["ClientFrame", "NodeFrame"],
    "crates/fastctx-net/src/wire.rs": ["Up", "Down", "Intent", "Question", "Change", "Told"],
    "crates/fastctx-net/src/peer.rs": ["Frame", "Intent"],
    "crates/fastctx-types/src/remote.rs": ["RemoteCall", "RemoteReply", "Letter"],
    "crates/fastctx-types/src/session.rs": ["Watched"],
    "crates/fastctx-types/src/account_records.rs": ["Watched"],
    "crates/fastctx-types/src/history.rs": ["Watched"],
    "crates/fastctx-types/src/composer/sync.rs": ["Frame"],
    "crates/fastctx-types/src/sync/transfer.rs": ["Message"],
    "crates/fastctx-types/src/sync/session.rs": ["TransportControl"],
}
# New channels require an explicitly named, exhaustive routing function.
DISPATCHERS = {
    "crates/fastctx-types/src/sync/transfer.rs::Message": ("asset_sync", "purpose"),
    "crates/fastctx-types/src/sync/session.rs::TransportControl": ("asset_sync.transport", "purpose"),
    "crates/fastctx-types/src/sync/session.rs::SessionBindingRequest": ("asset_sync.transport", "binding_purpose"),
    "crates/fastctx-net/src/wire.rs::Intent": ("asset_sync.transport", "asset_sync_purpose"),
    "crates/fastctx-net/src/peer.rs::Intent": ("asset_sync.transport", "asset_sync_purpose"),
    "crates/fastctx-types/src/calls.rs::Call": ("node_ipc", "purpose"),
    "crates/fastctx-types/src/calls.rs::Reply": ("node_ipc", "answer_purpose"),
}
FROZEN_SHA256 = "fbccea156dd77cd25a679487d83a21a85a82a0c64f51235f878e020b9b3c5ca2"


def code(source):
    """Mask comments and literals while retaining punctuation and source positions."""
    out = list(source)
    raw = re.compile(r'(?:b|c)?r(#{0,255})"')
    character = re.compile(r"'(?:\\u\{[0-9a-fA-F_]+\}|\\x[0-9a-fA-F]{2}|\\.|[^'\\\n])'")
    index = 0
    while index < len(source):
        begin = index
        if source.startswith("//", index):
            end = source.find("\n", index)
            index = len(source) if end < 0 else end
        elif source.startswith("/*", index):
            depth, index = 1, index + 2
            while depth and index < len(source):
                if source.startswith("/*", index):
                    depth, index = depth + 1, index + 2
                elif source.startswith("*/", index):
                    depth, index = depth - 1, index + 2
                else:
                    index += 1
            if depth:
                raise ValueError("unterminated Rust comment")
        elif source[index] in "brc" and (match := raw.match(source, index)):
            ending = '"' + match[1]
            end = source.find(ending, index + len(match[0]))
            if end < 0:
                raise ValueError("unterminated Rust raw string")
            index = end + len(ending)
        elif source[index] == '"':
            index += 1
            while index < len(source) and source[index] != '"':
                index += 2 if source[index] == "\\" else 1
            index += 1
        elif source[index] == "'" and (match := character.match(source, index)):
            index += len(match[0])
        else:
            index += 1
            continue
        for at in range(begin, min(index, len(source))):
            if out[at] != "\n":
                out[at] = " "
    return "".join(out)


def enums(source):
    masked = code(source)
    found = {}
    for match in re.finditer(r"\benum\s+([A-Za-z_][A-Za-z_0-9]*)\b", masked):
        start = masked.find("{", match.end())
        if start < 0:
            raise ValueError(f"enum has no body: {match[1]}")
        tokens = re.findall(r"[A-Za-z_][A-Za-z_0-9]*|[^\s]", masked[start + 1:])
        depth, expect, variants = [], True, []
        for token in tokens:
            if token == "}" and not depth:
                break
            if not depth and token == ",":
                expect = True
            elif not depth and expect and re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", token):
                variants.append(token)
                expect = False
            if token in "([{":
                depth.append(token)
            elif token in ")]}":
                if not depth:
                    raise ValueError(f"unbalanced enum: {match[1]}")
                depth.pop()
        found[match[1]] = sorted(set(found.get(match[1], ())) | set(variants))
    return found


def selected(path, source):
    definitions = enums(source)
    names = set(ROOTS.get(path, ()))
    if path.startswith("crates/fastctx-types/src/"):
        names.update(name for name in definitions if name.endswith("Request"))
    return {f"{path}::{name}": definitions[name] for name in names if name in definitions}


def current():
    paths = set(ROOTS)
    paths.update(path.relative_to(ROOT).as_posix() for path in (ROOT / "crates/fastctx-types/src").rglob("*.rs"))
    result = {}
    for path in paths:
        file = ROOT / path
        if file.exists():
            result.update(selected(path, file.read_text(encoding="utf-8")))
    return result


def check():
    raw = (REGISTRY / "wire_legacy.json").read_bytes().replace(b"\r\n", b"\n")
    if hashlib.sha256(raw).hexdigest() != FROZEN_SHA256:
        raise ValueError("the legacy wire baseline changed")
    baseline = json.loads(raw)
    # peer::Intent was omitted from the original inventory. These three exact
    # legacy variants predate asset sync; future variants still need mappings.
    baseline["crates/fastctx-net/src/peer.rs::Intent"] = ["Data", "Link", "Probe"]
    actual = current()
    additions = {(key, variant) for key, variants in actual.items()
                 for variant in variants if variant not in baseline.get(key, ())}
    registered = json.loads((REGISTRY / "wire_purposes.json").read_text(encoding="utf-8"))
    mapped = set()
    for item in registered:
        key = (item["type"], item["variant"])
        if key in mapped or item["purpose"] not in {"AssetSync", "ExecutionControl", "Observation", "Bootstrap"}:
            raise ValueError(f"invalid or duplicate wire purpose: {key}")
        dispatcher = DISPATCHERS.get(item["type"])
        if (dispatcher is None or item["capability"] != dispatcher[0]
                or item["mapping"] != item["type"].split("::")[0]):
            raise ValueError(f"new channel needs a registered dispatcher: {key}")
        source = code((ROOT / item["mapping"]).read_text(encoding="utf-8"))
        start = source.index("{", source.index(f"pub fn {dispatcher[1]}("))
        depth, end = 1, start + 1
        while depth and end < len(source):
            depth += (source[end] == "{") - (source[end] == "}")
            end += 1
        body = source[start:end]
        purposes = {}
        for pattern, purpose in re.findall(r"(Self::[^;=]*?)=>\s*(?:\{\s*)?(?:Some\()?Purpose::(\w+)", body, re.S):
            for variant in re.findall(r"\bSelf::(\w+)", pattern):
                if variant in purposes:
                    raise ValueError(f"duplicate purpose mapping: {variant}")
                purposes[variant] = purpose
        if re.search(r"\b_\s*(?:if\b[^=]*)?=>", body) or purposes.get(item["variant"]) != item["purpose"]:
            raise ValueError(f"wire variant lacks its exhaustive purpose mapping: {key}")
        mapped.add(key)
    if additions != mapped:
        raise ValueError(f"new wire variants differ from purpose registry: {sorted(additions ^ mapped)}")
    print(f"sync wire: {len(baseline)} legacy enums; {len(mapped)} new variants have capability and purpose mappings")


if __name__ == "__main__":
    try:
        check()
    except (ValueError, OSError, KeyError) as error:
        sys.exit(f"sync wire check refused: {error}")
