#!/usr/bin/env python3
"""Exercise Claude wake admission with the official CLI and a local fake model.

Linux, root, iproute2 and unshare are required; tested with Claude Code 2.1.322.
No account or API key is used.
Example (WSL lingsh):
  python3 scripts/claude-wake-once.py --kernel /path/to/fastctx \
    --claude /path/to/claude --root /root/wake-drill-001 --rounds 3
The root must not exist. Each invocation creates a network/mount namespace,
mounts its own /etc/environment, and retains commands, transcripts and results.
Use --scenarios raw for the direct-CLI counterexample, or select a comma-separated
subset of concurrent,slow,restart-late,restart-before. Restart-before waits for
the real reconciliation window; no production timing constant is overridden.
The optional copy-cleanup case deliberately injects a copy, fails stop/rm once,
and restarts the node before checking cleanup and permanent pairing exclusion.
"""

import argparse
import concurrent.futures
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Model(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    directory = None
    lock = threading.Lock()
    sequence = 0

    def log_message(self, *_):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"{}")

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("content-length", 0)))
        with self.lock:
            type(self).sequence += 1
            number = type(self).sequence
        (self.directory / f"model-{number:04}.json").write_bytes(raw)
        if "count_tokens" in self.path:
            body = b'{"input_tokens":1000}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        request = json.loads(raw)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        events = [
            ("message_start", {"message": {"id": f"msg_wake_{number}", "type": "message", "role": "assistant", "model": request.get("model", "claude-fable-5-2"), "content": [], "stop_reason": None, "stop_sequence": None, "usage": {"input_tokens": 1000, "output_tokens": 1}}}),
            ("content_block_start", {"index": 0, "content_block": {"type": "text", "text": ""}}),
            ("content_block_delta", {"index": 0, "delta": {"type": "text_delta", "text": "Local wake experiment completed."}}),
            ("content_block_stop", {"index": 0}),
            ("message_delta", {"delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 10}}),
            ("message_stop", {}),
        ]
        try:
            for kind, body in events:
                payload = {"type": kind, **body}
                self.wfile.write((f"event: {kind}\ndata: {json.dumps(payload)}\n\n").encode())
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        self.close_connection = True


WRAPPER = r"""#!/usr/bin/python3
import json,os,re,signal,subprocess,sys,threading,time
from pathlib import Path
root=Path(__ROOT__); real=__CLAUDE__; args=sys.argv[1:]
def event(**row):
 with (root/'cli.jsonl').open('a') as f:f.write(json.dumps({'at':time.time(),'pid':os.getpid(),**row})+'\n')
event(event='call',args=args)
if args and args[0] in ('stop','rm') and (root/'fault.json').exists() and json.loads((root/'fault.json').read_text()).get('kind')=='copy-cleanup':
 try:
  fd=os.open(root/('cleanup-fail-'+args[0]),os.O_CREAT|os.O_EXCL|os.O_WRONLY);os.close(fd)
  event(event='injected-cleanup-failure',args=args);print('injected cleanup failure',file=sys.stderr);sys.exit(17)
 except FileExistsError:pass
resume='--resume' in args and '--bg' in args
fault={}
if resume and (root/'fault.json').exists():
 candidate=json.loads((root/'fault.json').read_text())
 try:
  fd=os.open(root/'fault-used',os.O_CREAT|os.O_EXCL|os.O_WRONLY);os.close(fd);fault=candidate
 except FileExistsError: pass
 if fault and fault['kind'] in ('slow','restart-before'):
  (root/'launch-blocked').write_text(str(os.getpid()))
  event(event='delayed',fault=fault)
  time.sleep(600 if fault['kind']=='restart-before' else fault['delay'])
paused=[]
if fault.get('kind')=='restart-late':
 namespace=os.readlink('/proc/self/ns/net')
 for path in Path('/proc').iterdir():
  if not path.name.isdigit():continue
  try:
   if os.readlink(path/'ns/net')!=namespace:continue
   argv=(path/'cmdline').read_bytes().split(b'\0')
   if b'--bg-spare' in argv and b'--bg-pty-host' not in argv:
    pid=int(path.name);os.kill(pid,signal.SIGSTOP);paused.append(pid)
  except OSError:pass
 event(event='paused-spares',workers=paused)
 for pid in paused:
  code='import os,signal,time;time.sleep('+str(fault['delay'])+');os.kill('+str(pid)+',signal.SIGCONT)'
  subprocess.Popen(['/usr/bin/python3','-c',code],start_new_session=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
event(event='exec',args=args)
p=subprocess.run([real,*args],capture_output=True)
if fault.get('kind')=='copy-cleanup' and p.returncode==0:
 p=subprocess.run([real,*args],capture_output=True)
if fault.get('kind')=='restart-late' and p.returncode==0:
 if not paused:
  target=args[args.index('--resume')+1]
  until=time.monotonic()+30
  while not paused and time.monotonic()<until:
   listed=subprocess.run([real,'agents','--json','--all'],capture_output=True)
   for row in json.loads(listed.stdout):
    if row.get('sessionId')==target and row.get('pid'):
     pid=row['pid'];os.kill(pid,signal.SIGSTOP);paused.append(pid)
     event(event='paused-registered-worker',worker=pid)
     code='import os,signal,time;time.sleep('+str(fault['delay'])+');os.kill('+str(pid)+',signal.SIGCONT)'
     subprocess.Popen(['/usr/bin/python3','-c',code],start_new_session=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
     break
   if not paused:time.sleep(.05)
 if paused:(root/'launch-blocked').write_text(str(paused[0]))
event(event='result',args=args,code=p.returncode,out=p.stdout.decode(errors='replace'),err=p.stderr.decode(errors='replace'))
sys.stdout.buffer.write(p.stdout);sys.stderr.buffer.write(p.stderr)
sys.exit(p.returncode)
"""



class Lab:
    def __init__(self, args):
        self.args = args
        self.root = Path(args.root)
        self.root.mkdir()
        for name in ("bin", "home/.claude", "data/bin", "evidence"):
            (self.root / name).mkdir(parents=True)
        shutil.copy2(args.kernel, self.root / "data/bin/fastctx")
        self.kernel = str(self.root / "data/bin/fastctx")
        (self.root / "bin/fastctx").symlink_to(self.kernel)
        self.real = str(Path(args.claude).resolve())
        wrapper = WRAPPER.replace("__ROOT__", repr(str(self.root))).replace("__CLAUDE__", repr(self.real))
        (self.root / "bin/claude").write_text(wrapper)
        (self.root / "bin/claude").chmod(0o755)
        self.env = {"PATH": f"{self.root}/bin:/usr/bin:/bin", "HOME": str(self.root / "home"), "LANG": "C.UTF-8", "CLAUDE_CONFIG_DIR": str(self.root / "home/.claude"), "FASTCTX_HOME": str(self.root / "data"), "FASTCTX_USER_HOME": str(self.root / "home"), "IS_SANDBOX": "1", "ANTHROPIC_BASE_URL": "http://127.0.0.1:18765", "ANTHROPIC_API_KEY": "sk-lab-not-a-key", "ANTHROPIC_AUTH_TOKEN": "sk-lab-not-a-token", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_AUTOUPDATER": "1", "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1", "FASTCTX_UPDATE_RELEASES": "http://127.0.0.1:9/releases"}
        settings = {"env": {key: self.env[key] for key in self.env if key.startswith(("ANTHROPIC_", "CLAUDE_CODE_", "DISABLE_"))}, "skipDangerousModePermissionPrompt": True, "permissions": {"defaultMode": "bypassPermissions"}}
        (self.root / "home/.claude/settings.json").write_text(json.dumps(settings))
        onboarding = {"hasCompletedOnboarding": True, "numStartups": 3, "oauthAccount": {"accountUuid": "lab", "emailAddress": "lab@example.invalid", "organizationUuid": "lab", "organizationName": "Lab"}, "customApiKeyResponses": {"approved": ["sk-lab-not-a-key"], "rejected": []}}
        for name in ("home/.claude.json", "home/.claude/.claude.json"):
            (self.root / name).write_text(json.dumps(onboarding))
        environment = self.root / "environment"
        environment.write_text(f"PATH={self.env['PATH']}\n")
        subprocess.run(["mount", "--bind", str(environment), "/etc/environment"], check=True)
        Model.directory = self.root / "evidence"
        self.server = ThreadingHTTPServer(("127.0.0.1", 18765), Model)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.node = None
        self.start_node()
        self.f("apply", "--target", "claude", "--workbench")
        self.resident = None
        self.session = None
        self.samples = []
        self.maximum = 0
        self.maximum_processes = 0
        self.monitoring = True
        threading.Thread(target=self.monitor, daemon=True).start()

    def command(self, argv, timeout=300):
        return subprocess.run(argv, env=self.env, cwd=(self.root / "data/world/outside") if "--resume" in argv else self.root, capture_output=True, text=True, timeout=timeout)

    def f(self, *args, timeout=300):
        result = self.command([self.kernel, *args], timeout)
        if result.returncode:
            raise RuntimeError(f"fastctx {' '.join(args)}: {result.stdout}{result.stderr}")
        return result.stdout

    def claude(self, *args):
        result = self.command([self.real, *args], 60)
        if result.returncode:
            raise RuntimeError(f"claude {args}: {result.stdout}{result.stderr}")
        return result.stdout

    def start_node(self):
        output = (self.root / "node-stdio.log").open("a")
        self.node = subprocess.Popen([self.kernel, "node"], env=self.env, cwd=self.root, stdout=output, stderr=subprocess.STDOUT)
        output.close()
        end = time.monotonic() + 60
        while time.monotonic() < end:
            try:
                if self.command([self.kernel, "nodes"], 5).returncode == 0:
                    return
            except subprocess.TimeoutExpired:
                pass
            time.sleep(.2)
        raise RuntimeError("node did not start")

    def restart(self):
        controller = json.loads((self.root / "data/world/claude-controller.json").read_text())
        pid = controller["pid"]
        self.samples.append({"kind": "node-restart", "old_pid": pid, "at": time.time()})
        os.kill(pid, signal.SIGKILL)
        if self.node.poll() is None:
            self.node.wait(timeout=10)
        time.sleep(.5)
        self.start_node()

    def agents(self):
        return json.loads(self.claude("agents", "--json", "--all"))

    def monitor(self):
        # Warm daemon spares are unclaimed processes. Count only the public
        # session jobs, including starting jobs whose PID has not appeared yet.
        while self.monitoring:
            try:
                rows = [row for row in self.agents() if row.get("kind") == "background" and row.get("state") not in ("done", "failed", "stopped")]
                self.maximum = max(self.maximum, len(rows))
                pids = {row["pid"] for row in rows if row.get("pid")}
                self.maximum_processes = max(self.maximum_processes, len(pids))
                with (self.root / "processes.jsonl").open("a") as out:
                    out.write(json.dumps({"at": time.time(), "workers": rows}) + "\n")
            except (OSError, RuntimeError, subprocess.TimeoutExpired):
                pass
            time.sleep(.5)

    def open(self):
        began = time.time()
        opened = self.f("talk", "--open", "claude", "--model", "claude-fable-5-2", "WAKE_BIRTH")
        (self.root / "open.txt").write_text(opened)
        self.resident = re.search(r"=== opened (\S+)", opened)[1]
        rows = [row for row in self.agents() if row.get("kind") == "background"]
        if len(rows) != 1:
            raise RuntimeError(f"expected one initial session: {rows}")
        self.session = rows[0]["sessionId"]
        self.samples.append({"kind": "birth", "seconds": time.time() - began})

    def stop_host(self):
        self.claude("stop", self.session[:8])
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            rows = [row for row in self.agents() if row.get("id") == self.session[:8]]
            if rows and rows[0].get("state") in ("stopped", "done", "failed") and not rows[0].get("pid"):
                time.sleep(1)
                return
            time.sleep(.2)
        raise RuntimeError("host never became terminal")

    def transcript_counts(self, markers):
        path = self.root / "data/world/claude" / self.session / "transcript.json"
        transcript = Path(json.loads(path.read_text())["path"])
        counts = dict.fromkeys(markers, 0)
        raw = transcript.read_bytes()
        lines = raw.splitlines()
        if not raw.endswith(b"\n"):
            lines = lines[:-1]
        for line in lines:
            row = json.loads(line)
            if row.get("type") != "user":
                continue
            text = json.dumps(row.get("message", {}).get("content", ""), ensure_ascii=False)
            for marker in markers:
                if marker in text:
                    counts[marker] += 1
        return counts

    def scenario(self, kind, number):
        self.stop_host()
        for name in ("fault-used", "launch-blocked", "fault.json", "cleanup-fail-stop", "cleanup-fail-rm"):
            (self.root / name).unlink(missing_ok=True)
        if kind in ("slow", "restart-late", "restart-before", "copy-cleanup"):
            (self.root / "fault.json").write_text(json.dumps({"kind": kind, "delay": self.args.delay}))
        began = time.time()
        self.maximum = 0
        self.maximum_processes = 0
        markers = [f"WAKE_{kind}_{number}_{i}_END" for i in range(self.args.concurrency)]
        # The control connection exercises its internal one-second retry loop.
        with concurrent.futures.ThreadPoolExecutor(max_workers=self.args.concurrency + 1) as pool:
            control = pool.submit(self.command, [self.kernel, "turn-facts", self.resident, "--compact"], 360)
            if kind in ("slow", "restart-late", "restart-before", "copy-cleanup"):
                marker_path = self.root / ("cleanup-fail-stop" if kind == "copy-cleanup" else "launch-blocked")
                until = time.monotonic() + 60
                while not marker_path.exists() and time.monotonic() < until:
                    time.sleep(.1)
                if not marker_path.exists():
                    raise RuntimeError("fault did not reach a CLI wake")
                if kind == "restart-before":
                    os.kill(int((self.root / "launch-blocked").read_text()), signal.SIGKILL)
                if kind.startswith("restart-") or kind == "copy-cleanup":
                    self.restart()
            futures = [pool.submit(self.f, "talk", self.resident, marker, timeout=360) for marker in markers]
            outcomes = []
            for marker, future in zip(markers, futures):
                try:
                    text = future.result()
                    (self.root / "evidence" / f"{marker}.txt").write_text(text)
                    outcomes.append({"marker": marker, "ok": True})
                except Exception as error:
                    outcomes.append({"marker": marker, "ok": False, "error": str(error)})
            result = control.result()
            (self.root / "evidence" / f"control-{kind}-{number}.txt").write_text(result.stdout + result.stderr)
        time.sleep(2)
        rows = [row for row in self.agents() if row.get("kind") == "background"]
        counts = self.transcript_counts(markers)
        deadline = time.monotonic() + (330 if kind == "restart-before" else 90)
        while any(value == 0 for value in counts.values()) and time.monotonic() < deadline:
            time.sleep(.5)
            counts = self.transcript_counts(markers)
        rows = [row for row in self.agents() if row.get("kind") == "background"]
        result = {"scenario": kind, "round": number, "seconds": time.time() - began, "sessions": [{"id": row["id"], "pid": row.get("pid"), "state": row.get("state")} for row in rows], "counts": counts, "outcomes": outcomes, "control_code": result.returncode}
        events = [json.loads(line) for line in (self.root / "cli.jsonl").read_text().splitlines()]
        copies = [event for event in events if event["at"] >= began and "started a copy" in event.get("err", "")]
        result["copies_created"] = len(copies)
        result["maximum_session_jobs"] = self.maximum
        result["maximum_listed_processes"] = self.maximum_processes
        cleanup_ok = True
        if kind == "copy-cleanup":
            deadline = time.monotonic() + 60
            kept = []
            while time.monotonic() < deadline:
                with sqlite3.connect(self.root / "data/world/node.sqlite3") as database:
                    kept = [json.loads(row[0]) for row in database.execute("SELECT body FROM claude_copies")]
                if kept and all(row["removed"] for row in kept):
                    break
                time.sleep(.5)
            excluded = self.command([self.kernel, "claude-session-directory", str(self.root / "data/world"), kept[-1]["short"], "--previous", self.session, "--cwd", str(self.root / "data/world/outside")]) if kept else None
            cleanup_ok = bool(kept) and all(row["removed"] for row in kept) and excluded.returncode != 0 and "excluded copy" in excluded.stderr
            rows = [row for row in self.agents() if row.get("kind") == "background"]
            result["cleanup_records"] = kept
            result["late_pairing_refused"] = cleanup_ok
        copies_ok = len(copies) == 1 if kind == "copy-cleanup" else not copies
        workers_ok = (self.maximum <= 2 and self.maximum_processes <= 2) if kind == "copy-cleanup" else (self.maximum == 1 and self.maximum_processes <= 1)
        result["sessions"] = [{"id": row["id"], "pid": row.get("pid"), "state": row.get("state")} for row in rows]
        result["passed"] = copies_ok and workers_ok and cleanup_ok and len(rows) == 1 and all(value == 1 for value in counts.values()) and all(outcome["ok"] for outcome in outcomes)
        result["seconds"] = time.time() - began
        self.samples.append(result)
        (self.root / "results.json").write_text(json.dumps(self.samples, indent=2))
        print(json.dumps(result), flush=True)
        return result["passed"]

    def raw(self):
        results = []
        for number in range(self.args.rounds):
            with concurrent.futures.ThreadPoolExecutor(max_workers=self.args.concurrency) as pool:
                futures = [pool.submit(self.command, [str(self.root / "bin/claude"), "--resume", self.session, "--bg", "--", f"RAW_{number}_{i}"], 120) for i in range(self.args.concurrency)]
                for i, future in enumerate(futures):
                    result = future.result()
                    (self.root / "evidence" / f"raw-{number}-{i}.txt").write_text(result.stdout + result.stderr)
                    results.append({"round": number, "code": result.returncode, "copy": "started a copy" in result.stderr})
        (self.root / "raw-results.json").write_text(json.dumps(results, indent=2))
        print(json.dumps(results), flush=True)
        return len(results) == self.args.rounds * self.args.concurrency and all(row["code"] == 0 and row["copy"] for row in results)

    def close(self):
        self.monitoring = False
        if (self.root / "cli.jsonl").exists():
            events = [json.loads(line) for line in (self.root / "cli.jsonl").read_text().splitlines()]
            abandoned = {event["pid"] for event in events if event["event"] == "delayed" and event.get("fault", {}).get("kind") == "restart-before"}
            resumed = {event["pid"] for event in events if event["event"] == "result" and event.get("code") == 0 and "--resume" in event.get("args", []) and "started a copy" not in event.get("err", "")}
            started = [event["at"] for event in events if event["event"] == "call" and "--resume" in event.get("args", []) and event["pid"] in resumed and event["pid"] not in abandoned]
            lines = (self.root / "data/world/node.log").read_text().splitlines()
            activations = [datetime.datetime.fromisoformat(line.split(" ")[0].replace("Z", "+00:00")).timestamp() for line in lines if "host present" in line]
            self.samples.append({"kind": "activation-latencies", "seconds": [min(at - began for at in activations if at >= began) for began in started if any(at >= began for at in activations)]})
        (self.root / "summary.json").write_text(json.dumps({"samples": self.samples, "maximum_session_jobs": self.maximum, "maximum_listed_processes": self.maximum_processes}, indent=2))
        self.server.shutdown()
        if self.node and self.node.poll() is None:
            self.node.kill()
            self.node.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--kernel", required=True)
    parser.add_argument("--claude", required=True)
    parser.add_argument("--root", required=True)
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--delay", type=float, default=20)
    parser.add_argument("--scenarios", default="concurrent,slow,restart-late,restart-before")
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("run as root inside a disposable Linux environment")
    if not args.inside:
        if Path(args.root).exists():
            parser.error("the experiment root must not exist")
        command = ["unshare", "--net", "--mount", "--propagation", "private", sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], "--inside"]
        return subprocess.call(command)
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    if subprocess.check_output(["ip", "route"]).strip():
        parser.error("the experiment namespace must have no external routes")
    lab = None
    passed = True
    try:
        lab = Lab(args)
        lab.open()
        for scenario in args.scenarios.split(","):
            if scenario == "raw":
                passed &= lab.raw()
            else:
                for number in range(args.rounds):
                    passed &= lab.scenario(scenario, number)
    finally:
        if lab is not None:
            lab.close()
        # Every remaining CLI/daemon belongs to this private network namespace.
        namespace = os.readlink("/proc/self/ns/net")
        for path in Path("/proc").iterdir():
            if path.name.isdigit() and int(path.name) != os.getpid():
                try:
                    if os.readlink(path / "ns/net") == namespace:
                        os.kill(int(path.name), signal.SIGCONT)
                        os.kill(int(path.name), signal.SIGTERM)
                except OSError:
                    pass
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
