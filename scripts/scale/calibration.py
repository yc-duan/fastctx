"""Independently scan immutable fixture bytes and compare them with the manifest."""
import argparse
import hashlib
import json
from pathlib import Path
from manifest import read_manifest


def verify(root):
    """Reject size, hash, record-count or long-line drift with bounded scan buffers."""
    manifest = read_manifest(root)
    total = 0
    for entry in manifest["files"]:
        digest, size, lines, tail, longest = hashlib.sha256(), 0, 0, 0, 0
        with (root / entry["path"]).open("rb") as source:
            while block := source.read(1 << 20):
                digest.update(block)
                size += len(block)
                if entry["kind"] in ("claude", "subagent"):
                    parts = block.split(b"\n")
                    if len(parts) > 1:
                        longest = max(longest, tail + len(parts[0]) + 1,
                                      max(map(len, parts[1:-1]), default=-1) + 1)
                        lines += len(parts) - 1
                        tail = len(parts[-1])
                    else:
                        tail += len(block)
        if size != entry["bytes"] or digest.hexdigest() != entry["sha256"]:
            raise ValueError(f"size/hash mismatch: {entry['path']}")
        if "records" in entry and (lines != entry["records"] or lines != entry["lines"]
                                   or max(longest, tail) != entry["longest_line"]):
            raise ValueError(f"record/line calibration mismatch: {entry['path']}")
        total += size
    if total != manifest["total_bytes"]:
        raise ValueError("total dataset size mismatch")
    actual = sum(e["records"] for e in manifest["files"] if "records" in e)
    expected = manifest["workload_rows"] + 4 * len(manifest["conversations"]) + 3
    if actual != expected:
        raise ValueError(f"workload row count mismatch: {actual} != {expected}")
    return dict(bytes=total, files=len(manifest["files"]), rows=actual, sha256="verified")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    print(json.dumps(verify(parser.parse_args().root)))
