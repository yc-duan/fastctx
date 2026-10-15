"""A release's smoke test on the machine it runs on, through the command line alone: a private hub,
two computers that log in to it and copy files to each other through it, the node's startup service
installed, serving and taken away where asked, and a complete uninstall. Nothing outside a fresh
temporary directory is used, except the startup service the test installs and removes again.

    python scripts/smoke.py PROGRAM [--service]
"""
import argparse
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

RESULTS = []


def check(label, ok, detail=""):
    RESULTS.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + label + ("" if ok or not detail else f"\n     {str(detail)[-1500:]}"), flush=True)
    return ok


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Lab:
    def __init__(self, program, root):
        self.root = root
        self.program = root / ("fastctx.exe" if os.name == "nt" else "fastctx")
        shutil.copy2(program, self.program)
        self.program.chmod(0o755)
        self.owned = []

    def env(self, who):
        values = {key: value for key, value in os.environ.items() if not key.upper().startswith("CLAUDE")}
        values["FASTCTX_HOME"] = str(self.root / who)
        values["CODEX_HOME"] = str(self.root / f"{who}-codex")
        values["CLAUDE_CONFIG_DIR"] = str(self.root / f"{who}-claude")
        values["FASTCTX_UPDATE_RELEASES"] = "http://127.0.0.1:9/releases"
        (self.root / who).mkdir(exist_ok=True)
        return values

    def run(self, who, *args, stdin=None, timeout=180):
        done = subprocess.run([str(self.program), *args], env=self.env(who), input=stdin, capture_output=True,
                              text=True, encoding="utf-8", errors="replace", timeout=timeout)
        return done.returncode, done.stdout + done.stderr

    def spawn(self, who, *args):
        log = open(self.root / f"{who}-{args[0]}.log", "ab", buffering=0)
        process = subprocess.Popen([str(self.program), *args], env=self.env(who), stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=subprocess.STDOUT)
        self.owned.append(process)
        return process


def until(label, probe, limit=90):
    began = time.monotonic()
    last = None
    while time.monotonic() - began < limit:
        try:
            value = probe()
            if value:
                return check(label, True)
        except Exception as error:  # noqa: BLE001 - a probe that throws is tried again
            last = error
        time.sleep(0.5)
    return check(label, False, last)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("program", type=Path)
    parser.add_argument("--service", action="store_true", help="also install and remove the node's startup service")
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="fastctx-smoke-", dir=os.environ.get("RUNNER_TEMP")))
    lab = Lab(args.program.resolve(), root)
    port = free_port()
    password = "Smoke-" + uuid.uuid4().hex[:12] + "-7"
    try:
        code, out = lab.run("hub", "--version")
        check("the program says its version", code == 0 and out.startswith("fastctx "), out)
        code, out = lab.run("hub", "hub", "init", "--name", "smoke", "--admin", "root", "--public", f"127.0.0.1:{port}",
                            "--listen", f"127.0.0.1:{port}", "--dir", str(root / "hub-data"), stdin=password + "\n")
        check("a hub is made", code == 0, out)
        lab.spawn("hub", "hub", "serve", "--dir", str(root / "hub-data"))

        def listening():
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return True

        until("the hub listens", listening, 60)
        for who in ("alpha", "beta"):
            code, out = lab.run(who, "login", f"127.0.0.1:{port}", "root", "--name", who, stdin=password + "\n")
            check(f"{who} logs in", code == 0, out)
        until("alpha sees beta online", lambda: "beta" in lab.run("alpha", "nodes")[1] and lab.run("alpha", "nodes")[0] == 0)

        source = root / "from-alpha.txt"
        source.write_text("smoke " + uuid.uuid4().hex + "\n", encoding="utf-8")
        target = root / "on-beta.txt"
        code, out = lab.run("alpha", "copy", str(source), f"beta:{target}")
        check("alpha copies a file to beta through the hub", code == 0 and target.is_file()
              and target.read_bytes() == source.read_bytes(), out)
        back = root / "back.txt"
        code, out = lab.run("alpha", "copy", f"beta:{target}", str(back))
        check("alpha copies it back from beta", code == 0 and back.is_file() and back.read_bytes() == source.read_bytes(), out)

        if args.service:
            service(lab, root)

        code, out = lab.run("beta", "uninstall", "run", "--everything", "--json", timeout=300)
        check("beta is uninstalled completely", code == 0, out)
        check("nothing of beta's World is left", not (root / "beta" / "bin").exists(), sorted(p.name for p in (root / "beta").glob("*")))
    finally:
        for process in lab.owned:
            process.kill()
        for who in ("alpha", "beta"):
            lab.run(who, "uninstall", "run", "--everything", "--json", timeout=300)
    passed = sum(RESULTS)
    print(f"\n{passed}/{len(RESULTS)} passed")
    return 0 if passed == len(RESULTS) else 1


def service(lab, root):
    """The node's startup service: installed, serving the World, and taken away again."""
    home = str(root / "alpha")
    installed = root / "alpha" / "bin" / lab.program.name
    elevate = ["sudo", "-E"] if sys.platform.startswith("linux") else []
    extra = ["--uid", str(os.getuid())] if sys.platform.startswith("linux") else []
    command = [*elevate, str(installed), "service", "node", "install", "--dir", home, "--executable", str(installed), *extra]
    done = subprocess.run(command, env=lab.env("alpha"), capture_output=True, text=True)
    check("alpha's startup service is installed", done.returncode == 0, done.stdout + done.stderr)
    until("the service's node serves alpha's World", lambda: lab.run("alpha", "nodes")[0] == 0 and "beta" in lab.run("alpha", "nodes")[1], 120)
    code, out = lab.run("alpha", "service", "node", "status", "--dir", home, "--executable", str(installed))
    check("the service says it runs", code == 0 and "running" in out, out)
    command = [*elevate, str(installed), "service", "node", "uninstall", "--dir", home, "--executable", str(installed), *extra]
    done = subprocess.run(command, env=lab.env("alpha"), capture_output=True, text=True)
    check("alpha's startup service is taken away", done.returncode == 0, done.stdout + done.stderr)
    code, out = lab.run("alpha", "service", "node", "status", "--dir", home, "--executable", str(installed))
    check("the service is gone", '"installed": false' in out or "absent" in out, out)


if __name__ == "__main__":
    sys.exit(main())
