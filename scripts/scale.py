#!/usr/bin/env python3
"""GB-scale source fixtures and real-node read measurements, without model calls.

Generate and verify on any Python 3.11+ host. Run on Linux (including WSL),
inside a private loopback-only network and PID namespace. See scripts/scale/README.md.
"""
import argparse
import copy
import json
import math
import socket
import tempfile
import os
from pathlib import Path
try:
    import resource
except ImportError:
    resource = None
import shutil
import signal
import subprocess
import sys
import time

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent / "scale"))
from fixture import GIB, PROFILES, SEED, generate
from calibration import verify
from manifest import read_manifest


def lower_priority():
    if hasattr(os, "nice"):
        os.nice(19)
    elif os.name == "nt":
        import ctypes
        ctypes.windll.kernel32.SetPriorityClass(ctypes.windll.kernel32.GetCurrentProcess(), 0x00004000)
    if hasattr(os, "sched_getaffinity"):
        available = sorted(os.sched_getaffinity(0))
        os.sched_setaffinity(0, available[:2])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    create = sub.add_parser("generate")
    create.add_argument("--root", type=Path, required=True)
    create.add_argument("--profile", choices=PROFILES, default="quick")
    create.add_argument("--history", type=int, choices=(1, 10), default=1)
    create.add_argument("--seed", type=int, default=SEED)
    create.add_argument("--max-gib", type=int, default=36)
    check = sub.add_parser("verify")
    check.add_argument("--root", type=Path, required=True)
    run = sub.add_parser("run")
    run.add_argument("--fixture", type=Path, required=True)
    run.add_argument("--root", type=Path, required=True)
    run.add_argument("--kernel", type=Path, required=True)
    run.add_argument("--drill", type=Path, required=True)
    run.add_argument("--quiet-seconds", type=int, default=600)
    run.add_argument("--repeats", type=int, default=1)
    run.add_argument("--memory-gib", type=int, default=4)
    run.add_argument("--page-timeout-seconds", type=int, default=600)
    run.add_argument("--stop-after-first-screen", action="store_true")
    run.add_argument("--image-parent", type=Path, required=True)
    run.add_argument("--next-fixture", type=Path)
    run.add_argument("--next-root", type=Path)
    run.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    lower_priority()
    args.root = args.root.absolute()
    if args.command == "generate":
        settings = PROFILES[args.profile]
        main = settings["rows"] * args.history * 56 // 100
        hot = settings["hot_rows"] * args.history
        estimate = int((hot * 4096 + (main - hot) * settings["row_bytes"] + 100 * 1024**2) * 1.8)
        estimate += settings["output_bytes"] + settings["attachment_bytes"] + settings["long_line"]
        if estimate > args.max_gib * GIB:
            raise SystemExit(f"estimated {estimate / GIB:.2f} GiB exceeds --max-gib {args.max_gib}")
        parent = args.root.parent
        parent.mkdir(parents=True, exist_ok=True)
        if shutil.disk_usage(parent).free < estimate + GIB:
            raise SystemExit("not enough free space for the fixture plus 1 GiB reserve")
        began = time.monotonic()
        manifest = generate(args.root, args.profile, args.history, args.seed)
        calibration = verify(args.root)
        if manifest["total_bytes"] > args.max_gib * GIB:
            raise SystemExit("actual size exceeds requested budget")
        print(json.dumps(dict(seconds=time.monotonic()-began, calibration=calibration,
                              peak_rss_kib=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss if resource else None)))
        return
    if args.command == "verify":
        print(json.dumps(verify(args.root)))
        return
    if sys.platform != "linux" or os.geteuid() != 0:
        raise SystemExit("run needs Linux root for its isolated namespaces; use WSL on Windows")
    for name in ("fixture", "kernel", "drill"):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    if args.quiet_seconds < 0 or min(args.repeats, args.memory_gib, args.page_timeout_seconds) < 1:
        raise SystemExit("quiet duration must be nonnegative; repeats, memory and page timeout must be positive")
    if bool(args.next_fixture) != bool(args.next_root):
        raise SystemExit("--next-fixture and --next-root must be supplied together")
    if args.next_fixture:
        args.next_fixture = args.next_fixture.resolve(strict=True)
        args.next_root = args.next_root.absolute()
    if not args.inside:
        if args.root.exists() or (args.next_root and args.next_root.exists()):
            raise SystemExit("run root must not exist; each measurement uses a fresh world")
        args.root.mkdir(parents=True)
        if args.next_root:
            args.next_root.mkdir(parents=True)
        command = ["unshare", "--net", "--mount", "--pid", "--fork", "--kill-child", "--mount-proc",
                   sys.executable, str(Path(__file__).resolve()), "run", "--inside",
                   "--fixture", str(args.fixture), "--root", str(args.root), "--kernel", str(args.kernel),
                   "--drill", str(args.drill), "--quiet-seconds", str(args.quiet_seconds), "--repeats", str(args.repeats)]
        command.extend(["--memory-gib", str(args.memory_gib), "--image-parent", str(args.image_parent.absolute()),
                        "--page-timeout-seconds", str(args.page_timeout_seconds)])
        if args.stop_after_first_screen:
            command.append("--stop-after-first-screen")
        if args.next_fixture:
            command.extend(["--next-fixture", str(args.next_fixture), "--next-root", str(args.next_root)])
        command = ["systemd-run", "--scope", "--quiet", "--collect", f"--unit=fastctx-scale-{os.getpid()}",
                   "-p", f"MemoryMax={args.memory_gib}G", "-p", "MemorySwapMax=0", "--"] + command
        with (args.root / "drill.log").open("wb") as log:
            completed = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
        if args.next_root:
            shutil.copy2(args.root / "drill.log", args.next_root / "pair-launcher.log")
        print(f"exit={completed.returncode} result={args.root / 'scale-result.json'} log={args.root / 'drill.log'}")
        raise SystemExit(completed.returncode)
    group = next(line[3:] for line in Path("/proc/self/cgroup").read_text().splitlines() if line.startswith("0::"))
    group_root = Path("/sys/fs/cgroup") / group.lstrip("/")
    if (group_root / "memory.max").read_text().strip() != str(args.memory_gib * GIB):
        raise SystemExit("the experiment memory limit was not enforced")
    if (group_root / "memory.swap.max").read_text().strip() != "0":
        raise SystemExit("the experiment must not consume swap")
    from soak import Disk
    cases = [args]
    if args.next_fixture:
        following = copy.copy(args)
        following.fixture, following.root = args.next_fixture, args.next_root
        cases.append(following)
    estimates = []
    for case in cases:
        manifest = read_manifest(case.fixture)
        hot = next(entry["bytes"] for entry in manifest["files"] if entry["path"] == manifest["conversations"][0]["path"])
        kernel_bytes = args.kernel.stat().st_size
        # Installation temporarily stages a third program copy before client caches exist.
        programs_and_caches = max(3 * kernel_bytes, 2 * kernel_bytes + (args.repeats + 1) * hot)
        estimates.append(manifest["total_bytes"] + programs_and_caches + args.drill.stat().st_size + GIB // 2)
    gib = math.ceil(max(estimates) / GIB)
    args.image_parent.mkdir(parents=True, exist_ok=True)
    image_root = Path(tempfile.mkdtemp(prefix="fastctx-scale-", dir=args.image_parent))
    try:
        # Fail before copying gigabytes if this is another filesystem without Unix sockets.
        probe = socket.socket(socket.AF_UNIX)
        try:
            probe.bind(str(image_root / "socket-probe"))
        finally:
            probe.close()
            (image_root / "socket-probe").unlink(missing_ok=True)
        if shutil.disk_usage(image_root).free < (gib + 1) * GIB:
            raise SystemExit("not enough native-filesystem space for the experiment image")
        print(f"native experiment image: {gib} GiB at {image_root}", flush=True)
        outcomes = []
        with Disk(image_root, gib=gib, reserved_percent=0) as disk:
            for case in cases:
                base = disk.mount / "case"
                base.mkdir()
                print(f"case: {case.fixture} -> {case.root}", flush=True)
                try:
                    outcomes.append(inside(case, base))
                finally:
                    finish_case()
                    for relative in ("scale-config.json", "hub.log", "host/stdout.log", "host/world/node.log"):
                        path = base / relative
                        if path.exists():
                            shutil.copy2(path, case.root / relative.replace("/", "-"))
                    subprocess.run(["umount", "/etc/passwd"], check=False, capture_output=True)
                    shutil.rmtree(base)
    finally:
        shutil.rmtree(image_root)
    raise SystemExit(1 if any(outcomes) else 0)


def finish_case():
    """End only children of this private namespace, even if its driver was killed."""
    if os.getpid() != 1:
        raise RuntimeError("case cleanup requires the private namespace's init process")
    deadline = time.monotonic() + 10
    while True:
        children = [int(path.name) for path in Path("/proc").iterdir() if path.name.isdecimal() and path.name != "1"]
        if not children:
            return
        for pid in children:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        while True:
            try:
                if os.waitpid(-1, os.WNOHANG)[0] == 0:
                    break
            except ChildProcessError:
                break
        if time.monotonic() >= deadline:
            raise RuntimeError("experiment children did not exit; refusing to mix cases")
        time.sleep(0.05)


def inside(args, base):
    fixture = base / "fixture"
    shutil.copytree(args.fixture, fixture)
    print("native fixture calibration: " + json.dumps(verify(fixture)), flush=True)
    binaries = base / "programs"
    binaries.mkdir()
    kernel, drill = binaries / "fastctx", binaries / "placement-drill"
    shutil.copy2(args.kernel, kernel)
    shutil.copy2(args.drill, drill)
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    for name in ("user", "tmp", "bin", "codex", "claude"):
        (base / name).mkdir(exist_ok=True)
    passwd = Path("/etc/passwd").read_text().splitlines()
    for index, line in enumerate(passwd):
        fields = line.split(":")
        if fields[2] == str(os.getuid()):
            fields[5] = str(base / "user")
            passwd[index] = ":".join(fields)
    (base / "passwd").write_text("\n".join(passwd) + "\n")
    subprocess.run(["mount", "--bind", str(base / "passwd"), "/etc/passwd"], check=True)
    # Both the launcher and every child have private homes and no model executables on PATH.
    environment = dict(PATH="/usr/bin:/bin", HOME=str(base / "user"), LANG="C.UTF-8",
                       FASTCTX_HOME=str(base / "launcher"), TMPDIR=str(base / "tmp"),
                       CODEX_HOME=str(base / "codex"), CLAUDE_CONFIG_DIR=str(base / "claude"))
    for executable in ("claude", "codex"):
        if shutil.which(executable, path=environment["PATH"]):
            raise SystemExit(f"unexpected {executable} on isolated PATH")
    (base / "scale-config.json").write_text(json.dumps(dict(fixture=str(fixture), source_fixture=str(args.fixture), evidence=str(args.root),
        quiet_seconds=args.quiet_seconds, repeats=args.repeats, memory_gib=args.memory_gib,
        page_timeout_seconds=args.page_timeout_seconds, stop_after_first_screen=args.stop_after_first_screen)), encoding="utf-8")
    result = subprocess.run([str(drill), str(base), str(kernel), "--read-scale"],
                            env=environment, cwd=base / "user", check=False)
    return result.returncode


if __name__ == "__main__":
    main()
