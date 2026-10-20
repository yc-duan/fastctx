"""Lifecycle checks run only inside the disposable CentOS 7 guest."""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time

import smoke

BASE = pathlib.Path("/opt/fastctx-test")
ROOT = pathlib.Path("/var/tmp/fastctx-old-linux")
PROGRAM = BASE / "fastctx"
PREVIOUS = BASE / "previous"
PASSWORD = "Local-Lifecycle-Only-7"


def check(label, ok, detail=""):
    if not smoke.check(label, ok, detail):
        raise RuntimeError(label)


def wait(label, probe, seconds=120):
    if not smoke.until(label, probe, seconds):
        raise RuntimeError(label)


def lab_at(root, program=PROGRAM, fresh=False):
    if fresh:
        root.mkdir(parents=True)
        return smoke.Lab(program, root)
    lab = smoke.Lab.__new__(smoke.Lab)
    lab.root, lab.program, lab.owned = root, root / "fastctx", []
    return lab


def status(lab, who, kind="node"):
    directory = lab.root / who
    if kind == "hub":
        directory /= "hub"
    code, text = lab.run(who, "service", kind, "status", "--dir", str(directory),
                         "--executable", str(lab.root / who / "bin/fastctx"))
    if code:
        raise RuntimeError(text)
    return json.loads(text)


def identify(program, env):
    done = subprocess.run([str(program), "renew", "identify"], env=env, capture_output=True, text=True, timeout=120)
    if done.returncode:
        raise RuntimeError(done.stdout + done.stderr)
    return json.loads(done.stdout)


def owned(lab, who):
    service = status(lab, who)
    pid = smoke.service_process(service)
    code, text = lab.run(who, "renew", "status", "--dir", str(lab.root / who))
    state = json.loads(text) if code == 0 else {}
    return pid and (state.get("node") or {}).get("process", {}).get("pid") == pid and smoke.node_processes(lab.root, who) == {pid}


def exchange(lab):
    source = lab.root / "source.txt"
    source.write_text("old Linux: unchanged bytes\n", encoding="utf-8")
    remote, back = lab.root / "remote.txt", lab.root / "back.txt"
    for args, target in [((str(source), "beta:" + str(remote)), remote),
                         (("beta:" + str(remote), str(back)), back)]:
        code, out = lab.run("alpha", "copy", *args)
        check("copy preserves the bytes", code == 0 and target.read_bytes() == source.read_bytes(), out)
    before = len(smoke.RESULTS)
    smoke.hands(lab, "4.2")
    check("MCP run and on both work", all(smoke.RESULTS[before:]))


def setup():
    check("the guest really runs CentOS 7 and Linux 3.10", "release 7." in pathlib.Path("/etc/centos-release").read_text()
          and os.uname().release.startswith("3.10."))
    lab = lab_at(ROOT / "network", fresh=True)
    port = smoke.free_port()
    code, out = lab.run("alpha", "hub", "init", "--name", "old-linux", "--admin", "root",
                        "--public", f"127.0.0.1:{port}", "--listen", f"127.0.0.1:{port}",
                        "--dir", str(lab.root / "alpha/hub"), "--service", stdin=PASSWORD + "\n", timeout=300)
    check("the hub is installed as a boot service", code == 0, out)
    code, out = lab.run("alpha", "login", f"127.0.0.1:{port}", "root", "--name", "alpha", stdin=PASSWORD + "\n")
    check("alpha joins", code == 0, out)
    code, out = lab.run("alpha", "token", "new", "root", "--for", "server", "--name", "beta")
    check("a server installation line is issued", code == 0, out)
    line = out.splitlines()[0]
    done = subprocess.run(["/bin/sh", "-c", line], env=lab.env("beta"), capture_output=True, text=True, timeout=300)
    check("the original CentOS curl installs beta from the hub", done.returncode == 0, done.stdout + done.stderr)
    wait("alpha sees beta", lambda: lab.online("alpha", "beta"))
    wait("beta sees alpha", lambda: lab.online("beta", "alpha"))
    for who in ("alpha", "beta"):
        wait(f"{who}'s service owns its only node", lambda who=who: owned(lab, who))
    exchange(lab)
    state = {"boot_id": pathlib.Path("/proc/sys/kernel/random/boot_id").read_text(),
             "services": {"hub": status(lab, "alpha", "hub"), "alpha": status(lab, "alpha"), "beta": status(lab, "beta")}}
    (ROOT / "boot.json").write_text(json.dumps(state))


def boot():
    state = json.loads((ROOT / "boot.json").read_text())
    check("the guest has rebooted", pathlib.Path("/proc/sys/kernel/random/boot_id").read_text() != state["boot_id"])
    # No Fastctx client may start a missing node before these checks.
    for name, service in state["services"].items():
        done = subprocess.run(["systemctl", "is-active", service["name"]], capture_output=True, text=True)
        check(f"{name} started at boot without a client", done.returncode == 0 and done.stdout.strip() == "active", done.stdout + done.stderr)
    lab = lab_at(ROOT / "network")
    wait("the booted nodes reconnect", lambda: lab.online("alpha", "beta"))
    for who in ("alpha", "beta"):
        wait(f"{who}'s boot node belongs to its service", lambda who=who: owned(lab, who))
    exchange(lab)


def renewal():
    lab = lab_at(ROOT / "renewal", PREVIOUS, fresh=True)
    old = identify(PREVIOUS, lab.env("alpha"))
    new = identify(PROGRAM, lab.env("alpha"))
    check("renewal has two different images", old["digest"] != new["digest"], (old, new))
    # The baseline's connection monitor is broken on 3.10. It can still keep
    # a local World and hub, which the replacement must adopt before login.
    port = smoke.free_port()
    code, out = lab.run("alpha", "hub", "init", "--name", "renewal", "--admin", "root",
                        "--public", f"127.0.0.1:{port}", "--listen", f"127.0.0.1:{port}",
                        "--dir", str(lab.root / "alpha/hub"), "--service", stdin=PASSWORD + "\n", timeout=300)
    check("the previous hub is installed", code == 0, out)
    code, out = lab.run("alpha", "nodes")
    check("the previous local node starts", code == 0, out)
    installed = lab.root / "alpha/bin/fastctx"
    code, out = lab.run("alpha", "service", "node", "install", "--dir", str(lab.root / "alpha"), "--executable", str(installed), "--uid", "0")
    check("the previous node is kept by a service", code == 0, out)
    wait("the previous service has taken over its only node", lambda: owned(lab, "alpha"))
    done = subprocess.run([str(PROGRAM), "renew", "stage", "--dir", str(lab.root / "alpha"), "--source", str(PROGRAM)],
                          env=lab.env("alpha"), capture_output=True, text=True, timeout=300)
    check("the release stages itself over the previous image", done.returncode == 0, done.stdout + done.stderr)
    lab.program = PROGRAM

    def complete():
        code, text = lab.run("alpha", "renew", "status", "--dir", str(lab.root / "alpha"))
        state = json.loads(text) if code == 0 else {}
        if state.get("phase") in ("failed", "awaiting_administrator"):
            raise RuntimeError(text)
        return state.get("phase") == "complete" and not state.get("error") and (state.get("node") or {}).get("build", {}).get("digest") == new["digest"]

    wait("renewal completes without an administrator", complete, 240)
    for kind in ("node", "hub"):
        def running(kind=kind):
            pid = smoke.service_process(status(lab, "alpha", kind))
            return pid and hashlib.sha256(pathlib.Path(f"/proc/{pid}/exe").read_bytes()).hexdigest() == new["digest"]
        wait(f"the {kind} service runs the new inode", running)
    check("the stable image is the new image", hashlib.sha256(installed.read_bytes()).hexdigest() == new["digest"])
    code, out = lab.run("alpha", "login", f"127.0.0.1:{port}", "root", "--name", "renewed", stdin=PASSWORD + "\n")
    check("the renewed node can log in", code == 0, out)
    code, out = lab.run("alpha", "uninstall", "run", "--everything", "--json", timeout=300)
    check("the renewed World uninstalls", code == 0 and not installed.exists(), out)


def cleanup():
    lab = lab_at(ROOT / "network")
    for who in ("beta", "alpha"):
        code, out = lab.run(who, "uninstall", "run", "--everything", "--json", timeout=300)
        check(f"{who} uninstalls", code == 0 and not (lab.root / who / "bin").exists(), out)
    units = subprocess.run(["systemctl", "list-unit-files", "--no-legend", "fastctx-service-*"], capture_output=True, text=True)
    check("no boot units remain", not any(line.startswith("fastctx-service-") for line in units.stdout.splitlines()), units.stdout)
    listing = subprocess.run(["ps", "-e", "-o", "args="], capture_output=True, text=True).stdout
    check("no serving processes remain", not any(str(ROOT) in line and (" node " in line or " hub serve " in line) for line in listing.splitlines()), listing)


def diagnostics():
    subprocess.run(["journalctl", "--no-pager", "-n", "300"], timeout=30)
    for file in ROOT.rglob("*.log"):
        print(f"== {file}", flush=True)
        print(file.read_text(errors="replace")[-16000:], flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("setup", "boot", "renewal", "cleanup", "diagnostics"))
    args = parser.parse_args()
    globals()[args.phase]()
    if args.phase != "diagnostics":
        print(f"{sum(smoke.RESULTS)}/{len(smoke.RESULTS)} passed", flush=True)
    return 0 if all(smoke.RESULTS) else 1


if __name__ == "__main__":
    sys.exit(main())
