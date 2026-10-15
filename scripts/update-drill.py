"""The whole of an application update on a Windows computer, as a release's check: World installed
from its installer, a language chosen and automatic updates and test releases switched on, as by
a user of the interface, and a stand-in for GitHub on this computer offering a later version,
which is the same installer signed for that version. The node finds it; the interface has it
brought and checked, installs it with the official updater once nothing is under way, and
Fastctx opens again.

    python scripts/update-drill.py INSTALLER LATER-INSTALLER LATER-VERSION
"""
import json
import os
import subprocess
import sys
import threading
import time
import http.server
from pathlib import Path

RESULTS = []


def check(label, ok, detail=""):
    RESULTS.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + label + ("" if ok or not detail else f"\n     {str(detail)[-6000:]}"), flush=True)
    return ok


def applications():
    """Each running fastctx-app by its process id, with its command line."""
    listed = subprocess.run(["powershell", "-NoProfile", "-Command",
                             "Get-CimInstance Win32_Process -Filter \"Name = 'fastctx-app.exe'\" | "
                             "ForEach-Object { '{0}|{1}' -f $_.ProcessId, $_.CommandLine }"],
                            capture_output=True, text=True).stdout.splitlines()
    return {int(pid): line for pid, _, line in (entry.partition("|") for entry in listed) if pid.strip().isdigit()}


def opened(line):
    """Whether a fastctx-app is the application itself: the installer also runs it with an argument
    of its own for each of its steps."""
    return " --" not in line


def evidence(updates):
    """What the World kept of its looking and bringing, and what its node said."""
    said = [str(path) for path in sorted(updates.rglob("*"))]
    for name in ("looked.json", "app.json"):
        if (updates / name).is_file():
            said.append(f"{name}: {(updates / name).read_text(encoding='utf-8', errors='replace')[:800]}")
    log = updates.parent / "world" / "node.log"
    if log.is_file():
        said.append("node.log: " + " | ".join(log.read_text(encoding="utf-8", errors="replace").splitlines()[-25:]))
    return "\n     ".join(said)


def serve(directory, releases):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            if self.path.startswith("/releases"):
                body = json.dumps(releases).encode()
            else:
                file = directory / self.path.removeprefix("/download/")
                if not file.is_file():
                    self.send_response(404)
                    self.end_headers()
                    return
                body = file.read_bytes()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


def main():
    installer, later, version = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(), sys.argv[3]
    site = later.parent
    target = "windows-x86_64-nsis"
    releases = []
    port = serve(site, releases)
    manifest = {"version": version, "notes": "A drill's later version.",
                "platforms": {target: {"url": f"http://127.0.0.1:{port}/download/{later.name}",
                                       "signature": Path(str(later) + ".sig").read_text().strip(),
                                       "size": later.stat().st_size}}}
    (site / "latest.json").write_text(json.dumps(manifest))
    releases.append({"tag_name": "v" + version, "draft": False, "prerelease": True,
                     "assets": [{"name": "latest.json", "browser_download_url": f"http://127.0.0.1:{port}/download/latest.json"}]})

    subprocess.run([str(installer), "/S"], check=True)
    app = Path(os.environ["LOCALAPPDATA"]) / "Fastctx" / "fastctx-app.exe"
    check("the installer put Fastctx in place", app.is_file(), app)
    updates = Path.home() / ".fastctx" / "updates"
    updates.mkdir(parents=True, exist_ok=True)
    (updates / "settings.json").write_text(json.dumps({"auto": True, "beta": True, "relays": True}))
    # Until a language is chosen the interface shows only the page that asks for one; the rest of
    # it, which takes the updates, loads after.
    world = Path.home() / ".fastctx" / "world"
    world.mkdir(parents=True, exist_ok=True)
    (world / "features.json").write_text(json.dumps({"language": "en"}))
    env = {key: value for key, value in os.environ.items() if not key.upper().startswith("CLAUDE")}
    env["FASTCTX_UPDATE_RELEASES"] = f"http://127.0.0.1:{port}/releases"
    first = subprocess.Popen([str(app)], env=env)
    package = updates / "packages" / f"v{version}" / target
    began = time.monotonic()
    while time.monotonic() - began < 420 and not package.is_file():
        time.sleep(2)
    check("the node found the later version, brought its package and checked it", package.is_file(),
          evidence(updates))
    relaunched = {}
    while time.monotonic() - began < 600:
        if first.poll() is not None:
            relaunched = {pid: line for pid, line in applications().items() if pid != first.pid and opened(line)}
            if relaunched:
                break
        time.sleep(2)
    check("the interface installed it, and Fastctx closed and opened again", first.poll() is not None and relaunched,
          (first.poll(), applications()))
    marker = Path(os.environ["APPDATA"]) / "dev.fastctx.world" / "updated.json"
    reopened = time.monotonic()
    while marker.exists() and time.monotonic() - reopened < 60:
        time.sleep(1)
    check("the new start read what the update left for it", not marker.exists(), (marker, applications()))
    for pid in applications():
        subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)
    passed = sum(RESULTS)
    print(f"\n{passed}/{len(RESULTS)} passed")
    return 0 if passed == len(RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
