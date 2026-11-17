"""Run smoke under seccomp, including units started independently by systemd.

Run as root on a disposable test host. Only units belonging to this run's
private root are wrapped; the generator and its output disappear afterwards.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wrapper", type=Path)
    parser.add_argument("program", type=Path)
    parser.add_argument("--root", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        raise SystemExit("oldkernel smoke must run as root: seccomp prevents sudo escalation")
    root = args.root.resolve()
    root.mkdir(parents=True)
    here = Path(__file__).resolve().parent
    settings = root / "generator.json"
    settings.write_text(json.dumps({"root": str(root), "wrapper": str(args.wrapper.resolve())}))
    generators = generators_dir()
    generators.mkdir(parents=True, exist_ok=True)
    generator = generators / ("fastctx-oldkernel-" + str(os.getpid()))
    if generator.exists():
        raise RuntimeError("the test generator already exists")
    python = "/usr/bin/python3" if Path("/usr/bin/python3").exists() else sys.executable
    generator.write_text("#!/bin/sh\nexec " + shlex.quote(python) + " " + shlex.quote(str(here / "oldkernel-services.py"))
                         + " " + shlex.quote(str(settings)) + ' "$@"\n')
    generator.chmod(0o755)
    try:
        subprocess.run(["systemctl", "daemon-reload"], check=True)
        env = dict(os.environ, RUNNER_TEMP=str(root), TMPDIR=str(root), FASTCTX_HOME=str(root / "unused"))
        code = subprocess.run([str(args.wrapper.resolve()), "314", sys.executable, str(here / "smoke.py"),
                               str(args.program.resolve()), "--service", "--expect-seccomp"], env=env).returncode
        if code:
            diagnose(root)
        else:
            unread(root)
        return code
    finally:
        generator.unlink()
        subprocess.run(["systemctl", "daemon-reload"], check=True)


def generators_dir():
    """Where systemd finds this run's generator: under /run, which goes with
    the machine, unless /run lets no program run from it. Some runner images
    mount it so, and systemd then fails every generator there with
    'Permission denied', leaving the services unwrapped; /etc is searched for
    generators as well, and the generator is taken away when the run ends."""
    if os.statvfs("/run").f_flag & os.ST_NOEXEC:
        return Path("/etc/systemd/system-generators")
    return Path("/run/systemd/system-generators")


def unread(root):
    """Units the generator could not read, said even when the rest passed:
    one such unit once left every other unwrapped."""
    log = root / "generator.log"
    lines = [line for line in (log.read_text().splitlines() if log.exists() else []) if "Error:" in line]
    if lines:
        print("--- old-kernel generator could not read\n" + "\n".join(sorted(set(lines))))


def diagnose(root):
    """What the generator made of the units, and what systemd holds of it."""
    print("--- old-kernel generator")
    logs = [log for log in (root / "generator.log", Path("/run/systemd/generator/fastctx-oldkernel.log")) if log.exists()]
    print("".join(log.read_text() for log in logs) if logs else "(the generator left no log: it did not run, or could not write it)")
    for command in (["systemctl", "--version"],
                    ["sh", "-c", "findmnt -no OPTIONS /run"],
                    ["sh", "-c", "ls -la /run/systemd/system-generators /etc/systemd/system-generators /run/systemd/generator 2>&1 | grep -i -e fastctx -e total"],
                    ["sh", "-c", "cat /run/systemd/generator/fastctx-service-*.service.d/oldkernel.conf 2>&1"],
                    ["sh", "-c", "journalctl -b --no-pager -o cat 2>/dev/null | grep -i -e generator -e oldkernel | tail -n 20"]):
        result = subprocess.run(command, capture_output=True, text=True)
        print("$ " + " ".join(command) + "\n" + result.stdout + result.stderr)


if __name__ == "__main__":
    sys.exit(main())
