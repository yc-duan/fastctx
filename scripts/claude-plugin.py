"""The World's Claude plugin, as a node of this release writes it, through Claude Code's own checks
(`claude plugin validate --strict` and `claude plugin test`). Claude Code's hook scanner refuses a
module it cannot prove, and then the plugin does not load at all, which no build or contract check of
ours sees. Each Claude Code is the official release of its version, checked against that release's
manifest; it is never logged in and asks no model. The versions are the oldest the World supports
(`crates/fastctx-hosts/src/claude/oldest.rs`) and those given.

    python scripts/claude-plugin.py PROGRAM [VERSION ...]
"""
import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RELEASES = "https://downloads.claude.ai/claude-code-releases"
CHECKED = ["2.1.303"]


def oldest():
    text = (REPO / "crates/fastctx-hosts/src/claude/oldest.rs").read_text(encoding="utf-8")
    return re.search(r'pub const OLDEST: &str = "([^"]+)";', text).group(1)


def target():
    system = {"linux": "linux", "darwin": "darwin", "win32": "win32"}[sys.platform]
    machine = {"x86_64": "x64", "amd64": "x64", "aarch64": "arm64", "arm64": "arm64"}[platform.machine().lower()]
    return f"{system}-{machine}"


def fetch(url):
    with urllib.request.urlopen(url, timeout=600) as answer:
        return answer.read()


def claude(version, into):
    """The official Claude Code of `version` for this machine, its bytes checked against its manifest."""
    manifest = json.loads(fetch(f"{RELEASES}/{version}/manifest.json"))
    entry = manifest["platforms"][target()]
    body = fetch(f"{RELEASES}/{version}/{target()}/{entry['binary']}")
    digest = hashlib.sha256(body).hexdigest()
    if digest != entry["checksum"] or len(body) != entry["size"]:
        raise SystemExit(f"Claude Code {version} for {target()} does not match its manifest")
    path = into / f"claude-{version}" / entry["binary"]
    path.parent.mkdir(parents=True)
    path.write_bytes(body)
    path.chmod(0o755)
    return path


def plugin(program, root):
    """The plugin a node of `program` writes into its World as it starts."""
    home = root / "home"
    (home / "bin").mkdir(parents=True)
    installed = home / "bin" / program.name
    shutil.copy2(program, installed)
    env = {**os.environ, "FASTCTX_HOME": str(home), "HOME": str(root), "CLAUDE_CONFIG_DIR": str(root / "claude-home"),
           "CODEX_HOME": str(root / "codex-home"), "FASTCTX_UPDATE_RELEASES": "http://127.0.0.1:9/releases"}
    written = home / "world" / "claude-plugins" / "fastctx-world"
    with open(root / "node.log", "ab") as log:
        node = subprocess.Popen([str(installed), "node"], env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    try:
        deadline = time.monotonic() + 120
        while not (written / "hooks" / "world.js").is_file():
            if time.monotonic() > deadline or node.poll() is not None:
                raise SystemExit("the node wrote no plugin:\n" + (root / "node.log").read_text(errors="replace")[-3000:])
            time.sleep(0.5)
        time.sleep(2)
    finally:
        node.terminate()
        try:
            node.wait(timeout=30)
        except subprocess.TimeoutExpired:
            node.kill()
    return written


def check(binary, version, written, root):
    env = {**os.environ, "CLAUDE_CONFIG_DIR": str(root / f"config-{version}"), "HOME": str(root),
           "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1", "DISABLE_AUTOUPDATER": "1",
           "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"}
    passed = True
    for label, args, timeout in [("validate", ["plugin", "validate", str(written), "--strict", "--json"], 300),
                                 ("test", ["plugin", "test", str(written)], 900)]:
        done = subprocess.run([str(binary), *args], env=env, cwd=root, capture_output=True, text=True,
                              encoding="utf-8", errors="replace", timeout=timeout)
        said = done.stdout + done.stderr
        ok = done.returncode == 0 and (label != "test" or re.search(r"^\s*0 fail\s*$", said, re.M))
        print(("PASS " if ok else "FAIL ") + f"Claude Code {version}: plugin {label}", flush=True)
        if not ok:
            print(said[-6000:], flush=True)
        passed = passed and bool(ok)
    return passed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("program", type=Path)
    parser.add_argument("versions", nargs="*")
    args = parser.parse_args()
    versions = list(dict.fromkeys([oldest(), *(args.versions or CHECKED)]))
    root = Path(tempfile.mkdtemp(prefix="fastctx-claude-plugin-", dir=os.environ.get("RUNNER_TEMP")))
    try:
        written = plugin(args.program.resolve(), root)
        results = [check(claude(version, root), version, written, root) for version in versions]
    finally:
        shutil.rmtree(root, ignore_errors=True)
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
