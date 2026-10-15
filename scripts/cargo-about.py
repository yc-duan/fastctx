"""Installs cargo-about 0.9.0 from its published build for this machine, checked against the digests
pinned here, into cargo's bin directory: the same notices as `cargo install --locked cargo-about
--version 0.9.0 --features cli` writes, in seconds instead of minutes.

    python scripts/cargo-about.py
"""
import hashlib
import io
import os
import platform
import shutil
import tarfile
import urllib.request
from pathlib import Path

VERSION = '0.9.0'
# The SHA-256 of each archive EmbarkStudios published for this version.
ARCHIVES = {
    'x86_64-unknown-linux-musl': '7a1a1bfb3ae3b6feabe9e1d6147a6b34b85bd0339bfd8b0ec11b312dee10d99a',
    'aarch64-unknown-linux-musl': '46b202700ea136066cdaca38180da9ab4e7d7534b3418546de858ae0aee4a1cb',
    'x86_64-pc-windows-msvc': '87daca52ad79980b42b2de6acdec84e143be41e2d4bbebedba0d0c90c49b055a',
    'aarch64-pc-windows-msvc': 'dc2b30688d34b8648da364f0a0a2f0a1430d0532ce6b4773667e4d2bd288ab19',
    'aarch64-apple-darwin': '4e1cfc5d58b733f6def1741fc888b435cdb2996da5c2c4547ed36cba191667f6',
}


def machine():
    """This machine's target, as a runner names it or else as Python sees it."""
    system = os.environ.get('RUNNER_OS') or platform.system()
    arch = (os.environ.get('RUNNER_ARCH') or platform.machine()).lower()
    arch = 'aarch64' if arch in ('arm64', 'aarch64') else 'x86_64' if arch in ('x64', 'amd64', 'x86_64') else arch
    kind = {'linux': 'unknown-linux-musl', 'windows': 'pc-windows-msvc', 'macos': 'apple-darwin',
            'darwin': 'apple-darwin'}.get(system.lower())
    target = f'{arch}-{kind}'
    if target not in ARCHIVES:
        raise SystemExit(f'cargo-about {VERSION} publishes no build for {target}')
    return target


def main():
    target = machine()
    name = f'cargo-about-{VERSION}-{target}'
    url = f'https://github.com/EmbarkStudios/cargo-about/releases/download/{VERSION}/{name}.tar.gz'
    with urllib.request.urlopen(url, timeout=120) as response:
        archive = response.read()
    digest = hashlib.sha256(archive).hexdigest()
    if digest != ARCHIVES[target]:
        raise SystemExit(f'{url} has SHA-256 {digest}, not {ARCHIVES[target]}')
    program = 'cargo-about.exe' if 'windows' in target else 'cargo-about'
    bin_dir = Path(os.environ.get('CARGO_HOME') or Path.home()/'.cargo')/'bin'
    bin_dir.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as tar:
        source = tar.extractfile(f'{name}/{program}')
        with open(bin_dir/program, 'wb') as out:
            shutil.copyfileobj(source, out)
    (bin_dir/program).chmod(0o755)
    print(f'cargo-about {VERSION} for {target} in {bin_dir}')


if __name__ == '__main__':
    main()
