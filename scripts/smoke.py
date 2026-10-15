"""A release's smoke test on the machine it runs on, through the command line alone: a computer that
makes a private network keeps its hub as a system service; it and a second computer log in to it and
copy files to each other through it; an agent's `run` there reaches the other computer with `on`;
the node's startup service is installed, serving and taken away where asked; and both computers are
uninstalled completely, the one keeping the hub with it. Nothing outside a fresh temporary directory
is used, except the services the test installs and removes again. When a check fails, the logs of
the nodes, the services and the hub are shown.

    python scripts/smoke.py PROGRAM [--service]
"""
import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

RESULTS = []
# When the previous check was made: a check that took a while says how long, so a slow one shows.
LAST = [time.monotonic()]


def check(label, ok, detail=""):
    RESULTS.append(bool(ok))
    took, LAST[0] = time.monotonic() - LAST[0], time.monotonic()
    print(("PASS " if ok else "FAIL ") + label + (f" ({took:.0f} s)" if took >= 5 else "")
          + ("" if ok or not detail else f"\n     {str(detail)[-1500:]}"), flush=True)
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
        try:
            done = subprocess.run([str(self.program), *args], env=self.env(who), input=stdin, capture_output=True,
                                  text=True, encoding="utf-8", errors="replace", timeout=timeout)
        except subprocess.TimeoutExpired as expired:
            said = [part.decode("utf-8", "replace") if isinstance(part, bytes) else part or ""
                    for part in (expired.stdout, expired.stderr)]
            return None, "".join(said) + f"\n[still running after {timeout} s, so stopped]"
        return done.returncode, done.stdout + done.stderr

    def spawn(self, who, *args):
        log = open(self.root / f"{who}-{args[0]}.log", "ab", buffering=0)
        process = subprocess.Popen([str(self.program), *args], env=self.env(who), stdin=subprocess.DEVNULL,
                                   stdout=log, stderr=subprocess.STDOUT)
        self.owned.append(process)
        return process

    def online(self, who, other):
        """Whether `who` sees `other` online, as `fastctx nodes` lists it; what it listed, when not."""
        code, out = self.run(who, "nodes")
        if code == 0 and any(re.match(rf"{re.escape(other)}\b.*?\s{{2}}online\b", line) for line in out.splitlines()):
            return True
        raise RuntimeError(out)


class Agent:
    """An agent's session, as a host speaks to `fastctx serve` over MCP's stdio."""

    def __init__(self, lab, who):
        self.process = subprocess.Popen([str(lab.program), "serve"], env=lab.env(who), stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=open(lab.root / f"{who}-serve.log", "ab"))
        self.next = 0
        self.ask("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                "clientInfo": {"name": "smoke", "version": "1"}})
        self.tell("notifications/initialized")

    def tell(self, method, params=None):
        message = {"jsonrpc": "2.0", "method": method, **({"params": params} if params is not None else {})}
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()

    def ask(self, method, params):
        self.next += 1
        self.process.stdin.write((json.dumps({"jsonrpc": "2.0", "id": self.next, "method": method,
                                              "params": params}) + "\n").encode())
        self.process.stdin.flush()
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError("fastctx serve closed its output")
            message = json.loads(line)
            if message.get("id") == self.next:
                if "error" in message:
                    raise RuntimeError(json.dumps(message["error"]))
                return message["result"]

    def run(self, command):
        """The text a `run` call answers, and whether it says it failed."""
        result = self.ask("tools/call", {"name": "run", "arguments": {"command": command, "wait": 120}})
        text = "\n".join(part.get("text", "") for part in result.get("content", []) if part.get("type") == "text")
        return text, bool(result.get("isError"))

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.process.kill()


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
    parser.add_argument("--expect-bash", metavar="VERSION", help="the version `run` must find bash at, such as 3.2")
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="fastctx-smoke-", dir=os.environ.get("RUNNER_TEMP")))
    lab = Lab(args.program.resolve(), root)
    port = free_port()
    password = "Smoke-" + uuid.uuid4().hex[:12] + "-7"
    try:
        code, out = lab.run("alpha", "--version")
        check("the program says its version", code == 0 and out.startswith("fastctx "), out)
        # As a computer that makes a new network does: it keeps the hub, run as a system service.
        code, out = lab.run("alpha", "hub", "init", "--name", "smoke", "--admin", "root", "--public", f"127.0.0.1:{port}",
                            "--listen", f"127.0.0.1:{port}", "--dir", str(root / "alpha" / "hub"), "--service",
                            stdin=password + "\n", timeout=300)
        check("alpha makes a hub and runs it as a system service", code == 0, out)

        def listening():
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return True

        until("the hub listens", listening, 60)
        for who in ("alpha", "beta"):
            code, out = lab.run(who, "login", f"127.0.0.1:{port}", "root", "--name", who, stdin=password + "\n")
            check(f"{who} logs in", code == 0, out)
        until("alpha sees beta online", lambda: lab.online("alpha", "beta"))
        until("beta sees alpha online", lambda: lab.online("beta", "alpha"))

        source = root / "from-alpha.txt"
        source.write_text("smoke " + uuid.uuid4().hex + "\n", encoding="utf-8")
        target = root / "on-beta.txt"
        code, out = lab.run("alpha", "copy", str(source), f"beta:{target}")
        check("alpha copies a file to beta through the hub", code == 0 and target.is_file()
              and target.read_bytes() == source.read_bytes(), out)
        back = root / "back.txt"
        code, out = lab.run("alpha", "copy", f"beta:{target}", str(back))
        check("alpha copies it back from beta", code == 0 and back.is_file() and back.read_bytes() == source.read_bytes(), out)

        hands(lab, args.expect_bash)

        if args.service:
            service(lab, root)

        code, out = lab.run("beta", "uninstall", "run", "--everything", "--json", timeout=300)
        check("beta is uninstalled completely", code == 0, out)
        check("nothing of beta's World is left", not (root / "beta" / "bin").exists(), sorted(p.name for p in (root / "beta").glob("*")))
        code, out = lab.run("alpha", "uninstall", "run", "--everything", "--json", timeout=300)
        check("alpha, which keeps the hub, is uninstalled completely", code == 0, out)

        def closed():
            try:
                listening()
            except OSError:
                return True
            return False

        until("the hub is gone with it", closed, 60)
        check("nothing of alpha's World is left", not (root / "alpha" / "bin").exists(), sorted(p.name for p in (root / "alpha").glob("*")))
    finally:
        if not all(RESULTS):
            diagnose(lab, root)
        for process in lab.owned:
            process.kill()
        for who in ("alpha", "beta"):
            lab.run(who, "uninstall", "run", "--everything", "--json", timeout=300)
    passed = sum(RESULTS)
    print(f"\n{passed}/{len(RESULTS)} passed")
    return 0 if passed == len(RESULTS) else 1


def hands(lab, expect_bash):
    """An agent's `run` on alpha, and the part of its script `on` sends to beta with the script's state."""
    marker = uuid.uuid4().hex[:8]
    try:
        agent = Agent(lab, "alpha")
    except Exception as error:  # noqa: BLE001 - reported as the check's failure
        check("an agent's session starts", False, error)
        return
    try:
        script = (f'echo "here $FASTCTX_MACHINE ${{BASH_VERSION%%(*}}"\n'
                  f'carried={marker}\n'
                  "on beta <<'EOF'\n"
                  'echo "there $FASTCTX_MACHINE $carried ${BASH_VERSION%%(*}"\n'
                  "EOF\n")
        text, failed = agent.run(script)
        print(f"     {text.strip().splitlines()[-3:] if text.strip() else text!r}", flush=True)
        check("an agent's run answers on alpha", not failed and re.search(r"^here alpha \d", text, re.M), text)
        check("its on runs on beta with the script's variables", not failed
              and re.search(rf"^there beta {marker} \d", text, re.M), text)
        if expect_bash:
            versions = re.findall(r"^(?:here alpha|there beta \w+) (\S+)", text, re.M)
            check(f"both run in bash {expect_bash}", len(versions) == 2
                  and all(version.startswith(expect_bash) for version in versions), versions)
    except Exception as error:  # noqa: BLE001 - reported as the check's failure
        check("an agent's run answers", False, error)
    finally:
        agent.close()


def node_processes(root, who):
    """The process ids of the nodes serving `who`'s World, by their command lines."""
    listing = subprocess.run(["ps", "-eo", "pid=,args="], capture_output=True, text=True).stdout
    serving = re.compile(rf"\snode\s.*--dir {re.escape(str(root / who))}(\s|$)")
    return {int(line.split(None, 1)[0]) for line in listing.splitlines() if serving.search(line)}


def service_process(status):
    """The process id the service manager says runs the service `status` names."""
    name = status["name"]
    if sys.platform == "darwin":
        printed = subprocess.run(["launchctl", "print", f"gui/{os.getuid()}/{name}"], capture_output=True, text=True).stdout
        found = re.search(r"^\s*pid = (\d+)", printed, re.M)
    else:
        printed = subprocess.run(["systemctl", "show", "-p", "MainPID", "--value", name], capture_output=True, text=True).stdout
        found = re.match(r"\s*(\d+)", printed)
    return int(found.group(1)) if found and int(found.group(1)) > 0 else None


def service(lab, root):
    """The node's startup service: installed, taking the World over from the node that served it, and taken away."""
    home = str(root / "alpha")
    installed = root / "alpha" / "bin" / lab.program.name
    elevate = ["sudo", "-E"] if sys.platform.startswith("linux") else []
    extra = ["--uid", str(os.getuid())] if sys.platform.startswith("linux") else []
    command = [*elevate, str(installed), "service", "node", "install", "--dir", home, "--executable", str(installed), *extra]
    done = subprocess.run(command, env=lab.env("alpha"), capture_output=True, text=True)
    check("alpha's startup service is installed", done.returncode == 0, done.stdout + done.stderr)
    status = json.loads(lab.run("alpha", "service", "node", "status", "--dir", home, "--executable", str(installed))[1])

    def served_by_service():
        pid = service_process(status)
        return pid is not None and node_processes(root, "alpha") == {pid} and lab.online("alpha", "beta")

    until("the service's own node serves alpha's World, and no other", served_by_service, 120)
    code, out = lab.run("alpha", "service", "node", "status", "--dir", home, "--executable", str(installed))
    check("the service says it runs", code == 0 and "running" in out, out)
    command = [*elevate, str(installed), "service", "node", "uninstall", "--dir", home, "--executable", str(installed), *extra]
    done = subprocess.run(command, env=lab.env("alpha"), capture_output=True, text=True)
    check("alpha's startup service is taken away", done.returncode == 0, done.stdout + done.stderr)
    code, out = lab.run("alpha", "service", "node", "status", "--dir", home, "--executable", str(installed))
    check("the service is gone", '"installed": false' in out or "absent" in out, out)


def diagnose(lab, root):
    """What the nodes, services and hub said, for a failed check."""
    def tail(path, lines=60):
        try:
            text = Path(path).read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError as error:
            return [f"({error.strerror})"]
        return text[-lines:]

    print("\n--- diagnostics", flush=True)
    for who in ("alpha", "beta"):
        for log in [root / who / "world" / "node.log", *sorted((root / who).glob("service-*.log")),
                    *sorted((root / who / "hub").glob("*.log"))]:
            print(f"== {log.relative_to(root)}")
            print("\n".join(tail(log)))
        print(f"== {who}: fastctx nodes")
        print(lab.run(who, "nodes", timeout=60)[1])
    for log in sorted(root.glob("*-*.log")):
        print(f"== {log.name}")
        print("\n".join(tail(log)))
    if sys.platform.startswith("linux"):
        units = subprocess.run(["systemctl", "list-units", "--all", "--plain", "--no-legend", "fastctx-service-*"],
                               capture_output=True, text=True).stdout.split()
        for unit in [word for word in units if word.endswith(".service")]:
            print(f"== journal of {unit}")
            print(subprocess.run(["journalctl", "-u", unit, "-n", "60", "--no-pager"], capture_output=True, text=True).stdout)
    # An uninstall's administrator helper works from a directory of its own, with its own log.
    for helper in sorted(Path(tempfile.gettempdir()).glob("fastctx-uninstall-admin-*")):
        for name in ("helper.log", "events.jsonl", "continue.json", "complete.json"):
            if (helper / name).exists():
                print(f"== {helper.name}/{name}")
                print("\n".join(tail(helper / name, 40)))
    if os.name != "nt":
        print("== fastctx processes")
        listing = subprocess.run(["ps", "-eo", "pid=,ppid=,args="], capture_output=True, text=True).stdout
        print("\n".join(line.replace(str(root), "<root>") for line in listing.splitlines()
                        if str(root) in line or "fastctx" in line))


if __name__ == "__main__":
    sys.exit(main())
