"""Decode today's compatible replies with the actual historical Rust readers.

Requires git history for the pinned revisions (CI uses fetch-depth: 0). Sources
are materialized only under target; no historical production program is run.
The declaration baseline separately makes changes outside the sample corpus
visible to reviewers. Run after cargo test -p fastctx-types --test ipc_contract.
"""

import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / "target" / "ipc-contract"
REVISIONS = {
    5: "5534ed866e19c931f469c51e0f434ac2e3b4ea2e",
    6: "f00f76c68d27d74204110c876860e4ab61a7f85f",
    7: "bd905f697fbf793f9f755561f36b0758fb08e383",
}


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT)


def prepare():
    WORK.mkdir(parents=True, exist_ok=True)
    dependencies = []
    for protocol, revision in REVISIONS.items():
        directory = WORK / f"v{protocol}"
        for file in git("ls-tree", "-r", "--name-only", revision, "crates/fastctx-types/src").decode().splitlines():
            destination = directory / file.removeprefix("crates/fastctx-types/")
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(git("show", f"{revision}:{file}"))
        (directory / "Cargo.toml").write_text(f'''[package]
name = "ipc-reader-v{protocol}"
version = "0.0.0"
edition = "2024"
[dependencies]
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
jiff = {{ version = "0.2", features = ["serde"] }}
thiserror = "2"
''', encoding="utf-8")
        dependencies.append(f'v{protocol} = {{ package = "ipc-reader-v{protocol}", path = "v{protocol}" }}')
    manifest = '''[package]
name = "ipc-historical-readers"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
'''
    manifest += f'fastctx-types = {{ path = {json.dumps((ROOT / "crates/fastctx-types").as_posix())} }}\n'
    manifest += "\n".join(dependencies) + "\n"
    (WORK / "Cargo.toml").write_text(manifest, encoding="utf-8")
    (WORK / "src").mkdir(exist_ok=True)
    source = (ROOT / "scripts/ipc-contract/readers.rs").read_text(encoding="utf-8")
    source = source.replace("COMPAT_PATH", (ROOT / "crates/fastctx-node/src/renewal_legacy.rs").as_posix())
    (WORK / "src/main.rs").write_text(source, encoding="utf-8")
    (WORK / "Cargo.lock").write_bytes((ROOT / "Cargo.lock").read_bytes())
    env = {key: value for key, value in os.environ.items() if not key.upper().startswith("CLAUDE")}
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    subprocess.run(["cargo", "build", "--offline", "-j", "2", "--manifest-path", str(WORK / "Cargo.toml")],
                   cwd=ROOT, env=env, check=True)
    return ROOT / "target/debug" / ("ipc-historical-readers.exe" if os.name == "nt" else "ipc-historical-readers")


def verify(reader):
    sys.path.insert(0, str(ROOT / "scripts/ipc-contract"))
    from samples import samples
    corpus = list(samples(ROOT))
    process = subprocess.run([str(reader)], input="".join(json.dumps(case) + "\n" for case in corpus),
                             encoding="utf-8", capture_output=True)
    if process.returncode:
        completed = len(process.stdout.splitlines())
        print(f"Failed case {completed}: {json.dumps(corpus[completed])}", file=sys.stderr)
        print(process.stderr, file=sys.stderr)
        process.check_returncode()
    answers = [json.loads(line) for line in process.stdout.splitlines()]
    assert len(answers) == len(corpus), (len(answers), len(corpus))
    for case, answer in zip(corpus, answers):
        if case["mode"] == "request":
            assert answer["admitted"] == case["admitted"], (case, answer)
        else:
            wire = answer["wire"]
            if case.get("text_result"):
                assert "The call completed." in wire["reply"]["Err"]["message"], (case, answer)
            else:
                assert "Err" not in wire["reply"] or "Err" in case["value"]["reply"], (case, answer)
            if case.get("encodings"):
                for encoding in case["encodings"]:
                    assert encoding in json.dumps(answer["read"]), (case, answer)
            if case["protocol"] == 7:
                assert wire == answer["current"], (case, answer)
            if case.get("attached") and case["protocol"] >= 6:
                assert wire["reply"]["Ok"]["notices"] == case["value"]["reply"]["Ok"]["notices"]
    print(f"PASS: {len(corpus)} historical request/reply checks against protocols 5, 6 and 7")


if __name__ == "__main__":
    verify(prepare())
