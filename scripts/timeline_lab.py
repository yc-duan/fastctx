"""An isolated Windows node and its raw, length-framed local IPC for timeline_drill."""

import base64
import contextlib
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import sqlite3
import struct
import subprocess
import sys
import time

from drill_processes import WindowsScope


class CallFailed(RuntimeError):
    def __init__(self, failure):
        super().__init__(failure)
        self.failure = failure


def put_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    staged = path.with_suffix(path.suffix + ".tmp")
    staged.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")
    staged.replace(path)


def until(probe, seconds=90):
    deadline = time.monotonic() + seconds
    while True:
        value = probe()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError(f"not ready after {seconds}s")
        time.sleep(0.1)


class IPC:
    def __init__(self, pipe):
        self.file = open(pipe, "r+b", buffering=0)
        self.send({"protocol": 13, "version": "timeline-drill", "notices": 1, "shapes": 3,
                   "peer": {"kind": "command", "resident": None, "session": None}})
        self.welcome = self.read()
        if self.welcome.get("kind") != "accepted":
            raise RuntimeError(self.welcome)
        if not self.welcome.get("timeline_reads"):
            raise RuntimeError("node does not advertise timeline_reads")
        self.sequence = 0

    def close(self):
        self.file.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def send(self, value):
        data = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        self.file.write(struct.pack("<I", len(data)) + data)

    def exact(self, length):
        result = bytearray()
        while len(result) < length:
            piece = self.file.read(length - len(result))
            if not piece:
                raise EOFError("node closed its pipe")
            result.extend(piece)
        return result

    def read(self):
        length = struct.unpack("<I", self.exact(4))[0]
        return json.loads(self.exact(length))

    def call(self, call):
        self.sequence += 1
        self.send({"kind": "call", "id": self.sequence, "call": call})
        frame = self.read()
        if frame.get("kind") != "reply" or frame["id"] != self.sequence:
            raise RuntimeError(frame)
        if "Err" in frame["reply"]:
            raise CallFailed(frame["reply"]["Err"])
        return frame["reply"]["Ok"]


class Lab:
    def __init__(self, kernel, root):
        self.root = root.resolve()
        if self.root.exists():
            raise ValueError(f"use a fresh --root: {self.root}")
        self.root.mkdir(parents=True)
        self.scope = WindowsScope()
        self.program = self.root / "fastctx.exe"
        shutil.copy2(kernel, self.program)
        self.home = self.root / "home"
        self.world = self.home / "world"
        self.user = self.root / "user"
        self.claude = self.user / ".claude"
        self.codex = self.root / "codex"
        self.bin = self.root / "fake-bin"
        for directory in (self.world, self.user, self.claude, self.codex, self.bin):
            directory.mkdir(parents=True, exist_ok=True)
        self.probe_path = self.root / "probe-address"
        self.node = None
        self.generation = 0
        self.pipe = None
        self.fixtures = {}
        self.environment = {key: value for key, value in os.environ.items()
                            if not key.upper().startswith(("CLAUDE", "FASTCTX", "CODEX", "ANTHROPIC", "OPENAI"))}
        windows = Path(os.environ["SystemRoot"])
        self.environment.update(FASTCTX_HOME=str(self.home), FASTCTX_USER_HOME=str(self.user),
                                FASTCTX_TIMELINE_DRILL_PROBE=str(self.probe_path), CODEX_HOME=str(self.codex),
                                USERPROFILE=str(self.user), HOME=str(self.user),
                                APPDATA=str(self.user / "AppData/Roaming"),
                                LOCALAPPDATA=str(self.user / "AppData/Local"),
                                PATH=os.pathsep.join(map(str, [self.bin, Path(sys.executable).parent,
                                                              windows / "System32", windows])))
        assert not any(key.upper().startswith("CLAUDE") for key in self.environment)
        (self.bin / "claude.cmd").write_text(
            '@echo off\r\nif "%1"=="agents" (echo [] & exit /b 0)\r\n'
            'if "%1"=="--version" (echo 2.1.322 & exit /b 0)\r\n'
            'echo fake Claude refused %* 1>&2\r\nexit /b 88\r\n', encoding="ascii")
        put_json(self.world / "machine.json", {"name": "timeline-drill"})
        put_json(self.world / "agent-scan.json", {
            "version": 1, "workbench_seeded": True,
            "claude": {"workbench": [str(self.claude)]}, "codex": {"workbench": [str(self.codex)]}})
        # A connected fixture login keeps the normal startup refresh from closing its workbench.
        put_json(self.home / "applied.json", {
            "version": 2, "created": {"files": [], "directories": []}, "homes": [{"host": "claude", "home": str(self.claude),
                "receipt": {"version": 1, "codex": None, "claude_md": None,
                            "settings": None, "created": {"files": [], "directories": []}}}]})
        version = subprocess.run([str(self.program), "--version"], env=self.environment,
                                 capture_output=True, text=True, check=True).stdout.strip()
        if "development build" not in version:
            raise ValueError(f"refusing a release kernel: {version}")
        put_json(self.root / "isolation.json", {"version": version, "program": str(self.program),
                 "kernel_sha256": hashlib.sha256(self.program.read_bytes()).hexdigest(),
                 "FASTCTX_HOME": str(self.home), "FASTCTX_USER_HOME": str(self.user),
                 "PATH": self.environment["PATH"], "CLAUDE_variables": []})

    def seed(self, resident, rows, agent="claude"):
        session = resident + "-session"
        directory = self.world / "conversations" / resident
        directory.mkdir(parents=True)
        put_json(directory / "about", {"id": resident, "host": {"agent": agent,
            "client": "offline-timeline-drill", "version": "fixture"}, "machine": "timeline-drill",
            "born_at": "2026-11-10T00:00:00Z", "origin": {"kind": "user"},
            "session": session, "world_started": True,
            "home": None if agent == "claude" else str(self.codex)})
        put_json(directory / "now", {"activity": "idle", "since": "2026-11-10T00:00:00Z",
            "turn": None, "answering": [], "trouble": None})
        if agent == "claude":
            exchange = self.world / "claude" / session
            exchange.mkdir(parents=True)
            put_json(exchange / "resident.json", resident)
            put_json(exchange / "session.json", {"session": session, "instance": "fixture",
                "at": "2026-11-10T00:00:00Z", "process": None, "inbox_address": None,
                "entrypoint": "offline-fixture", "cwd": str(self.root), "model": None, "version": "fixture"})
            path = self.claude / "projects" / "drill" / (session + ".jsonl")
            put_json(exchange / "transcript.json", {"path": str(path)})
            self.fixtures[resident] = path
            self.rows(resident, rows)
        return session

    def rows(self, resident, rows):
        path = self.fixtures[resident]
        path.parent.mkdir(parents=True, exist_ok=True)
        staged = path.with_suffix(".tmp")
        with staged.open("w", encoding="utf-8", newline="\n") as file:
            for row in rows:
                file.write(json.dumps(row, ensure_ascii=False, separators=(",", ":")) + "\n")
        staged.replace(path)

    def start(self):
        self.probe_path.unlink(missing_ok=True)
        self.generation += 1
        output = open(self.root / f"node-{self.generation}.out", "ab")
        self.node = subprocess.Popen([str(self.program), "node"], env=self.environment,
                                     stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT)
        output.close()
        def ready():
            if self.node.poll() is not None:
                raise RuntimeError(f"node exited {self.node.returncode}")
            log = self.log()
            matches = re.findall(r"listening at (.+)", log)
            if self.probe_path.exists() and matches:
                self.pipe = matches[-1].strip()
                try:
                    with self.connect():
                        return True
                except OSError:
                    pass
            return False
        until(ready, 120)

    def stop(self):
        if self.node is not None and self.node.poll() is None:
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(self.node.pid)],
                           capture_output=True, check=False)
            self.node.wait(timeout=30)
        self.node = None

    def connect(self):
        return IPC(self.pipe)

    def call(self, call):
        with self.connect() as client:
            return client.call(call)

    def timeline(self, resident, do="page", **arguments):
        if do == "page":
            arguments = {"from": {"from": "latest"}, "max_events": 400,
                         "max_bytes": 1048576, **arguments}
        return self.call({"call": "timeline", "machine": None,
                          "request": {"do": do, "resident": resident, **arguments}})

    def golden(self, resident):
        host, port = self.probe_path.read_text().split(":")
        with socket.create_connection((host, int(port)), timeout=180) as connection:
            connection.sendall((resident + "\n").encode("ascii"))
            value = json.load(connection.makefile("r", encoding="utf-8"))
        if "error" in value:
            raise CallFailed(value["error"])
        if value["unread"] or value["unsettled"] or value["unavailable"]:
            raise RuntimeError({key: value[key] for key in ("unread", "unsettled", "unavailable")})
        if value["stamp_before"] != value["stamp_after"]:
            raise RuntimeError("history changed during reference reading")
        return value

    def stats(self, resident):
        with contextlib.closing(sqlite3.connect(f"file:{self.world.as_posix()}/content.sqlite3?mode=ro", uri=True)) as db:
            row = db.execute("SELECT streams.id, streams.revision, streams.next FROM streams JOIN scopes "
                "ON scopes.id=streams.scope WHERE scopes.key=? AND streams.name='timeline'",
                ("conversation/" + resident,)).fetchone()
            if row is None:
                return {"revision": None, "records": 0, "items": 0, "object_refs": 0}
            count = db.execute("SELECT count(*) FROM items WHERE stream=?", (row[0],)).fetchone()[0]
            refs = db.execute("SELECT count(*) FROM object_refs JOIN scopes ON scopes.id=object_refs.scope "
                              "WHERE scopes.key=?", ("conversation/" + resident,)).fetchone()[0]
            return {"revision": row[1], "records": row[2], "items": count, "object_refs": refs}

    def content_rows(self, resident):
        scope = "conversation/" + resident
        with contextlib.closing(sqlite3.connect(f"file:{self.world.as_posix()}/content.sqlite3?mode=ro", uri=True)) as db:
            db.execute("BEGIN")
            counts = {"scopes": db.execute("SELECT count(*) FROM scopes WHERE key=?", (scope,)).fetchone()[0]}
            for table in ("streams", "frames", "items", "object_refs"):
                join = "JOIN scopes ON scopes.id=streams.scope" if table == "streams" else (
                    "JOIN scopes ON scopes.id=object_refs.scope" if table == "object_refs" else
                    f"JOIN streams ON streams.id={table}.stream JOIN scopes ON scopes.id=streams.scope")
                counts[table] = db.execute(f"SELECT count(*) FROM {table} {join} WHERE scopes.key=?", (scope,)).fetchone()[0]
            counts["frame_records"] = db.execute("SELECT coalesce(sum(frames.count),0) FROM frames JOIN streams "
                "ON streams.id=frames.stream JOIN scopes ON scopes.id=streams.scope WHERE scopes.key=?", (scope,)).fetchone()[0]
            counts["checkpoints"] = db.execute("SELECT count(*) FROM checkpoints WHERE name=?", ("timeline/" + resident,)).fetchone()[0]
            global_rows = {table: db.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
                           for table in ("scopes", "streams", "frames", "items", "object_refs")}
            rows = db.execute("SELECT scopes.key,streams.revision,streams.next,"
                "(SELECT count(*) FROM frames WHERE stream=streams.id),"
                "(SELECT coalesce(sum(count),0) FROM frames WHERE stream=streams.id),"
                "(SELECT count(*) FROM items WHERE stream=streams.id),"
                "(SELECT count(*) FROM object_refs WHERE scope=scopes.id) "
                "FROM scopes JOIN streams ON streams.scope=scopes.id WHERE scopes.key=?", (scope,)).fetchall()
        return {"counts": counts, "columns": ["scope", "revision", "next", "frames", "frame_records", "items", "object_refs"],
                "rows": rows, "global_rows": global_rows}

    def referenced_objects(self, resident):
        with contextlib.closing(sqlite3.connect(f"file:{self.world.as_posix()}/content.sqlite3?mode=ro", uri=True)) as db:
            hashes = [row[0].hex() for row in db.execute("SELECT hash FROM object_refs JOIN scopes "
                "ON scopes.id=object_refs.scope WHERE scopes.key=? AND count>0", ("conversation/" + resident,))]
        return self.object_facts(hashes)

    def object_facts(self, hashes):
        with contextlib.closing(sqlite3.connect(f"file:{self.world.as_posix()}/objects.sqlite3?mode=ro", uri=True)) as db:
            result = []
            for name in hashes:
                row = db.execute("SELECT size,made FROM objects WHERE hash=?", (bytes.fromhex(name),)).fetchone()
                path = self.world / "objects" / name[:2] / name
                result.append({"hash": name, "bytes": row[0] if row else None, "made_ms": row[1] if row else None,
                               "stored": row is not None, "file": str(path), "file_exists": path.is_file()})
            return result

    def content_forget_jobs(self, resident):
        with contextlib.closing(sqlite3.connect(f"file:{self.world.as_posix()}/node.sqlite3?mode=ro", uri=True)) as db:
            db.row_factory = sqlite3.Row
            return [dict(row) for row in db.execute("SELECT kind,key,unit,state,due,tries,failure FROM jobs "
                "WHERE kind='content/forget' AND key=?", (resident,))]

    def jobs(self, resident):
        with contextlib.closing(sqlite3.connect(f"file:{self.world.as_posix()}/node.sqlite3?mode=ro", uri=True)) as db:
            db.row_factory = sqlite3.Row
            return [dict(row) for row in db.execute("SELECT * FROM jobs WHERE kind='timeline' AND unit=?", (resident,))]

    def log(self):
        path = self.world / "node.log"
        return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""

    def builds(self, resident):
        pattern = re.escape(resident) + r"'s timeline: (\d+) events, (\d+) written, (\d+) gone"
        return [tuple(map(int, match)) for match in re.findall(pattern, self.log())]

    def settled_build(self, resident, action=None):
        count = len(self.builds(resident))
        before = self.stats(resident)
        if action is not None:
            action()
        began = time.perf_counter()
        first = self.timeline(resident)
        until(lambda: len(self.builds(resident)) > count and not self.jobs(resident), 180)
        return {"before": before, "after": self.stats(resident), "builds": self.builds(resident)[count:],
                "first_revision": first["revision"], "first_events": len(first["events"]),
                "ready_after_s": time.perf_counter() - began}

    def pages(self, resident, size=137):
        pages = []
        page = self.timeline(resident, max_events=size)
        revision = page["revision"]
        while True:
            if page["revision"] != revision:
                raise RuntimeError("revision changed during pagination")
            pages.append(page["events"])
            if page["older"] is None:
                break
            page = self.timeline(resident, max_events=size, **{"from": {"from": "before", "place": page["older"]}})
        events, objects = [], []
        for entry in (event for page in reversed(pages) for event in page):
            if entry["shown"] == "event":
                events.append(entry["event"])
            else:
                body = bytearray()
                while len(body) < entry["bytes"]:
                    reply = self.timeline(resident, "object", object=entry["object"], offset=len(body), length=262144)
                    part = base64.b64decode(reply["base64"], validate=True)
                    if not part:
                        raise RuntimeError("empty object range before its end")
                    body.extend(part)
                if len(body) != entry["bytes"] or hashlib.sha256(body).hexdigest() != entry["object"]:
                    raise RuntimeError("object length or hash mismatch")
                events.append(json.loads(body))
                objects.append({"bytes": len(body), "hash": entry["object"]})
        return events, {"revision": revision, "pages": len(pages), "events": len(events), "objects": objects}

    def evidence(self, name, value):
        with gzip.open(self.root / (name + ".json.gz"), "wt", encoding="utf-8") as file:
            json.dump(value, file, ensure_ascii=False, separators=(",", ":"))

    def cleanup(self):
        self.stop()
        pids = self.scope.pids()
        if pids:
            subprocess.run(["taskkill", "/F", "/T", *sum((["/PID", str(pid)] for pid in pids), [])],
                           capture_output=True, check=False)
            until(lambda: not self.scope.pids(), 30)
        remaining = self.scope.pids()
        put_json(self.root / "cleanup.json", {"remaining_owned_pids": sorted(remaining)})
        self.scope.close()
        if remaining:
            raise RuntimeError(f"processes survived cleanup: {remaining}")
