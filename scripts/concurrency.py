"""The concurrency and lifecycle drill (`specs/world-concurrency.md`, 怎样验证).

One node in a fresh directory, its Codex login pointed at a model mock (`model_mock.py`), so no
request goes to any model vendor. For a few minutes several conversations at once are spoken to,
run commands through the World's `run`, are deleted while they work and opened again, while the node
is ended at random moments and started again. Then the node is left to settle, and the drill checks:

- sampled plain calls answered within a few seconds outside restarts;
- commands left running end, and their ends go into their conversations as notices;
- its log has no lock taken out of order, nothing stuck, no tasks waiting on each other in a circle;
- every conversation settles: none is left working, no words are left on their way in;
- each confirmed deletion leaves no directory, data listing or Codex rollout file;
- no deletion intent remains after the node has had time to recover.

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
import stat
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from model_mock import Mock  # noqa: E402
from drill_processes import WindowsScope  # noqa: E402

RESULTS = []
PRINTING = threading.Lock()
# How long a plain call may take, outside the moments the node is being ended and started.
ANSWERS_WITHIN = 5.0
# Background deletion recovery retries every 300 s (fastctx-node/src/recovery.rs); allow its next attempt to finish.
RECOVERY_OBSERVATION = 300 + 180
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
        self.scope = WindowsScope() if os.name == "nt" else None
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
        self.ending = threading.Event()
        self.restarter = None
        self.generation = 0
        self.lock = threading.Lock()

    def env(self):
        values = {key: value for key, value in os.environ.items() if not key.upper().startswith("CLAUDE")}
        values.update(FASTCTX_HOME=str(self.home), CODEX_HOME=str(self.codex_home), FASTCTX_DRILL_OWNER=str(self.root),
                      FASTCTX_UPDATE_RELEASES="http://127.0.0.1:9/releases", PATH=self.path())
        return values

    def on(self, command):
        """`command` as a script may be: one `on` naming this node's machine."""
        name = json.loads((self.home / "world" / "machine.json").read_text(encoding="utf-8"))["name"]
        return f"on {name} <<'RUN'\n{command}\nRUN"

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
        if self.ending.is_set():
            raise RuntimeError("the drill is ending; no new command can start")
        deadline = time.monotonic() + timeout
        while self.restarting.is_set() and threading.current_thread() is not self.restarter:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                check("a command did not time out waiting for restart", False, args)
                return -1, "timed out waiting for the node to restart"
            time.sleep(min(0.2, remaining))
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            check("a command began before its deadline", False, args)
            return -1, "command deadline expired"
        try:
            done = subprocess.run([str(self.program), *args], env=self.env(), capture_output=True,
                                  stdin=subprocess.DEVNULL, timeout=remaining)
        except subprocess.TimeoutExpired as late:
            check("a command returned within its deadline", False, (args, timeout))
            return -1, (late.stdout or b"").decode("utf-8", "replace") + (late.stderr or b"").decode("utf-8", "replace")
        return done.returncode, done.stdout.decode("utf-8", "replace") + done.stderr.decode("utf-8", "replace")

    def start_node(self):
        """Starts this lab's node; whether it is the one that took the World. No command asks it
        anything before some node listens, as a command that found none would start one of its own,
        with the user's environment and not this lab's."""
        if self.ending.is_set():
            raise RuntimeError("the drill is ending; no new node can start")
        listened = self.listened()
        with open(self.root / "node.out", "ab") as log:
            self.node = subprocess.Popen([str(self.program), "node"], env=self.env(), stdout=log,
                                         stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
        until("a node listens", lambda: self.listened() > listened or self.node.poll() is not None, limit=60)
        if self.node.poll() is not None:
            return False
        until("the node answers", lambda: self.run("w", timeout=20)[0] == 0, limit=60)
        return self.node.poll() is None

    def listened(self):
        """How many times a node of this lab began to listen."""
        try:
            return self.node_log().count("listening at ")
        except FileNotFoundError:
            return 0

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
                                     "$ErrorActionPreference = 'Stop'; "
                                     "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine } | "
                                     "ForEach-Object { \"$($_.ProcessId)`t$($_.CommandLine)\" }"],
                                    capture_output=True, text=True, timeout=60, encoding="utf-8", errors="replace")
            if listed.returncode != 0:
                raise RuntimeError(f"listing this lab's processes failed ({listed.returncode}): "
                                   f"{listed.stdout[-1000:]}{listed.stderr[-1000:]}")
            owned = self.scope.pids()
            for line in listed.stdout.splitlines():
                pid, _, command = line.partition("\t")
                if pid.strip().isdigit() and (root in command.lower() or int(pid) in owned):
                    found.append((int(pid), command))
                    owned.discard(int(pid))
            found.extend((pid, "owned by the drill's job") for pid in owned)
        else:
            for entry in Path("/proc").iterdir():
                if not entry.name.isdigit():
                    continue
                try:
                    command = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
                except (FileNotFoundError, ProcessLookupError):
                    continue
                owned = root in command.lower()
                if not owned:
                    try:
                        if entry.stat().st_uid != os.geteuid():
                            continue
                        environment = (entry / "environ").read_bytes().split(b"\0")
                        owned = f"FASTCTX_DRILL_OWNER={self.root}".encode() in environment
                    # One of this user's processes may still keep its environment from it, as one
                    # with capabilities of its own does; nothing the lab started is such.
                    except (FileNotFoundError, ProcessLookupError, PermissionError):
                        continue
                if owned:
                    found.append((int(entry.name), command))
        return [(pid, command) for pid, command in found if pid != os.getpid()]

    def end_all(self):
        """Ends everything of this lab still running."""
        self.ending.set()
        deadline = time.monotonic() + 30
        while processes := self.processes():
            if time.monotonic() >= deadline:
                raise RuntimeError(f"owned processes survived cleanup: {processes}")
            for pid, _ in processes:
                self.kill(pid)
            if self.node and self.node.poll() is not None:
                self.node.wait(timeout=1)
            time.sleep(0.2)
        if self.scope:
            self.scope.close()

    def kill(self, pid):
        if os.name == "nt":
            ended = subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True, timeout=30)
            if ended.returncode != 0 and any(found == pid for found, _ in self.processes()):
                raise RuntimeError(f"ending this lab's process {pid} failed ({ended.returncode}): "
                                   f"{ended.stdout!r} {ended.stderr!r}")
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
            self.node.wait(timeout=30)

    def restart(self):
        """Ends the node and starts this lab's again at once. A command still running when the node went
        may start one of its own as this one comes up, with the user's environment: one that lost the
        World to this lab's ends by itself; one that took it is ended, and this lab's started again,
        until the node is this lab's."""
        with self.lock:
            self.restarter = threading.current_thread()
            self.restarting.set()
            self.generation += 1
            try:
                deadline = time.monotonic() + 120
                while True:
                    self.end_node()
                    if self.start_node() and self.settled():
                        break
                    if time.monotonic() >= deadline:
                        raise RuntimeError("a node started by a command kept taking this lab's World")
            finally:
                time.sleep(2)
                self.restarting.clear()

    def settled(self):
        """Whether this lab's node serves alone, once a node a command started has had time to end."""
        give_up = time.monotonic() + 10
        while self.foreign_nodes():
            if time.monotonic() >= give_up or self.node.poll() is not None:
                return False
            time.sleep(0.5)
        return self.node.poll() is None

    def foreign_nodes(self):
        """Nodes of this lab's program a command started (`fastctx node --dir`), not this lab."""
        return [pid for pid, command in self.node_commands() if "--dir" in command]

    def conversations(self):
        return self.home / "world" / "conversations"

    def node_log(self):
        path = self.home / "world" / "node.log"
        return path.read_text(encoding="utf-8", errors="replace")


def until(label, probe, limit=90, every=0.5):
    deadline = time.monotonic() + limit
    last = "no probe completed"
    while time.monotonic() < deadline:
        try:
            if probe():
                if time.monotonic() <= deadline:
                    return True
                last = "true only after the deadline"
            else:
                last = "not yet true"
        except Exception as error:  # noqa: BLE001 - observation may recover on the next probe
            last = f"{type(error).__name__}: {error}"
        time.sleep(min(every, max(0, deadline - time.monotonic())))
    raise RuntimeError(f"{label} did not happen within {limit} s; last probe: {last}")


class Drill:
    def __init__(self, lab, rng):
        self.lab = lab
        self.rng = rng
        self.talks = []
        self.deleted = []
        # Capture each thread before deletion can remove its identity record.
        self.threads = {}
        self.speakers = []
        self.background_failures = []
        self.deleted_while_busy = 0
        self.plain_calls = 0
        self.slow_calls = []
        self.outcomes = []
        # Conversations a deletion could not delete within its two minutes, and why it last heard.
        self.undeleted = []
        self.lock = threading.Lock()
        self.stop = threading.Event()

    def note(self, what):
        with self.lock:
            self.outcomes.append(what)

    def guarded(self, label, action, *args):
        try:
            action(*args)
        except Exception as error:  # noqa: BLE001 - thread failures must reach the verdict
            with self.lock:
                self.background_failures.append((label, f"{type(error).__name__}: {error}"))
            self.stop.set()

    def open(self):
        generation = self.lab.generation
        restarting = self.lab.restarting.is_set()
        code, out = self.lab.run("talk", "--open", "codex", "hello", timeout=240)
        found = re.search(r"=== opened (\S+) on ", out)
        if code != 0 or not found:
            self.note(("open failed", code, out[-400:]))
            if not restarting and generation == self.lab.generation and not self.lab.restarting.is_set():
                raise RuntimeError(f"open failed without an injected restart ({code}): {out[-1000:]}")
            return None
        talk = found.group(1)
        thread = thread_of(self.lab, talk)
        if not thread_files(self.lab, thread):
            raise RuntimeError(f"{talk}: no Codex record observed for thread {thread}")
        with self.lock:
            self.talks.append(talk)
            self.threads[talk] = thread
        self.note(("opened", talk))
        return talk

    def speak(self, talk, words, timeout=240, deleting=False):
        started = time.monotonic()
        generation = self.lab.generation
        restarting = self.lab.restarting.is_set()
        code, out = self.lab.run("talk", talk, words, timeout=timeout)
        took = time.monotonic() - started
        self.note(("said", talk, words[:30], code, round(took, 1), out.strip()[-160:] if code else ""))
        if code != 0 and not deleting and not restarting and generation == self.lab.generation and not self.lab.restarting.is_set():
            raise RuntimeError(f"speak failed without an injected restart or deletion ({code}): {out[-1000:]}")
        return code, out

    def delete(self, talk, timeout=180):
        thread = self.threads[talk]
        deadline = time.monotonic() + timeout
        # A failed command may have left an intent that recovery has since completed.
        if not present(self.lab.conversations() / talk) and not left_of(self.lab, talk, thread, timeout=timeout):
            code, out = 0, f"{talk}: deleted by recovery"
            gone = True
        else:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False, "deletion deadline expired while checking recovery"
            code, out = self.lab.run("data", "delete", "--yes", talk, timeout=remaining)
            gone = code == 0 and any(line.startswith(f"{talk}: deleted, ") for line in out.splitlines())
        self.note(("delete", talk, code, out.strip()[-200:]))
        if gone:
            with self.lock:
                if talk in self.talks:
                    self.talks.remove(talk)
                self.deleted.append(talk)
        return gone, out

    def delete_working(self, talk):
        """Deletes a conversation that may be working, stopping what it runs, within two minutes."""
        deadline = time.monotonic() + 120
        state = record(self.lab.conversations() / talk / "now")
        if state.get("activity") == "busy" or state.get("turn"):
            with self.lock:
                self.deleted_while_busy += 1
        last = "no deletion tried"
        while (remaining := deadline - time.monotonic()) > 0:
            gone, out = self.delete(talk, timeout=remaining)
            if gone and time.monotonic() <= deadline:
                return True
            last = out
            for job in sorted(set(re.findall(r"\bfastctx stop (j-[0-9a-z]{6})\b", out))):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                code, stopped = self.lab.run("stop", job, timeout=min(60, remaining))
                self.note(("stop", job, code, stopped.strip()[-120:]))
            time.sleep(min(self.rng.uniform(0.2, 1.5), max(0, deadline - time.monotonic())))
        state = self.state_of(talk)
        with self.lock:
            self.undeleted.append((talk, state, last[-2000:]))
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
            if roll < 0.3:
                self.speak(talk, "hello again")
            elif roll < 0.5:
                pause = self.rng.choice(["0.2", "1", "2"])
                self.speak(talk, "[[run: " + self.lab.on(f"echo w{index}; sleep {pause}") + "]]")
            elif roll < 0.6:
                # Left running: its end comes back through the conversation's
                # inbox while others' commands start (`world-concurrency.md`,
                # 记录的锁).
                pause = self.rng.choice(["2", "4"])
                self.speak(talk, "[[later: " + self.lab.on(f"echo l{index}; sleep {pause}") + "]]")
            elif roll < 0.7:
                self.speak(talk, "[[run: " + self.lab.on("sleep 25") + "]]", timeout=300)
            elif roll < 0.78:
                self.speak(talk, f"[[slow: {self.rng.choice(['1', '3'])}]]")
            elif roll < 0.83:
                self.speak(talk, "[[hang]]", timeout=120)
            else:
                # Deleted while it works: refused while a turn or a command
                # of it runs, as a user would, its command is stopped and the
                # deletion asked again until it goes.
                words = self.rng.choice(["[[run: " + self.lab.on("sleep 8") + "]]", "[[slow: 3]]", "hello"])
                speaking = threading.Thread(target=self.guarded, args=(f"speaking to {talk}", self.speak, talk, words, 240, True),
                                            daemon=True)
                with self.lock:
                    self.speakers.append(speaking)
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
            generation = self.lab.generation
            started = time.monotonic()
            code, out = self.lab.run("w", timeout=60)
            took = time.monotonic() - started
            if not self.lab.restarting.is_set() and self.lab.generation == generation:
                with self.lock:
                    self.plain_calls += 1
                    if code != 0 or took > ANSWERS_WITHIN:
                        self.slow_calls.append((round(took, 1), code, out[-400:]))
            time.sleep(2)

    def chaos(self):
        while not self.stop.wait(self.rng.uniform(40, 80)):
            say("  ending the node and starting it again")
            self.lab.restart()
            self.note(("restarted",))


def present(path):
    """Whether a path exists; an unreadable path is not evidence of absence."""
    try:
        path.lstat()
    except FileNotFoundError:
        return False
    return True


def record(path):
    """A record the node keeps, as JSON. On Windows a file being replaced at
    that moment refuses to open for a few milliseconds, so it is read again."""
    for attempt in range(50):
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except PermissionError:
            if attempt == 49:
                raise
            time.sleep(0.05)


def entries(directory, missing_ok=False):
    try:
        return list(directory.iterdir())
    except FileNotFoundError:
        if missing_ok:
            return []
        raise


def settled(lab, talks):
    """Whether every conversation kept here is idle with no words on their way in."""
    # An open command can fail after creating its conversation.
    found = {directory.name for directory in entries(lab.conversations())}
    for talk in sorted(set(talks) | found):
        directory = lab.conversations() / talk
        # An opening the node did not live to record is no conversation (`world-concurrency.md`,
        # Codex 对话出生): nothing of it is listed, working or waiting.
        if talk not in talks and not (directory / "about").exists():
            continue
        state = record(directory / "now")
        if (not isinstance(state, dict) or state.get("activity") not in ("busy", "idle", "asleep")
                or "turn" not in state or (state["turn"] is not None and not isinstance(state["turn"], str))):
            raise ValueError(f"{talk}: invalid activity record: {state!r}")
        if state["activity"] == "busy" or state.get("turn"):
            return False
        if any(path.suffix == ".json" for path in entries(directory / "inbox", missing_ok=True)):
            return False
    return True


def thread_of(lab, talk):
    """The recorded Codex thread, before anything of the conversation can be deleted."""
    about = record(lab.conversations() / talk / "about")
    if not isinstance(about, dict) or not isinstance(about.get("session"), str) or not about["session"]:
        raise ValueError(f"{talk}: no Codex thread in its identity record")
    if about.get("home") is not None and Path(about["home"]).resolve() != lab.codex_home.resolve():
        raise ValueError(f"{talk}: Codex home differs from this lab's: {about['home']}")
    return about["session"]


def thread_files(lab, thread):
    """Find a thread's rollouts in both Codex stores, surfacing every scan error."""
    if not thread:
        raise ValueError("cannot check a Codex thread whose identity was not captured")
    pending = [path for path in entries(lab.codex_home) if path.name in ("sessions", "archived_sessions")]
    found = []
    while pending:
        for path in entries(pending.pop(), missing_ok=True):
            try:
                metadata = path.lstat()
            except FileNotFoundError:
                # Another worker can finish deleting an unrelated thread during this scan.
                continue
            if stat.S_ISDIR(metadata.st_mode):
                pending.append(path)
            elif thread in path.name:
                found.append(path)
    return found


def listed(lab, timeout=120):
    code, out = lab.run("data", "list", "--json", timeout=timeout)
    if code != 0:
        raise RuntimeError(f"data list failed ({code}): {out[-2000:]}")
    space = json.loads(out)
    if not isinstance(space, dict) or not isinstance(space.get("conversations"), list):
        raise ValueError(f"data list has no conversation list: {out[-2000:]}")
    names = []
    for conversation in space["conversations"]:
        if not isinstance(conversation, dict) or not isinstance(conversation.get("name"), str):
            raise ValueError(f"data list has an invalid conversation: {conversation!r}")
        names.append(conversation["name"])
    return names


def left_of(lab, talk, thread, timeout=120):
    """What is still here of a deleted conversation: its directory, its listing, its Codex rollouts."""
    left = []
    if present(lab.conversations() / talk):
        left.append("its directory")
    if talk in listed(lab, timeout=timeout):
        left.append("its listing")
    files = thread_files(lab, thread)
    if files:
        left.append(f"its Codex thread {thread}: {files[:5]}")
    return left


def halfway(lab):
    """The conversations whose deletion was begun and is not done: their intent is still kept."""
    return sorted(directory.name for directory in entries(lab.conversations())
                  if present(directory / "deleting") or present(directory / "host-deleted"))


def check_empty(label, probe):
    try:
        left = probe()
    except Exception as error:  # noqa: BLE001 - an observation failure is a failed check
        return check(label, False, f"{type(error).__name__}: {error}")
    return check(label, not left, left)


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
    mock = lab = drill = None
    threads = []
    try:
        mock = Mock()
        lab = Lab(args.program, args.codex, root, mock)
        if not lab.start_node():
            raise RuntimeError("this lab's node did not take its World")
        code, out = lab.run("apply", "--target", "codex", "--home", str(lab.codex_home), timeout=180)
        if not check("the Codex login gets the World's tools", code == 0, out):
            return 1
        drill = Drill(lab, rng)
        threads = [threading.Thread(target=drill.guarded, args=(f"worker {index}", drill.worker, index), daemon=True)
                   for index in range(args.workers)]
        threads.append(threading.Thread(target=drill.guarded, args=("plain-call probe", drill.probe), daemon=True))
        if args.restarts:
            threads.append(threading.Thread(target=drill.guarded, args=("node restarter", drill.chaos), daemon=True))
        for thread in threads:
            thread.start()
        drill.stop.wait(args.minutes * 60)
        drill.stop.set()
        for thread in threads:
            thread.join(timeout=360)
        if not check("every worker came back", all(not thread.is_alive() for thread in threads)):
            return 1
        for thread in drill.speakers:
            thread.join(timeout=300)
        if not check("every concurrent speaker came back", all(not thread.is_alive() for thread in drill.speakers)):
            return 1
        check("no background thread failed", not drill.background_failures, drill.background_failures)
        say(f"  {len(drill.outcomes)} things done, {len(drill.deleted)} conversations deleted, {len(mock.requests)} model requests")
        opened = sum(1 for outcome in drill.outcomes if outcome[0] == "opened")
        failures = [outcome for outcome in drill.outcomes if outcome[0] == "open failed"]
        check("conversations opened and were spoken to", opened > 0 and len(mock.requests) > 0, failures[:3])
        ends = sum(1 for request in mock.requests if re.search(r"=== j-\w+ on \S+ ended \(", json.dumps(request)))
        check("commands left running ended and their ends went in as notices", ends > 0, f"{ends} ends heard")
        check("deletion was actually attempted while a conversation was working", drill.deleted_while_busy > 0)
        check("every deletion went through within two minutes of stopping what its conversation ran",
              bool(drill.deleted) and not drill.undeleted, drill.undeleted[:5] or "no deletion completed")
        if args.restarts:
            check("the node was restarted", any(outcome[0] == "restarted" for outcome in drill.outcomes))
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
        check("sampled plain calls answered within a few seconds outside restarts",
              drill.plain_calls > 0 and not drill.slow_calls, drill.slow_calls[:20] or "no plain call observed")
        try:
            until("every conversation settles", lambda: settled(lab, list(drill.talks)), limit=180, every=2)
            check("every conversation settled: none working, no words on their way in", True)
        except RuntimeError as error:
            check("every conversation settled: none working, no words on their way in", False, error)
        try:
            until("no deletion is left halfway", lambda: not halfway(lab), limit=RECOVERY_OBSERVATION, every=2)
            check("deletions a stopped node began were finished: no deletion is left halfway", True)
        except RuntimeError as error:
            check("deletions a stopped node began were finished: no deletion is left halfway", False, error)
        for talk in list(drill.deleted):
            check_empty(f"{talk}, deleted, left no directory, listing or Codex rollout",
                        lambda: left_of(lab, talk, drill.threads[talk]))
        check_empty("the node's log has no lock out of order, nothing stuck, no circle",
                    lambda: [line for line in lab.node_log().splitlines() if BAD_LINES.search(line)])
    except Exception as error:  # noqa: BLE001 - keep the lab and a failed verdict on setup or inspection errors
        check("the drill completed", False, f"{type(error).__name__}: {error}")
    finally:
        if drill:
            drill.stop.set()
        try:
            if lab:
                lab.end_all()
            deadline = time.monotonic() + 30
            for thread in threads + (drill.speakers if drill else []):
                thread.join(timeout=max(0, deadline - time.monotonic()))
                if thread.is_alive():
                    check("every drill thread ended during cleanup", False, thread.name)
        except Exception as error:  # noqa: BLE001 - cleanup failure must not erase the verdict
            check("the lab's processes could be ended", False, error)
        finally:
            if mock:
                try:
                    mock.close()
                except Exception as error:  # noqa: BLE001 - mock cleanup is part of the verdict
                    check("the model mock ended", False, error)
        if lab and not all(RESULTS):
            say("--- node.log (last 80 lines) ---")
            try:
                say("\n".join(lab.node_log().splitlines()[-80:]))
            except OSError as error:
                say(f"could not read node.log: {error}")
        say(f"lab: {root}")
    return 0 if all(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
