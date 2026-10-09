"""Assemble npm tarballs and strict four-member legacy Release archives."""
import argparse
import gzip
import hashlib
import io
import json
import struct
import sys
import tarfile
import tomllib
import zipfile
from pathlib import Path

import ripgrep
from license_text import input_marker

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    'x86_64-pc-windows-msvc': ('win32', 'x64'),
    'aarch64-pc-windows-msvc': ('win32', 'arm64'),
    'x86_64-unknown-linux-gnu': ('linux', 'x64'),
    'aarch64-unknown-linux-gnu': ('linux', 'arm64'),
    'x86_64-apple-darwin': ('darwin', 'x64'),
    'aarch64-apple-darwin': ('darwin', 'arm64'),
}
VERSION = tomllib.loads((ROOT/'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
LEGAL = ['LICENSE-APACHE', 'NOTICE', 'THIRD_PARTY_LICENSES.md']


def json_bytes(value):
    return (json.dumps(value, indent=2, ensure_ascii=False)+'\n').encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def verify_machine(binary, target):
    os, arch = TARGETS[target]
    if os == 'win32':
        assert binary[:2] == b'MZ', 'Expected a PE executable'
        offset = struct.unpack_from('<I', binary, 0x3c)[0]
        assert binary[offset:offset+4] == b'PE\0\0'
        assert struct.unpack_from('<H', binary, offset+4)[0] == {'x64': 0x8664, 'arm64': 0xaa64}[arch]
    elif os == 'linux':
        assert binary[:6] == b'\x7fELF\x02\x01', 'Expected a little-endian ELF64 executable'
        assert struct.unpack_from('<H', binary, 18)[0] == {'x64': 62, 'arm64': 183}[arch]
        # A historical GNU archive name is retained for the old upgrader.
        # Its new payload is musl: no runtime loader or shared libraries.
        offset = struct.unpack_from('<Q', binary, 32)[0]
        entry_size, count = struct.unpack_from('<HH', binary, 54)
        for index in range(count):
            entry = offset + index * entry_size
            kind = struct.unpack_from('<I', binary, entry)[0]
            assert kind != 3, 'Linux releases must not have PT_INTERP'
            if kind == 2:
                dynamic_offset = struct.unpack_from('<Q', binary, entry + 8)[0]
                dynamic_size = struct.unpack_from('<Q', binary, entry + 32)[0]
                for position in range(dynamic_offset, dynamic_offset + dynamic_size, 16):
                    tag = struct.unpack_from('<q', binary, position)[0]
                    assert tag != 1, 'Linux releases must not have DT_NEEDED'
                    if tag == 0:
                        break
    else:
        assert struct.unpack_from('<I', binary)[0] == 0xfeedfacf, 'Expected a Mach-O 64 executable'
        assert struct.unpack_from('<I', binary, 4)[0] == {'x64': 0x01000007, 'arm64': 0x0100000c}[arch]


def archive(path, files):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.suffix == '.zip':
        with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as output:
            for name, (data, mode) in sorted(files.items()):
                member = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
                member.create_system = 3
                member.external_attr = (0o100000 | mode) << 16
                member.compress_type = zipfile.ZIP_DEFLATED
                output.writestr(member, data)
    else:
        with path.open('wb') as raw, gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as output:
                for name, (data, mode) in sorted(files.items()):
                    member = tarfile.TarInfo(name)
                    member.size, member.mode = len(data), mode
                    output.addfile(member, io.BytesIO(data))


def manifest(name):
    return dict(name=name, version=VERSION, description='One world, many machines.',
                license='Apache-2.0', repository=dict(type='git', url='git+https://github.com/yc-duan/fastctx.git'),
                publishConfig=dict(access='public'))


def legal_files(notices):
    if input_marker(ROOT) not in notices.read_text(encoding='utf-8'):
        raise ValueError('THIRD_PARTY_LICENSES.md is stale; regenerate it with scripts/notices.py')
    if ripgrep.notices().strip() not in notices.read_text(encoding='utf-8'):
        raise ValueError('THIRD_PARTY_LICENSES.md is missing the bundled ripgrep notices')
    return {name: ((notices if name == 'THIRD_PARTY_LICENSES.md' else ROOT/name).read_bytes(), 0o644)
            for name in LEGAL}


def verify_embedded_legal(binary, legal):
    for name, (data, _) in legal.items():
        if data not in binary:
            raise ValueError(f'Executable does not embed this build\'s {name}; rebuild it with these notices')


def check_binary(args):
    binary = args.binary.read_bytes()
    verify_machine(binary, args.target)
    ripgrep.verify_embedded(binary, args.target)
    verify_embedded_legal(binary, legal_files(args.notices))
    print(f'Verified embedded legal material: {args.binary}', file=sys.stderr)


def npm(output, name, metadata, files):
    packed = {f'package/{name}': contents for name, contents in files.items()}
    packed['package/package.json'] = (json_bytes(metadata), 0o644)
    filename = name.lstrip('@').replace('/', '-')+'-'+VERSION+'.tgz'
    archive(output/'npm'/filename, packed)


def roots(args):
    files = {f'licenses/{name}': data for name, data in legal_files(args.notices).items()}
    files['launcher.js'] = ((ROOT/'packages/launcher.js').read_bytes(), 0o755)
    files['README.md'] = ((ROOT/'README.md').read_bytes(), 0o644)
    for name in ('fastctx', 'codex-fastctx'):
        metadata = manifest(name)
        metadata.update(engines=dict(node='>=18'), bin=dict(fastctx='launcher.js'),
                        optionalDependencies={f'@fastctx/{os}-{arch}': VERSION for os, arch in TARGETS.values()})
        npm(args.output, name, metadata, files)


def platform(args):
    os, arch = TARGETS[args.target]
    executable = 'fastctx.exe' if os == 'win32' else 'fastctx'
    binary = args.binary.read_bytes()
    verify_machine(binary, args.target)
    ripgrep.verify_embedded(binary, args.target)
    legal = legal_files(args.notices)
    verify_embedded_legal(binary, legal)
    files = {**legal, executable: (binary, 0o755)}
    extension = '.zip' if os == 'win32' else '.tar.gz'
    name = f'fastctx-{args.target}{extension}'
    archive(args.output/name, files)
    metadata = manifest(f'@fastctx/{os}-{arch}')
    metadata.update(os=[os], cpu=[arch])
    npm(args.output, metadata['name'], metadata, {f'licenses/{n}': data for n, data in legal.items()} |
        {f'bin/{executable}': (binary, 0o755)})
    build_target = args.target.replace('-unknown-linux-gnu', '-unknown-linux-musl')
    evidence = dict(version=VERSION, target=args.target, build_target=build_target, archive=name,
                    members={n: digest(data) for n, (data, _) in sorted(files.items())})
    (args.output/f'{args.target}.json').write_bytes(json_bytes(evidence))
    checksums(args.output)


def checksums(output):
    files = sorted([*output.glob('fastctx-*.zip'), *output.glob('fastctx-*.tar.gz')])
    (output/'SHA256SUMS').write_text(''.join(f'{digest(p.read_bytes())}  {p.name}\n' for p in files),
                                   encoding='ascii', newline='\n')


def verify(args):
    targets = TARGETS if args.complete else [p.stem for p in args.output.glob('*.json')]
    if not targets:
        raise ValueError('No platform packages to verify')
    for target in targets:
        evidence = json.loads((args.output/f'{target}.json').read_bytes())
        assert evidence['version'] == VERSION
        os, arch = TARGETS[target]
        executable = 'fastctx.exe' if os == 'win32' else 'fastctx'
        path = args.output/evidence['archive']
        if path.suffix == '.zip':
            with zipfile.ZipFile(path) as source:
                names = source.namelist()
                files = {n: source.read(n) for n in names}
        else:
            with tarfile.open(path) as source:
                names = source.getnames()
                assert all(m.isfile() for m in source)
                files = {n: source.extractfile(n).read() for n in names}
        assert len(names) == 4 and set(names) == {*LEGAL, executable}, names
        assert {n: digest(data) for n, data in files.items()} == evidence['members']
        verify_machine(files[executable], target)
        ripgrep.verify_embedded(files[executable], target)
        verify_embedded_legal(files[executable], {name: (files[name], 0o644) for name in LEGAL})
        assert input_marker(ROOT) in files['THIRD_PARTY_LICENSES.md'].decode('utf-8')
        assert ripgrep.notices().strip() in files['THIRD_PARTY_LICENSES.md'].decode('utf-8').replace('\r\n', '\n')
        path = args.output/'npm'/f'fastctx-{os}-{arch}-{VERSION}.tgz'
        with tarfile.open(path) as source:
            assert source.extractfile('package/bin/'+executable).read() == files[executable]
            metadata = json.load(source.extractfile('package/package.json'))
            assert metadata['name'] == f'@fastctx/{os}-{arch}' and metadata['version'] == VERSION
            for name in LEGAL:
                assert source.extractfile('package/licenses/'+name).read() == files[name]
    for name in ('fastctx', 'codex-fastctx'):
        with tarfile.open(args.output/'npm'/f'{name}-{VERSION}.tgz') as source:
            metadata = json.load(source.extractfile('package/package.json'))
            assert metadata['version'] == VERSION and metadata['name'] == name
            assert metadata['bin'] == dict(fastctx='launcher.js')
            assert metadata['optionalDependencies'] == {f'@fastctx/{o}-{a}': VERSION for o, a in TARGETS.values()}
            assert source.extractfile('package/launcher.js').read() == (ROOT/'packages/launcher.js').read_bytes()
            for legal_name in LEGAL:
                member = source.extractfile('package/licenses/'+legal_name)
                assert member is not None, f'{name} is missing {legal_name}'
                data = member.read()
                assert data == files[legal_name], f'{name}: {legal_name} differs from the native distribution'
    checksums(args.output)
    print(f'Verified {len(targets)} platform(s) and both npm roots at {VERSION}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    for command in ('roots', 'platform', 'verify', 'check-binary'):
        p = commands.add_parser(command)
        if command != 'check-binary':
            p.add_argument('--output', type=Path, required=True)
        if command != 'verify':
            p.add_argument('--notices', type=Path, required=True)
        else:
            p.add_argument('--complete', action='store_true')
        if command in ('platform', 'check-binary'):
            p.add_argument('--target', choices=TARGETS, required=True)
            p.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    globals()[args.command.replace('-', '_')](args)


if __name__ == '__main__':
    main()
