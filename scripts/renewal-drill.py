"""The boot service taking a newer release by itself, on Windows (`world-upgrade.md`, 换版不要人确认).

A computer keeps its hub and its node as system services of this release. The same source, built as
a later release and signed as one with the official key, is handed to the renewal by the path a
node's own update takes, as a node of an earlier release left it: its release's package, the
program and its own signature side by side, kept in the World's updates, and the program staged
with another signature of that release beside it, which no guardian takes it by. The renewal mends
the signature from the package; the node takes the program, each protected guardian takes it into
its own directory by that signature with no administrator asked, the hub follows its machine, and
both services run it. Nothing outside a fresh temporary directory is used,
except the services the drill installs and removes again. When a check fails, the renewal's state,
what each guardian said and the logs are shown.

    python scripts/renewal-drill.py bump VERSION
    python scripts/renewal-drill.py run PROGRAM LATER VERSION

`bump` prints a version later than VERSION and sets it as the workspace's, for building the later
release; `run` drills with this release's PROGRAM and the LATER one of VERSION, its signature beside
it, and beside that `decoy.sig`, the same release's signature of something else.
"""
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import uuid
import zipfile
from pathlib import Path

from smoke import RESULTS, Lab, check, diagnose, free_port, until

REPO = Path(__file__).resolve().parent.parent


def later_than(version):
    """A version after `version`: a test release gains a part, a stable one its next patch's test."""
    if "-" in version:
        return version + ".renewal"
    major, minor, patch = (int(part) for part in version.split("+")[0].split("."))
    return f"{major}.{minor}.{patch + 1}-renewal"


def bump(version):
    later = later_than(version)
    manifest = REPO / "Cargo.toml"
    text = manifest.read_bytes().decode("utf-8")
    head = text.index("[workspace.package]")
    old = f'version = "{version}"'
    at = text.index(old, head)
    manifest.write_bytes((text[:at] + f'version = "{later}"' + text[at + len(old):]).encode("utf-8"))
    # The workspace's own packages are the ones with no source in the lock,
    # which keeps the newlines its checkout gave it.
    lock = REPO / "Cargo.lock"
    text = lock.read_bytes().decode("utf-8")
    newline = "\r\n" if "\r\n" in text else "\n"
    blocks = text.split(f"{newline}[[package]]{newline}")
    changed = 0
    for index, block in enumerate(blocks):
        if index and f"{newline}source = " not in block and f'{newline}version = "{version}"' in block:
            blocks[index] = block.replace(f'{newline}version = "{version}"', f'{newline}version = "{later}"', 1)
            changed += 1
    if not changed:
        raise SystemExit(f"no package of the workspace is at {version} in Cargo.lock")
    lock.write_bytes(f"{newline}[[package]]{newline}".join(blocks).encode("utf-8"))
    # The notices are bound to the lockfile they were made from; the later
    # release, never published, carries the same notices bound to its own.
    notices = REPO / "dist" / "THIRD_PARTY_LICENSES.md"
    if notices.is_file():
        from license_text import input_marker
        text = notices.read_bytes().decode("utf-8")
        refreshed = re.sub(r"<!-- fastctx-notices-inputs: [0-9a-f]+ -->", input_marker(REPO), text, count=1)
        if refreshed == text:
            raise SystemExit("the notices carry no input marker to refresh")
        notices.write_bytes(refreshed.encode("utf-8"))
    print(later)


def sha256(path):
    try:
        return hashlib.sha256(Path(path).read_bytes()).hexdigest()
    except OSError:
        return None


def protected(name):
    return Path(os.environ["ProgramData"]) / "fastctx-services" / name


def run(program, later, version):
    root = Path(tempfile.mkdtemp(prefix="fastctx-renewal-", dir=os.environ.get("RUNNER_TEMP")))
    lab = Lab(Path(program).resolve(), root)
    port = free_port()
    password = "Renewal-" + uuid.uuid4().hex[:12] + "-7"
    home = root / "alpha"
    installed = home / "bin" / "fastctx.exe"
    names = {}
    phases = []

    def status(kind, directory):
        code, out = lab.run("alpha", "service", kind, "status", "--dir", str(directory), timeout=60)
        try:
            return json.loads(out[out.index("{"):])
        except ValueError:
            return {}

    def renewal():
        code, out = lab.run("alpha", "renew", "status", "--dir", str(home), timeout=60)
        try:
            return json.loads(out[out.index("{"):])
        except ValueError:
            return {}

    def listening():
        with socket.create_connection(("127.0.0.1", port), timeout=1):
            return True

    try:
        code, out = lab.run("alpha", "hub", "init", "--name", "renewal", "--admin", "root", "--public", f"127.0.0.1:{port}",
                            "--listen", f"127.0.0.1:{port}", "--dir", str(home / "hub"), "--service",
                            stdin=password + "\n", timeout=300)
        check("alpha keeps a hub as a system service", code == 0, out)
        until("the hub listens", listening, 60)
        code, out = lab.run("alpha", "login", f"127.0.0.1:{port}", "root", "--name", "alpha", stdin=password + "\n", timeout=300)
        check("alpha logs in", code == 0, out)
        if not status("node", home).get("installed"):
            done = subprocess.run([str(installed), "service", "node", "install", "--dir", str(home), "--executable", str(installed)],
                                  env=lab.env("alpha"), capture_output=True, text=True)
            check("alpha's node is kept by a system service", done.returncode == 0, done.stdout + done.stderr)
        names["node"] = status("node", home).get("name")
        names["hub"] = status("hub", home / "hub").get("name")
        check("both services are named", all(names.values()), names)

        def served():
            node = renewal().get("node") or {}
            return node.get("supervised") and not node.get("bridge") and not node.get("yielding")

        until("the node service's own node serves alpha", served, 120)
        before = {kind: sha256(protected(name) / "guardian.exe") for kind, name in names.items()}
        check("both guardians run this release", all(digest == sha256(installed) for digest in before.values()), before)

        # The release's package, kept where a node keeps what it brought; the program staged with
        # the wrong signature beside it, as a node of an earlier release unpacked it.
        kept = home / "updates" / "packages" / f"v{version}" / "fastctx-x86_64-pc-windows-msvc"
        kept.parent.mkdir(parents=True)
        with zipfile.ZipFile(kept, "w", zipfile.ZIP_DEFLATED) as zipped:
            zipped.write(later, "fastctx.exe")
            zipped.write(f"{later}.sig", "fastctx.exe.sig")
        staged = root / "later" / "fastctx-x86_64-pc-windows-msvc.zip"
        staged.parent.mkdir()
        with zipfile.ZipFile(staged, "w", zipfile.ZIP_DEFLATED) as zipped:
            zipped.write(later, "fastctx.exe")
            zipped.write(Path(later).with_name("decoy.sig"), "fastctx.exe.sig")
        code, out = lab.run("alpha", "renew", "stage", "--dir", str(home), "--package", str(staged), timeout=300)
        check("the later release is handed to the renewal, with another signature beside it", code == 0, out)

        def ended():
            phase = renewal().get("phase")
            if phase and (not phases or phases[-1] != phase):
                phases.append(phase)
            return phase in ("complete", "failed", "awaiting_administrator")

        until("the renewal ends", ended, 300)
        check("it completes", phases[-1:] == ["complete"], phases)
        check("no administrator was asked", "awaiting_administrator" not in phases, phases)
        printed = subprocess.run([str(installed), "--version"], capture_output=True, text=True).stdout.strip()
        check("the stable World program is the later release", printed == f"fastctx {version}", printed)
        digest = sha256(Path(later))
        until("the node's guardian took it into its own directory",
              lambda: sha256(protected(names["node"]) / "guardian.exe") == digest, 60)
        until("the hub's guardian and its program took it too",
              lambda: sha256(protected(names["hub"]) / "guardian.exe") == digest
              and sha256(protected(names["hub"]) / "payload.exe") == digest, 120)
        for kind, name in names.items():
            adopted = protected(name) / "adopted.json"

            def said():
                return json.loads(adopted.read_text(encoding="utf-8")) if adopted.is_file() else {}

            def took():
                now = said()
                return not now.get("why") and now.get("digest") == digest

            # The guardian says so just after it put the program in place.
            if not until(f"the {kind}'s guardian says it took it", took, 30):
                print(f"     {said()}", flush=True)

        def serving_later():
            node = renewal().get("node") or {}
            return (node.get("build") or {}).get("version") == version and node.get("supervised") and not node.get("bridge")

        until("the node service's node runs the later release", serving_later, 120)
        # Started again by the renewal at once, not by the service's check
        # at the next minute.
        until("the hub listens again within half a minute", listening, 30)
        for kind in names:
            state = status(kind, home if kind == "node" else home / "hub")
            check(f"the {kind} service runs", state.get("state", "").lower() == "running", state)
        check("the renewal kept no error", not renewal().get("error"), renewal())
    finally:
        if not all(RESULTS):
            print("\n--- the renewal", flush=True)
            print(json.dumps(renewal(), indent=1)[-4000:])
            print("phases:", phases)
            for kind, name in names.items():
                folder = protected(name) if name else None
                if folder and folder.is_dir():
                    print(f"== {kind} guardian at {folder}")
                    for entry in sorted(folder.iterdir()):
                        print(f"   {entry.name} {sha256(entry) if entry.is_file() else ''}")
                    for log in ("adopted.json", "transaction.json"):
                        if (folder / log).is_file():
                            print(f"   {log}: {(folder / log).read_text(encoding='utf-8', errors='replace')[-1500:]}")
            for log in sorted(home.glob("**/*.log")):
                if "renewal" in str(log) or "service" in log.name:
                    print(f"== {log.relative_to(root)}")
                    print("\n".join(log.read_text(encoding="utf-8", errors="replace").splitlines()[-60:]))
            diagnose(lab, root)
        for process in lab.owned:
            process.kill()
        cleaner = installed if installed.is_file() else lab.program
        subprocess.run([str(cleaner), "uninstall", "run", "--everything", "--json"], env=lab.env("alpha"),
                       capture_output=True, text=True, timeout=300)
    passed = sum(RESULTS)
    print(f"\n{passed}/{len(RESULTS)} passed")
    return 0 if passed == len(RESULTS) else 1


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "bump":
        bump(sys.argv[2])
        return 0
    if len(sys.argv) == 5 and sys.argv[1] == "run":
        return run(sys.argv[2], sys.argv[3], sys.argv[4])
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
