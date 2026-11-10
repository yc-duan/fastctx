"""The worst-case soak (`specs/world-engine.md`, 验证的设施).

A real node held to a small server: two cores, two gigabytes, and a disk of its own that does a
hundred operations a second, slowed further and hung now and then. Its World and its Codex home
are on that disk, a sparse ext4 file on a loop device; it runs in a slice of its own, while its
hub, the patrol machine, the drill and the model mock run on the other cores. The drill
(`crates/fastctx-hub/tests/soak_drill`) loads it, makes things go wrong, and holds what it measured
to the foundation's numbers.

    sudo python3 scripts/soak.py --kernel K --drill D --codex C [--minutes 30] [--scale ci|full] [--root DIR]

Linux and root only. Nothing goes to any model vendor: the model is a mock on this machine. A
private mount namespace gives the World's processes a PATH with Codex and without Claude, and a
home of their own, leaving the machine's as they are.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from model_mock import Mock  # noqa: E402

# What a drill of each size holds: conversations opened beforehand, the turns each is given and the
# minutes they are given in, people at the node's interface, its interfaces watching, and phones.
SCALES = {
    "ci": {"conversations": 120, "turns": 4, "seeding": 12, "workers": 8, "watchers": 6, "devices": 24},
    "full": {"conversations": 300, "turns": 8, "seeding": 40, "workers": 12, "watchers": 12, "devices": 48},
}
# A small server's disk, in operations a second.
IOPS = 100
NODE_SLICE = "fastctx-soak-node.slice"
REST_SLICE = "fastctx-soak-rest.slice"


def arguments():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    for name in ("kernel", "drill", "codex"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--minutes", type=int, default=30)
    parser.add_argument("--scale", choices=sorted(SCALES), default="ci")
    parser.add_argument("--root", type=Path)
    parser.add_argument("--seed", type=int)
    return parser.parse_args()


def run(*command, **options):
    return subprocess.run([str(part) for part in command], check=True, **options)


def world_path(codex_dir):
    """The PATH the World's processes see: Codex's directory first, and nothing that holds Claude."""
    keep = [str(codex_dir)]
    for entry in "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".split(":"):
        if entry not in keep and not any((Path(entry) / name).exists() for name in ("claude", "claude.exe")):
            keep.append(entry)
    path = ":".join(keep)
    if shutil.which("claude", path=path):
        raise RuntimeError("the World's PATH must not hold Claude")
    return path


class Disk:
    """A disk of the node's own: a sparse ext4 file on a loop device, mounted under the root."""

    def __init__(self, root, gib=6, reserved_percent=5):
        self.image = root / "disk.img"
        self.mount = root / "disk"
        self.device = None
        self.gib = gib
        self.reserved_percent = reserved_percent

    def __enter__(self):
        self.mount.mkdir(parents=True, exist_ok=True)
        run("truncate", "-s", f"{self.gib}G", self.image)
        run("mkfs.ext4", "-q", "-F", "-m", str(self.reserved_percent), self.image)
        self.device = subprocess.run(["losetup", "--find", "--show", str(self.image)], check=True,
                                     capture_output=True, text=True).stdout.strip()
        run("mount", self.device, self.mount)
        number = os.stat(self.device).st_rdev
        self.number = f"{os.major(number)}:{os.minor(number)}"
        return self

    def __exit__(self, *_):
        subprocess.run(["umount", "--lazy", str(self.mount)], check=False)
        if self.device:
            subprocess.run(["losetup", "--detach", self.device], check=False)
        self.image.unlink(missing_ok=True)


def cpus():
    """The cores the node is held to, and those everything else runs on."""
    count = os.cpu_count() or 1
    if count < 4:
        print(f"soak: only {count} cores; the drill shares the node's", flush=True)
        return "0-1", f"0-{count - 1}"
    return "0-1", f"2-{count - 1}"


def main():
    args = arguments()
    if os.name != "posix" or os.geteuid() != 0:
        raise SystemExit("the soak runs on Linux as root, for its disk and its slice")
    for name in ("kernel", "drill", "codex"):
        path = getattr(args, name).resolve(strict=True)
        if not path.is_file():
            raise SystemExit(f"{name} is not a file")
        setattr(args, name, path)
    root = args.root.resolve() if args.root else Path(tempfile.mkdtemp(prefix="soak-"))
    root.mkdir(parents=True, exist_ok=True)
    base = root / "drill"
    if base.exists():
        shutil.rmtree(base)
    base.mkdir()
    node_cpus, rest_cpus = cpus()
    mock = Mock(keep=200)
    passed = False
    with Disk(root) as disk:
        # The node's user's home is on its disk, its World where a release keeps it there and
        # Codex's home beside it, as on a server: so whatever its Codex starts of the World's finds
        # this World, as it would there.
        home = disk.mount / "home"
        world = home / ".fastctx"
        codex_home = home / ".codex"
        try:
            codex_home.mkdir(parents=True)
            (codex_home / "config.toml").write_text(
                f'model = "mock-model"\nmodel_provider = "mock"\n\n[model_providers.mock]\nname = "mock"\n'
                f'base_url = "{mock.base_url}"\nwire_api = "responses"\nrequires_openai_auth = false\n'
                f'request_max_retries = 0\nstream_max_retries = 0\nstream_idle_timeout_ms = 15000\n',
                encoding="utf-8")
            path = world_path(args.codex.parent)
            config = dict(SCALES[args.scale], base=str(base), home=str(home), world=str(world), codex_home=str(codex_home),
                          kernel=str(args.kernel), path=path, minutes=args.minutes, slice=NODE_SLICE,
                          disk=disk.device, number=disk.number, iops=IOPS)
            (base / "config.json").write_text(json.dumps(config, indent=2))
            (base / "user").mkdir()
            passwd = Path("/etc/passwd").read_text().splitlines()
            for index, line in enumerate(passwd):
                fields = line.split(":")
                if fields[2] == "0":
                    fields[5] = str(home)
                    passwd[index] = ":".join(fields)
            (base / "passwd").write_text("\n".join(passwd) + "\n")
            (base / "environment").write_text(f"PATH={path}\nLANG=C.UTF-8\n")
            environment = {key: value for key, value in os.environ.items() if not key.upper().startswith("CLAUDE")}
            environment.update(PATH=path + ":" + os.environ.get("PATH", ""), RUST_BACKTRACE="1")
            if args.seed is not None:
                environment["SOAK_SEED"] = str(args.seed)
            inside = (f"mount --bind '{base / 'passwd'}' /etc/passwd && "
                      f"mount --bind '{base / 'environment'}' /etc/environment && "
                      f"exec '{args.drill}' '{base / 'config.json'}'")
            print(f"soak: the node on cores {node_cpus}, everything else on {rest_cpus}; root {root}", flush=True)
            # What its clients say as they go (each phone's every reconnect) is kept apart from
            # what the drill tells of itself and its judgement.
            with open(root / "drill.err", "wb") as stderr:
                subprocess.run(["systemd-run", "--scope", "--quiet", "--collect", f"--slice={REST_SLICE}",
                                "-p", f"AllowedCPUs={rest_cpus}", "--",
                                "unshare", "--mount", "--propagation", "private", "bash", "-c", inside],
                               check=True, env=environment, stderr=stderr,
                               timeout=(args.minutes + SCALES[args.scale]["seeding"] + 45) * 60)
            passed = True
        finally:
            # What the node said, off its disk before the disk goes.
            log = world / "world" / "node.log"
            if log.exists():
                shutil.copy2(log, root / "node.log")
                if not passed:
                    print("NODE LOG (last lines)", *log.read_text(errors="replace").splitlines()[-60:], sep="\n  ",
                          flush=True)
            errors = root / "drill.err"
            if not passed and errors.exists():
                said = [line for line in errors.read_text(errors="replace").splitlines()
                        if not line.startswith("World device: connected")]
                print("DRILL STDERR (last lines)", *said[-40:], sep="\n  ", flush=True)
            for unit in (NODE_SLICE, REST_SLICE):
                subprocess.run(["systemctl", "stop", unit], check=False, capture_output=True)
                subprocess.run(["systemctl", "revert", unit], check=False, capture_output=True)
            # Beside the rest, readable by whoever collects them after the root that wrote them.
            for kept, name in ((base / "soak-result.json", "soak-result.json"), (base / "hub" / "hub.log", "hub.log"),
                               (base / "soak-node.out", "soak-node.out")):
                if kept.exists():
                    shutil.copy2(kept, root / name)
            for name in ("soak-result.json", "hub.log", "node.log", "drill.err", "soak-node.out"):
                if (root / name).exists():
                    os.chmod(root / name, 0o644)
            mock.close()
            print("ARTIFACTS", root, flush=True)


if __name__ == "__main__":
    main()
