"""Versioned manifest IO for immutable scale fixtures; see MANIFEST.md."""
import json

FORMAT_VERSION = 1


def read_manifest(root):
    """Read a supported manifest without reading any fixture contents."""
    manifest = json.loads((root / "manifest.json").read_bytes())
    if not isinstance(manifest, dict) or manifest.get("format") != FORMAT_VERSION:
        raise ValueError(f"unsupported fixture manifest format; expected {FORMAT_VERSION}")
    return manifest


def write_manifest(root, manifest):
    """Write deterministic UTF-8 JSON; file order is part of the fixture format."""
    if manifest.get("format") != FORMAT_VERSION:
        raise ValueError(f"unsupported fixture manifest format; expected {FORMAT_VERSION}")
    (root / "manifest.json").write_bytes(json.dumps(manifest, indent=2).encode() + b"\n")
