"""Write a release's latest.json: the manifest the official updater reads, and the World with it
(`world-upgrade.md`, 两个通道), from the release's signed packages. Each package's signature
must be bound to the release's version; every platform must be there.

    python scripts/release-manifest.py --version V --assets DIR --notes TEXT --output latest.json

DIR holds the packages the release publishes, each with its `.sig` beside it, and the command-line
programs, `fastctx-<triple>.tar.gz` with theirs, which the release does not publish: the network's
machines that carry them hand them on.
"""
import argparse
import base64
import json
from datetime import datetime, timezone
from pathlib import Path

REPOSITORY = 'yc-duan/fastctx'


def packages(version):
    """Each published package by name, with the updater targets it serves."""
    return {
        f'Fastctx_{version}_x64-setup.exe': ['windows-x86_64-nsis', 'windows-x86_64'],
        f'Fastctx_{version}_arm64-setup.exe': ['windows-aarch64-nsis', 'windows-aarch64'],
        f'Fastctx_{version}_aarch64.app.tar.gz': ['darwin-aarch64-app', 'darwin-aarch64'],
        f'Fastctx_{version}_x64.app.tar.gz': ['darwin-x86_64-app', 'darwin-x86_64'],
        f'Fastctx_{version}_amd64.AppImage': ['linux-x86_64-appimage', 'linux-x86_64'],
        f'Fastctx_{version}_amd64.deb': ['linux-x86_64-deb'],
        f'Fastctx_{version}_universal.apk': ['android-apk'],
    }


KERNELS = ['x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu']


def signature(path, version):
    text = (path.parent/(path.name+'.sig')).read_text(encoding='utf-8').strip()
    trusted = base64.b64decode(text).decode('utf-8').splitlines()[2]
    if f'version:{version}' not in trusted.split('\t'):
        raise ValueError(f'{path.name} is not signed for {version}: {trusted}')
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', required=True)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--notes', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    base = f'https://github.com/{REPOSITORY}/releases/download/v{args.version}'
    platforms = {}
    for name, targets in packages(args.version).items():
        path = args.assets/name
        if not path.is_file():
            raise ValueError(f'the release has no {name}')
        entry = {'url': f'{base}/{name}', 'signature': signature(path, args.version), 'size': path.stat().st_size}
        for target in targets:
            platforms[target] = entry
    fastctx = {}
    for triple in KERNELS:
        path = args.assets/f'fastctx-{triple}.tar.gz'
        if not path.is_file():
            raise ValueError(f'the release has no {path.name}')
        fastctx[triple] = {'signature': signature(path, args.version), 'size': path.stat().st_size}
    manifest = {
        'version': args.version,
        'notes': args.notes,
        'pub_date': datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'),
        'platforms': platforms,
        'fastctx': fastctx,
    }
    args.output.write_text(json.dumps(manifest, indent=2)+'\n', encoding='utf-8')
    print(f'{args.output}: {len(platforms)} updater targets, {len(fastctx)} command-line programs')


if __name__ == '__main__':
    main()
