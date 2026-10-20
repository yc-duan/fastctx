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
    generators = Path("/run/systemd/system-generators")
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
        return subprocess.run([str(args.wrapper.resolve()), "314", sys.executable, str(here / "smoke.py"),
                               str(args.program.resolve()), "--service", "--expect-seccomp"], env=env).returncode
    finally:
        generator.unlink()
        subprocess.run(["systemctl", "daemon-reload"], check=True)


if __name__ == "__main__":
    sys.exit(main())
