"""Verify or restore the pinned official ripgrep images; never runs at installation."""
import argparse
import gzip
import hashlib
import io
import json
import struct
import tarfile
import urllib.request
import zipfile
from pathlib import Path

from license_text import share_html_notices

VENDOR = Path(__file__).resolve().parents[1] / 'crates/fastctx-ripgrep/vendor'


def manifest():
    return json.loads((VENDOR / 'manifest.json').read_bytes())


def image(target):
    target = target.replace('-unknown-linux-gnu', '-unknown-linux-musl')
    entry = manifest()['targets'][target]
    data = gzip.decompress((VENDOR / f'{target}.gz').read_bytes())
    verify_image(data, entry)
    return data


def verify_image(data, entry):
    if len(data) != entry['size'] or hashlib.sha256(data).hexdigest() != entry['sha256']:
        raise ValueError(f"ripgrep executable does not match pinned SHA-256 {entry['sha256']}")


def windows_imports(binary):
    pe = struct.unpack_from('<I', binary, 0x3c)[0]
    optional = pe + 24
    sections = optional + struct.unpack_from('<H', binary, pe + 20)[0]
    count = struct.unpack_from('<H', binary, pe + 6)[0]

    def offset(rva):
        for index in range(count):
            virtual_size, address, raw_size, raw = struct.unpack_from('<IIII', binary, sections + 40 * index + 8)
            if address <= rva < address + max(virtual_size, raw_size):
                return raw + rva - address
        raise ValueError(f'PE import address {rva} is outside its sections')

    imports = offset(struct.unpack_from('<I', binary, optional + 112 + 8)[0])
    names = []
    while any(binary[imports:imports + 20]):
        name = offset(struct.unpack_from('<I', binary, imports + 12)[0])
        names.append(binary[name:binary.index(0, name)].decode('ascii'))
        imports += 20
    return names


def verify_runtime(binary, target):
    if 'windows' in target:
        # The official 15.2.0 ARM64 asset requires VCRUNTIME140.dll. The World
        # must also run on installations without the Visual C++ redistributable.
        external = [name for name in windows_imports(binary)
                    if name.lower().startswith(('vcruntime', 'msvcp', 'api-ms-win-crt-'))
                    or name.lower() == 'ucrtbase.dll']
        if external:
            raise ValueError(f'ripgrep {target} requires a dynamic CRT: {external}')


def verify_embedded(binary, target):
    payload = image(target)
    verify_runtime(payload, target)
    if payload not in binary:
        raise ValueError(f'fastctx does not embed the pinned official ripgrep for {target}')


def notices():
    pinned = manifest()
    text = (f"\n## Bundled ripgrep {pinned['version']}\n\n"
            f"Upstream: {pinned['repository']}/releases/tag/{pinned['version']}. "
            "Five targets use unmodified official release executables. Windows ARM64 is built "
            "from the same unmodified official source with a static CRT; the official ARM64 release "
            "requires the separately installed Visual C++ runtime. "
            "The following upstream license texts are reproduced with LF line endings.\n")
    for name in ('COPYING', 'LICENSE-MIT', 'UNLICENSE'):
        text += f'\n### ripgrep {name}\n\n```text\n' + (VENDOR / name).read_text(encoding='utf-8') + '```\n'
    text += '\n### PCRE2 10.45 (bundled by official ripgrep)\n\n' + (VENDOR / 'PCRE2-LICENCE.md').read_text(encoding='utf-8')
    text += '\n### ripgrep Rust dependencies\n\n' + (VENDOR / 'DEPENDENCIES.md').read_text(encoding='utf-8')
    components = VENDOR.parents[2] / 'licenses/components'
    for name in ('jemalloc-5.3.1', 'regex-syntax-0.8.11-unicode', 'musl-1.2.5-zig-0.14.1'):
        text += f'\n### ripgrep supplemental {name}\n\n' + (components / f'{name}.txt').read_text(encoding='utf-8')
    directory = components.parent / 'ripgrep-rust'
    toolchains = json.loads((directory / 'sources.json').read_bytes())
    covered = []
    for entry in toolchains:
        data = (directory / entry['file']).read_bytes()
        if hashlib.sha256(data).hexdigest() != entry['sha256']:
            raise ValueError(f"Rust notice does not match its recorded hash: {entry['file']}")
        for target in entry['targets']:
            if entry['commit'].encode() not in image(target):
                raise ValueError(f'ripgrep {target} toolchain changed; refresh its Rust notices')
            covered.append(target)
        text += f"\n### ripgrep Rust {entry['version']} standard library\n\n"
        text += 'Targets: ' + ', '.join(entry['targets']) + f". Compiler commit: {entry['commit']}.\n\n"
        text += data.decode('utf-8')
    if sorted(covered) != sorted(pinned['targets']):
        raise ValueError('Every ripgrep target must have exactly one standard-library notice inventory')
    return share_html_notices(text)


def fetch():
    pinned = manifest()
    for target, entry in pinned['targets'].items():
        if 'source_build' in entry:
            verify_runtime(image(target), target)
            print(f'Verified vendored {target} source build; rebuild instructions: crates/fastctx-ripgrep/README.md')
            continue
        url = f"{pinned['repository']}/releases/download/{pinned['version']}/{entry['archive']}"
        with urllib.request.urlopen(url) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != entry['archive_sha256']:
            raise ValueError(f'upstream archive SHA-256 mismatch: {url}')
        directory = f"ripgrep-{pinned['version']}-{target}"
        executable = 'rg.exe' if 'windows' in target else 'rg'
        if entry['archive'].endswith('.zip'):
            with zipfile.ZipFile(io.BytesIO(data)) as archive:
                files = {name: archive.read(f'{directory}/{name}')
                         for name in (executable, 'COPYING', 'LICENSE-MIT', 'UNLICENSE')}
        else:
            with tarfile.open(fileobj=io.BytesIO(data)) as archive:
                files = {name: archive.extractfile(f'{directory}/{name}').read()
                         for name in (executable, 'COPYING', 'LICENSE-MIT', 'UNLICENSE')}
        verify_image(files[executable], entry)
        for name in ('COPYING', 'LICENSE-MIT', 'UNLICENSE'):
            if files[name].replace(b'\r\n', b'\n') != (VENDOR / name).read_bytes():
                raise ValueError(f'upstream license differs: {target}/{name}')
        path = VENDOR / f'{target}.gz'
        # Stage verified bytes, so an interrupted fetch cannot damage a build.
        staged = path.with_suffix('.gz.tmp')
        with staged.open('wb') as raw:
            with gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as output:
                output.write(files[executable])
        staged.replace(path)
        print(f'Verified official {entry["archive"]}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['verify', 'fetch'])
    args = parser.parse_args()
    if args.command == 'fetch':
        fetch()
    else:
        from package import verify_machine
        for target in manifest()['targets']:
            payload = image(target)
            verify_machine(payload, target.replace('-unknown-linux-musl', '-unknown-linux-gnu'))
            verify_runtime(payload, target)
        print(f"Verified ripgrep {manifest()['version']}: six images, static Linux linkage and Windows CRT")


if __name__ == '__main__':
    main()
