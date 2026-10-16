"""The concurrency and lifecycle drill (`specs/world-concurrency.md`, 怎样验证).

One node in a fresh directory, its Codex login pointed at a model mock (`model_mock.py`), so no
request goes to any model vendor. For a few minutes several conversations at once are spoken to,
run commands through the World's `run`, are deleted while they work and opened again, while the node
is ended at random moments and started again. Then the node is left to settle, and the drill checks:

- the node answered a plain call within a few seconds whenever it was up;
- its log has no lock taken out of order, nothing stuck, no tasks waiting on each other in a circle;
- every conversation settles: none is left working, no words are left on their way in;
- a deleted conversation leaves nothing behind: no record, no directory, no Codex thread;
- every node that stopped halfway through something finished it once it was up again.

    python scripts/concurrency.py PROGRAM [--codex CODEX] [--minutes N] [--restarts] [--seed N]

PROGRAM is a fastctx build; CODEX the `codex` program to run (found on PATH without it). Nothing
outside a fresh directory is used, and no `claude` is on the PATH the World sees.
"""
import argparse
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from model_mock import Mock  # noqa: E402

RESULTS = []
PRINTING = threading.Lock()
# How long a plain call may take, outside the moments the node is being ended and started.
ANSWERS_WITHIN = 5.0
# What the node's log must never say.
BAD_LINES = re.compile(r"lock order:|stuck for \d+ s|in a circle|panicked at", re.I)


def say(line):
    with PRINTING:
        print(line, flush=True)


def check(label, ok, detail=""):
    RESULTS.append(bool(ok))
    say(("PASS " if ok else "FAIL ") + label + ("" if ok or not detail else f"\n     {str(detail)[-2000:]}"))
    return ok


class Lab:
    """A World of its own: its directory, its Codex login and the mock it asks."""

    def __init__(self, program, codex, root, mock):
        self.root = root
        self.program = root / ("fastctx.exe" if os.name == "nt" else "fastctx")
        shutil.copy2(program, self.program)
        self.program.chmod(0o755)
        self.home = root / "world"
        self.codex_home = root / "codex"
        self.home.mkdir()
        self.codex_home.mkdir()
        (self.codex_home / "config.toml").write_text(
            f'model = "mock-model"\nmodel_provider = "mock"\n\n[model_providers.mock]\nname = "mock"\n'
            f'base_url = "{mock.base_url}"\nwire_api = "responses"\nrequires_openai_auth = false\n'
            f'request_max_retries = 0\nstream_max_retries = 0\nstream_idle_timeout_ms = 15000\n',
            encoding="utf-8")
        self.codex_dir = Path(codex).parent
        self.node = None
        self.restarting = threading.Event()
        self.restarter = None
        self.lock = threading.Lock()

    def env(self):
        values = {key: value for key, value in os.environ.items() if not key.upper().startswith("CLAUDE")}
        values.update(FASTCTX_HOME=str(self.home), CODEX_HOME=str(self.codex_home),
                      FASTCTX_UPDATE_RELEASES="http://127.0.0.1:9/releases", PATH=self.path())
        return values

    def path(self):
        keep = [str(self.codex_dir)]
        for entry in os.environ.get("PATH", "").split(os.pathsep):
            if not entry or entry in keep:
                continue
            if any((Path(entry) / name).exists() for name in ("claude", "claude.exe", "claude.cmd")):
                continue
            keep.append(entry)
        return os.pathsep.join(keep)

    def run(self, *args, timeout=120):
        """What a fastctx command printed, and its exit code; a command that ran past `timeout` is -1.
        None starts while the node is being ended and started again: a command that found no node would
        start one of its own, with the user's environment and not this lab's."""
        while self.restarting.is_set() and threading.current_thread() is not self.restarter:
            time.sleep(0.2)
        try:
            done = subprocess.run([str(self.program), *args], env=self.env(), capture_output=True,
                                  stdin=subprocess.DEVNULL, timeout=timeout)
        except subprocess.TimeoutExpired as late:
            return -1, (late.stdout or b"").decode("utf-8", "replace") + (late.stderr or b"").decode("utf-8", "replace")
        return done.returncode, done.stdout.decode("utf-8", "replace") + done.stderr.decode("utf-8", "replace")

    def start_node(self):
        log = open(self.root / "node.out", "ab")
        self.node = subprocess.Popen([str(self.program), "node"], env=self.env(), stdout=log,
                                     stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
        until("the node answers", lambda: self.run("w", timeout=20)[0] == 0, limit=60)

    def node_pids(self):
        """Every node of this lab: the one started here, or one a command started from the World's own
        copy of the program."""
        return [pid for pid, command in self.processes() if re.search(r"\snode(\s|$)", command)]

    def node_commands(self):
        """Every node of this lab, with its command line."""
        return [(pid, command) for pid, command in self.processes() if re.search(r"\snode(\s|$)", command)]

    def processes(self):
        """Every process whose command line names this lab's directory: its nodes, the app-servers and
        commands they started."""
        root = str(self.root).lower()
        found = []
        if os.name == "nt":
            listed = subprocess.run(["powershell", "-NoProfile", "-Command",
                                     "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine } | "
                                     "ForEach-Object { \"$($_.ProcessId)`t$($_.CommandLine)\" }"],
                                    capture_output=True, text=True, timeout=60, encoding="utf-8", errors="replace")
            for line in listed.stdout.splitlines():
                pid, _, command = line.partition("\t")
                if root in command.lower() and pid.strip().isdigit():
                    found.append((int(pid), command))
        else:
            for entry in Path("/proc").iterdir():
                if not entry.name.isdigit():
                    continue
                try:
                    command = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
                except OSError:
                    continue
                if root in command.lower():
                    found.append((int(entry.name), command))
        return found

    def end_all(self):
        """Ends everything of this lab still running."""
        for pid, _ in self.processes():
            self.kill(pid)

    @staticmethod
    def kill(pid):
        if os.name == "nt":
            subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)
        else:
            try:
                os.kill(pid, 9)
            except ProcessLookupError:
                pass

    def end_node(self):
        """Ends the node at once, as a power cut or a crash would."""
        for pid in self.node_pids():
            self.kill(pid)
        if self.node:
            try:
                self.node.wait(timeout=30)
            except subprocess.TimeoutExpired:
                pass

    def restart(self):
        """Ends the node and starts this lab's again at once. A command still running when the node went
        may have started one of its own before this one was up, with the user's environment: it is ended
        too, until the node is this lab's."""
        with self.lock:
            self.restarter = threading.current_thread()
            self.restarting.set()
            try:
                for _ in range(5):
                    self.end_node()
                    self.start_node()
                    if not self.foreign_nodes():
                        break
                else:
                    raise RuntimeError("a node started by a command kept taking this lab's World")
            finally:
                time.sleep(2)
                self.restarting.clear()

    def foreign_nodes(self):
        """Nodes of this lab's program a command started (`fastctx node --dir`), not this lab."""
        return [pid for pid, command in self.node_commands() if "--dir" in command]

    def conversations(self):
        return self.home / "world" / "conversations"

    def node_log(self):
        path = self.home / "world" / "node.log"
        return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""


def until(label, probe, limit=90, every=0.5):
    deadline = time.monotonic() + limit
    while time.monotonic() < deadline:
        try:
            if probe():
                return True
        except Exception:  # noqa: BLE001 - a probe that fails is a probe not yet true
            pass
        time.sleep(every)
    raise RuntimeError(f"{label} did not happen within {limit} s")


class Drill:
    def __init__(self, lab, rng):
        self.lab = lab
        self.rng = rng
        self.talks = []
        self.deleted = []
        # The Codex thread of each conversation deleted, which goes with it.
        self.threads = {}
        self.slow_calls = []
        self.outcomes = []
        # Conversations a deletion could not delete within its two minutes, and why it last heard.
        self.undeleted = []
        self.lock = threading.Lock()
        self.stop = threading.Event()

    def note(self, what):
        with self.lock:
            self.outcomes.append(what)

    def open(self):
        code, out = self.lab.run("talk", "--open", "codex", "hello", timeout=240)
        found = re.search(r"=== opened (\S+) on ", out)
        if code != 0 or not found:
            self.note(("open failed", code, out[-400:]))
            return None
        with self.lock:
            self.talks.append(found.group(1))
        return found.group(1)

    def speak(self, talk, words, timeout=240):
        started = time.monotonic()
        code, out = self.lab.run("talk", talk, words, timeout=timeout)
        took = time.monotonic() - started
        self.note(("said", talk, words[:30], code, round(took, 1), out.strip()[-160:] if code else ""))
        return code, out

    def delete(self, talk):
        # Its Codex thread, from its record, so a deletion is checked to take that too.
        thread = thread_of(self.lab, talk)
        code, out = self.lab.run("data", "delete", "--yes", talk, timeout=180)
        gone = code == 0 and "not deleted" not in out.lower() and "refused" not in out.lower()
        self.note(("delete", talk, code, out.strip()[-200:]))
        if gone:
            with self.lock:
                if talk in self.talks:
                    self.talks.remove(talk)
                self.deleted.append(talk)
                if thread:
                    self.threads[talk] = thread
        return gone

    def delete_working(self, talk):
        """Deletes a conversation that may be working, stopping what it runs, within two minutes."""
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            if self.delete(talk):
                return True
            with self.lock:
                said = self.outcomes[-1][3] if self.outcomes and self.outcomes[-1][0] == "delete" else ""
            for job in set(re.findall(r"\bj-[0-9a-z]{6}\b", said)):
                code, out = self.lab.run("stop", job, timeout=60)
                self.note(("stop", job, code, out.strip()[-120:]))
            time.sleep(self.rng.uniform(0.2, 1.5))
        with self.lock:
            last = self.outcomes[-1] if self.outcomes else None
            self.undeleted.append((talk, self.state_of(talk), last))
        return False

    def state_of(self, talk):
        """What the node keeps of a conversation's state now, for a failure to show."""
        now = self.lab.conversations() / talk / "now"
        try:
            return now.read_text(encoding="utf-8")[:300]
        except OSError as error:
            return str(error)

    def worker(self, index):
        talk = self.open()
        while not self.stop.is_set():
            if talk is None:
                talk = self.open()
                if talk is None:
                    time.sleep(2)
                    continue
            roll = self.rng.random()
            if roll < 0.35:
                self.speak(talk, "hello again")
            elif roll < 0.6:
                self.speak(talk, f"[[run: echo w{index}; sleep {self.rng.choice(['0.2', '1', '2'])}]]")
            elif roll < 0.7:
                self.speak(talk, "[[run: sleep 25]]", timeout=300)
            elif roll < 0.78:
                self.speak(talk, f"[[slow: {self.rng.choice(['1', '3'])}]]")
            elif roll < 0.83:
                self.speak(talk, "[[hang]]", timeout=120)
            else:
                # Deleted while it works: refused while a turn or a command
                # of it runs, as a user would, its command is stopped and the
                # deletion asked again until it goes.
                words = self.rng.choice(["[[run: sleep 8]]", "[[slow: 3]]", "hello"])
                speaking = threading.Thread(target=self.speak, args=(talk, words), daemon=True)
                speaking.start()
                time.sleep(self.rng.uniform(0.2, 4))
                if self.delete_working(talk):
                    talk = None
                speaking.join(timeout=300)
            time.sleep(self.rng.uniform(0.1, 1.5))

    def probe(self):
        while not self.stop.is_set():
            if self.lab.restarting.is_set():
                time.sleep(0.5)
                continue
            started = time.monotonic()
            code, _ = self.lab.run("w", timeout=60)
            took = time.monotonic() - started
            if not self.lab.restarting.is_set() and (code != 0 or took > ANSWERS_WITHIN):
                with self.lock:
                    self.slow_calls.append((round(took, 1), code))
            time.sleep(2)

    def chaos(self):
        while not self.stop.wait(self.rng.uniform(40, 80)):
            say("  ending the node and starting it again")
            self.lab.restart()


def settled(lab, talks):
    """Whether every conversation kept here is idle with no words on their way in."""
    for talk in talks:
        directory = lab.conversations() / talk
        now = directory / "now"
        if now.exists():
            state = json.loads(now.read_text(encoding="utf-8"))
            if state.get("activity") == "busy" or state.get("turn"):
                return False
        inbox = directory / "inbox"
        if inbox.exists() and any(path.suffix == ".json" for path in inbox.iterdir()):
            return False
    return True


def thread_of(lab, talk):
    """The Codex thread a conversation's record names, if it can be read."""
    try:
        return json.loads((lab.conversations() / talk / "about").read_text(encoding="utf-8")).get("session")
    except (OSError, ValueError):
        return None


def left_of(lab, talk, thread):
    """What is still here of a deleted conversation: its directory, its listing, its Codex thread."""
    left = []
    if (lab.conversations() / talk).exists():
        left.append("its directory")
    code, out = lab.run("data", "list", "--json", timeout=120)
    if code == 0 and f'"{talk}"' in out:
        left.append("its listing")
    if thread and any((lab.codex_home / "sessions").rglob(f"*{thread}*")):
        left.append(f"its Codex thread {thread}")
    return left


def halfway(lab):
    """The conversations whose deletion was begun and is not done: their intent is still kept."""
    try:
        return sorted(directory.name for directory in lab.conversations().iterdir()
                      if (directory / "deleting").exists() or (directory / "host-deleted").exists())
    except OSError:
        return []


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("program")
    parser.add_argument("--codex", default=shutil.which("codex"))
    parser.add_argument("--minutes", type=float, default=4)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--restarts", action="store_true")
    parser.add_argument("--seed", type=int, default=int(time.time()))
    parser.add_argument("--root", help="where the lab's directory goes; the system's temporary directory without it")
    args = parser.parse_args()
    if not args.codex:
        sys.exit("no codex program: give --codex")
    say(f"seed {args.seed}")
    rng = random.Random(args.seed)
    root = Path(tempfile.mkdtemp(prefix="fastctx-concurrency-", dir=args.root))
    mock = Mock()
    lab = Lab(args.program, args.codex, root, mock)
    try:
        lab.start_node()
        code, out = lab.run("apply", "--target", "codex", "--home", str(lab.codex_home), timeout=180)
        if not check("the Codex login gets the World's tools", code == 0, out):
            return 1
        drill = Drill(lab, rng)
        threads = [threading.Thread(target=drill.worker, args=(index,), daemon=True) for index in range(args.workers)]
        threads.append(threading.Thread(target=drill.probe, daemon=True))
        if args.restarts:
            threads.append(threading.Thread(target=drill.chaos, daemon=True))
        for thread in threads:
            thread.start()
        time.sleep(args.minutes * 60)
        drill.stop.set()
        for thread in threads:
            thread.join(timeout=360)
        check("every worker came back", all(not thread.is_alive() for thread in threads))
        say(f"  {len(drill.outcomes)} things done, {len(drill.deleted)} conversations deleted, {len(mock.requests)} model requests")
        opened = sum(1 for outcome in drill.outcomes if outcome[0] == "said")
        failures = [outcome for outcome in drill.outcomes if outcome[0] == "open failed"]
        check("conversations opened and were spoken to", opened > 0 and len(mock.requests) > 0, failures[:3])
        check("every deletion went through within two minutes of stopping what its conversation ran",
              not drill.undeleted, drill.undeleted[:5])
        unsaid = [outcome for outcome in drill.outcomes if outcome[0] == "said" and outcome[3] != 0]
        reasons = {}
        for outcome in unsaid:
            reason = re.sub(r"[0-9a-f]{4,}", "…", outcome[5].splitlines()[-1] if outcome[5] else "")
            reasons[reason] = reasons.get(reason, 0) + 1
        said = sum(1 for outcome in drill.outcomes if outcome[0] == "said")
        say(f"  {said} words said, {len(unsaid)} not: " + "; ".join(f"{count}x {reason}" for reason, count in sorted(reasons.items(), key=lambda item: -item[1])[:6]))
        deletes = [outcome for outcome in drill.outcomes if outcome[0] == "delete"]
        say(f"  {len(deletes)} deletions tried, {len(drill.deleted)} done")
        for outcome in deletes[:6]:
            say(f"    {outcome}")
        check("the node answered a plain call within a few seconds whenever it was up", not drill.slow_calls,
              drill.slow_calls[:20])
        try:
            until("every conversation settles", lambda: settled(lab, list(drill.talks)), limit=180, every=2)
            check("every conversation settled: none working, no words on their way in", True)
        except RuntimeError as error:
            check("every conversation settled: none working, no words on their way in", False, error)
        for talk in list(drill.deleted):
            left = left_of(lab, talk, drill.threads.get(talk))
            check(f"{talk}, deleted, left nothing behind", not left, left)
        begun = halfway(lab)
        check("no deletion is left halfway", not begun, begun[:10])
        bad = [line for line in lab.node_log().splitlines() if BAD_LINES.search(line)]
        check("the node's log has no lock out of order, nothing stuck, no circle", not bad, "\n".join(bad[:30]))
        unfinished = [line for line in lab.node_log().splitlines() if "finishing the deletion of" in line]
        check("deletions a stopped node began were finished", not unfinished, "\n".join(unfinished[:10]))
    finally:
        lab.end_all()
        mock.close()
        failed = not all(RESULTS)
        if failed:
            say("--- node.log (last 80 lines) ---")
            say("\n".join(lab.node_log().splitlines()[-80:]))
        say(f"lab: {root}")
    return 0 if all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
