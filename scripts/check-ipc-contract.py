"""Decode today's compatible replies with the actual historical Rust readers.

Requires git history for the pinned revisions: the development history, which
the public repository does not carry, so this runs in a clone that has it. Sources
are materialized only under target; no historical production program is run.
A protocol that ships adds its revision to REVISIONS, and a reply shape that
changes adds a case to scripts/ipc-contract/samples.py.
"""

import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
WORK = TARGET / "ipc-contract"
REVISIONS = {
    5: "5534ed866e19c931f469c51e0f434ac2e3b4ea2e",
    6: "f00f76c68d27d74204110c876860e4ab61a7f85f",
    7: "bd905f697fbf793f9f755561f36b0758fb08e383",
    8: "52d3e985cace21aab0ec8b01f824f45fc187d60f",
    9: "287bacbb4e59cf755eb0d2fdf4840705e1ebff61",
    10: "11b480d9efbaf949b3dad14e0fc9a80b1cbb22d2",
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
        # Compile each decoder beside its unchanged historical types. Putting
        # all serde monomorphizations in the driver scales its compiler memory
        # with the number of protocol revisions instead of one revision.
        with (directory / "src/lib.rs").open("a", encoding="utf-8", newline="\n") as source:
            source.write('''
pub fn contract_frame(value: serde_json::Value, request: bool) -> serde_json::Value {
    if request {
        serde_json::to_value(serde_json::from_value::<ipc::ClientFrame>(value).unwrap()).unwrap()
    } else {
        serde_json::to_value(serde_json::from_value::<ipc::NodeFrame>(value).unwrap()).unwrap()
    }
}
pub fn contract_welcome(value: serde_json::Value) -> serde_json::Value {
    serde_json::to_value(serde_json::from_value::<ipc::Welcome>(value).unwrap()).unwrap()
}
''')
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
    env["CARGO_TARGET_DIR"] = str(TARGET)
    # Each historical reader derives serde for the whole wire contract. Build
    # them one at a time rather than keeping four copies of that compiler work
    # in memory; ordinary dependency compilation still has four cargo jobs.
    build = ["cargo", "build", "--offline", "-j", "4", "--manifest-path", str(WORK / "Cargo.toml")]
    for protocol in REVISIONS:
        subprocess.run([*build, "-p", f"ipc-reader-v{protocol}"], cwd=ROOT, env=env, check=True)
    subprocess.run(build, cwd=ROOT, env=env, check=True)
    return TARGET / "debug" / ("ipc-historical-readers.exe" if os.name == "nt" else "ipc-historical-readers")


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
        if case["mode"] in ("request", "current_request"):
            assert answer["admitted"] == case["admitted"], (case, answer)
        elif case["mode"] == "welcome":
            # The node's lifetime is read from protocol 8 on; older readers drop it.
            expected = {key: value for key, value in case["value"].items() if key != "node_lifetime" or case["protocol"] >= 8}
            assert answer["read"] == expected, (case, answer)
        else:
            wire = answer["wire"]
            if case.get("text_result"):
                assert "The call completed." in wire["reply"]["Err"]["message"], (case, answer)
            else:
                assert "Err" not in wire["reply"] or "Err" in case["value"]["reply"], (case, answer)
            if case.get("encodings"):
                for encoding in case["encodings"]:
                    assert encoding in json.dumps(answer["read"]), (case, answer)
            # Protocols 7 and 8 read today's replies as they are, but for those said as text.
            if case.get("world"):
                told = wire["reply"]["Ok"]["notices"][0]["notice"]
                assert told == {"kind": "words", "from": {"kind": "world"}, "text": "the World's words"}, (case, answer)
            elif case["protocol"] >= 7 and not case.get("text_result"):
                expected = json.loads(json.dumps(answer["current"]))
                if case.get("receipts"):
                    expected["reply"]["Ok"].pop("consumed")
                if case.get("collaboration"):
                    expected["reply"]["Ok"].pop("collab")
                if case.get("turn_facts"):
                    expected["reply"]["Ok"]["notices"][0]["notice"]["end"].pop("facts")
                assert wire == expected, (case, answer)
            if case.get("turn_facts"):
                assert "facts" not in wire["reply"]["Ok"]["notices"][0]["notice"]["end"], (case, answer)
            if case.get("attached") and case["protocol"] >= 6:
                assert wire["reply"]["Ok"]["notices"] == case["value"]["reply"]["Ok"]["notices"]
    print(f"PASS: {len(corpus)} historical request/reply checks against protocols {', '.join(map(str, REVISIONS))}")


if __name__ == "__main__":
    verify(prepare())
