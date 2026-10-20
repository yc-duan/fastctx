"""Run the release lifecycle in a disposable official CentOS 7 x86_64 VM.

Requires qemu-system-x86_64, qemu-img, genisoimage and OpenSSH on the host.
The image cache is immutable; every run uses a fresh copy-on-write overlay.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import socket
import subprocess
import sys
import tarfile
import time
import urllib.request

HERE = Path(__file__).resolve().parent
IMAGE = "https://cloud.centos.org/centos/7/images/CentOS-7-x86_64-GenericCloud-2009.qcow2c"
IMAGE_HASH = "7ba4513d96591496213a07bbe25e3eb643d61491924c8548a91815b420fd9827"
PYTHON = "https://github.com/astral-sh/python-build-standalone/releases/download/20261013/cpython-3.11.17%2B20261013-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz"
PYTHON_HASH = "3b8e5a248bcc351265ffe68f5e457668a2e09c99a47f81f658988c7d3d107522"
BASELINE = "1.0.1-beta.6"


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def download(url, path, expected=None):
    if path.exists() and (expected is None or digest(path) == expected):
        return path
    temporary = path.with_suffix(path.suffix + ".part")
    with urllib.request.urlopen(url, timeout=60) as response, open(temporary, "wb") as output:
        shutil.copyfileobj(response, output)
    if expected and digest(temporary) != expected:
        raise RuntimeError(f"download checksum differs: {url}")
    temporary.replace(path)
    return path


def previous(cache):
    name = "fastctx-x86_64-unknown-linux-gnu.tar.gz"
    repository = os.environ.get("GITHUB_REPOSITORY")
    if not repository:
        raise RuntimeError("pass --previous outside GitHub Actions, or set GITHUB_REPOSITORY")
    base = f"https://github.com/{repository}/releases/download/v{BASELINE}"
    sums = download(base + "/SHA256SUMS", cache / "baseline-SHA256SUMS")
    expected = next(line.split()[0] for line in sums.read_text().splitlines() if line.split()[-1].lstrip("*") == name)
    archive = download(base + "/" + name, cache / "baseline.tar.gz", expected)
    binary = cache / "previous"
    with tarfile.open(archive, "r:gz") as package, open(binary, "wb") as output:
        member = next(entry for entry in package.getmembers() if entry.name in ("fastctx", "./fastctx") and entry.isfile())
        shutil.copyfileobj(package.extractfile(member), output)
    binary.chmod(0o755)
    return binary


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def can_use_kvm():
    # Device access alone does not prove nested virtualization is usable.
    try:
        import fcntl
        device = os.open("/dev/kvm", os.O_RDWR | os.O_CLOEXEC)
        try:
            if fcntl.ioctl(device, 0xAE00) != 12:  # KVM_GET_API_VERSION
                return False
            machine = fcntl.ioctl(device, 0xAE01, 0)  # KVM_CREATE_VM
            os.close(machine)
            return True
        finally:
            os.close(device)
    except OSError:
        return False


class Guest:
    def __init__(self, args, work, key):
        self.args, self.work, self.key = args, work, key
        self.port = port()
        self.process = None

    def ssh(self, command, *, data=None, check=True, timeout=900):
        argv = ["ssh", "-p", str(self.port), "-i", str(self.key), "-o", "BatchMode=yes", "-o", "ConnectTimeout=5",
                "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=" + str(self.work / "known_hosts"),
                "-o", "ServerAliveInterval=10", "root@127.0.0.1", command]
        result = subprocess.run(argv, input=data, capture_output=True, timeout=timeout)
        if check and result.returncode:
            raise RuntimeError((result.stdout + result.stderr).decode("utf-8", "replace"))
        return result

    def ready(self, after_reboot=None):
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError("QEMU exited: " + (self.work / "qemu.log").read_text(errors="replace"))
            try:
                # 2026-10-20: the image restarts sshd during cloud-config, after
                # the first successful login. Upload only after cloud-init finishes.
                result = self.ssh("test -f /var/lib/cloud/instance/boot-finished && cat /proc/sys/kernel/random/boot_id",
                                  check=False, timeout=10)
                if result.returncode == 0 and result.stdout.strip() and result.stdout.strip() != after_reboot:
                    return result.stdout.strip()
            except subprocess.TimeoutExpired:
                pass
            time.sleep(2)
        raise RuntimeError("CentOS 7 did not become reachable within 240 seconds")

    def upload(self, source, destination):
        self.ssh("cat > " + shlex.quote(destination), data=Path(source).read_bytes())

    def phase(self, phase):
        command = ("env PATH=/usr/bin:/bin:/usr/sbin:/sbin FASTCTX_HOME=/var/tmp/fastctx-test-unused "
                   "TMPDIR=/var/tmp RUNNER_TEMP=/var/tmp "
                   "/opt/fastctx-test/python/bin/python3 /opt/fastctx-test/old-linux-guest.py " + shlex.quote(phase))
        result = self.ssh(command, check=False)
        text = (result.stdout + result.stderr).decode("utf-8", "replace")
        (self.work / (phase + ".log")).write_text(text)
        print(text, flush=True)
        if result.returncode:
            raise RuntimeError(f"CentOS 7 lifecycle phase {phase} failed ({result.returncode})")

    def start(self, image, seed):
        qemu = self.args.qemu or shutil.which("qemu-system-x86_64")
        if not qemu:
            raise RuntimeError("qemu-system-x86_64 is required")
        image_tool = str(Path(qemu).with_name("qemu-img"))
        overlay = self.work / "guest.qcow2"
        subprocess.run([image_tool, "create", "-f", "qcow2", "-F", "qcow2", "-b", str(image), str(overlay)], check=True)
        accelerated = can_use_kvm()
        argv = [qemu, "-accel", "kvm" if accelerated else "tcg", "-cpu", "host" if accelerated else "max",
                "-smp", "2", "-m", "2048", "-nodefaults", "-display", "none",
                "-drive", f"file={overlay},format=qcow2,if=virtio",
                "-drive", f"file={seed},format=raw,media=cdrom,if=ide",
                "-netdev", f"user,id=network,restrict=on,hostfwd=tcp:127.0.0.1:{self.port}-:22",
                "-device", "virtio-net-pci,netdev=network,romfile=",
                "-object", "rng-random,id=random,filename=/dev/urandom", "-device", "virtio-rng-pci,rng=random",
                "-serial", "file:" + str(self.work / "serial.log")]
        if self.args.bios:
            argv += ["-bios", str(self.args.bios)]
        print("CentOS 7 acceleration: " + ("KVM" if accelerated else "TCG"), flush=True)
        with open(self.work / "qemu.log", "wb") as log:
            self.process = subprocess.Popen(argv, stdout=log, stderr=subprocess.STDOUT)

    def close(self):
        if self.process is None:
            return
        if self.process.poll() is None:
            try:
                self.ssh("poweroff", check=False, timeout=20)
                self.process.wait(timeout=30)
            except (subprocess.TimeoutExpired, OSError):
                self.process.terminate()
                try:
                    self.process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait()
        # Logs survive, but a stopped disposable guest must not fill the cache.
        (self.work / "guest.qcow2").unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("program", type=Path)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--previous", type=Path)
    parser.add_argument("--qemu")
    parser.add_argument("--bios", type=Path)
    parser.add_argument("--ssh-key", type=Path)
    args = parser.parse_args()
    cache, work = args.cache.resolve(), args.work.resolve()
    cache.mkdir(parents=True, exist_ok=True)
    work.mkdir(parents=True)
    image = download(IMAGE, cache / "centos7.qcow2c", IMAGE_HASH)
    python = download(PYTHON, cache / "python.tar.gz", PYTHON_HASH)
    baseline = args.previous.resolve() if args.previous else previous(cache)
    key = args.ssh_key.resolve() if args.ssh_key else work / "key"
    if not args.ssh_key:
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)], check=True)
        key.chmod(0o600)
    if key.stat().st_mode & 0o077:
        raise RuntimeError("SSH key permissions are too open; use --ssh-key on a filesystem supporting mode 0600")
    metadata = work / "meta-data"
    userdata = work / "user-data"
    metadata.write_text("instance-id: fastctx-old-linux\nlocal-hostname: fastctx-test\n")
    userdata.write_text("#cloud-config\ndisable_root: false\nssh_pwauth: false\nusers:\n  - name: root\n"
                        "    ssh_authorized_keys:\n      - " + Path(str(key) + ".pub").read_text().strip() + "\n")
    iso = shutil.which("genisoimage") or shutil.which("mkisofs")
    if not iso:
        raise RuntimeError("genisoimage or mkisofs is required")
    seed = work / "seed.iso"
    subprocess.run([iso, "-quiet", "-output", str(seed), "-volid", "cidata", "-joliet", "-rock", str(userdata), str(metadata)], check=True)
    guest = Guest(args, work, key)
    try:
        guest.start(image, seed)
        guest.ready()
        guest.ssh("mkdir -p /opt/fastctx-test")
        guest.ssh("tar xzf - -C /opt/fastctx-test", data=python.read_bytes())
        for source, destination in [(args.program, "fastctx"), (baseline, "previous"),
                                    (HERE / "smoke.py", "smoke.py"), (HERE / "old-linux-guest.py", "old-linux-guest.py")]:
            guest.upload(source, "/opt/fastctx-test/" + destination)
        guest.ssh("chmod 755 /opt/fastctx-test/fastctx /opt/fastctx-test/previous")
        guest.phase("setup")
        before = guest.ssh("cat /proc/sys/kernel/random/boot_id").stdout.strip()
        guest.ssh("reboot", check=False, timeout=20)
        guest.ready(after_reboot=before)
        guest.phase("boot")
        guest.phase("renewal")
        guest.phase("cleanup")
        (work / "result.json").write_text(json.dumps({"passed": True, "program_sha256": digest(args.program), "image_sha256": IMAGE_HASH}))
        return 0
    except Exception as error:
        print(str(error), file=sys.stderr, flush=True)
        try:
            guest.phase("diagnostics")
        except Exception as diagnostic_error:
            print("Collecting guest diagnostics: " + str(diagnostic_error), file=sys.stderr)
        return 1
    finally:
        guest.close()


if __name__ == "__main__":
    sys.exit(main())
