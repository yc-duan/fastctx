"""Attribute the deployed Linux payload, seal it with the reviewed runtime, and verify both GUI packages."""
import argparse
import gzip
import hashlib
import json
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from license_text import fenced, gui_input_marker
from package import check_binary

ROOT = Path(__file__).resolve().parents[1]


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def one(paths):
    paths = list(paths)
    if len(paths) != 1:
        raise ValueError(f'Expected one artifact, found {paths}')
    return paths[0]


def owners(path):
    """linuxdeploy flattens libraries and edits their RPATH; retain original ownership."""
    result = subprocess.run(['dpkg-query', '-S', '*/'+path.name], text=True, capture_output=True)
    if result.returncode not in (0, 1):
        result.check_returncode()
    candidates = []
    for line in result.stdout.splitlines():
        package, separator, original = line.partition(': ')
        if separator and Path(original).is_file():
            candidates.extend((name.strip(), Path(original)) for name in package.split(','))
    identical = [(package, original) for package, original in candidates if sha(original) == sha(path)]
    if identical:
        return identical
    if candidates:
        return candidates
    raise ValueError(f'No installed-package attribution for deployed file {path}')


def native_notices(appdir):
    gui_files = {Path('usr/bin/fastctx-app'), Path('usr/bin/fastctx'), Path('Fastctx.png')}
    records, packages = {}, set()
    hooks = []
    for path in sorted(appdir.rglob('*')):
        if not path.is_file() or path.is_symlink():
            continue
        relative = path.relative_to(appdir)
        if relative in gui_files or relative.parts[:3] == ('usr', 'lib', 'Fastctx') or relative.parts[:4] == ('usr', 'share', 'doc', 'fastctx'):
            continue
        if relative.parts[:3] in (('usr', 'share', 'icons'), ('usr', 'share', 'applications')):
            continue
        if relative == Path('AppRun'):
            continue
        if relative.parts[0] == 'apprun-hooks':
            hooks.append(path)
            continue
        if path.name in ('gschemas.compiled', 'loaders.cache', 'immodules.cache', 'giomodule.cache'):
            records[str(relative)] = {'sha256': sha(path), 'generated': 'GTK deployment cache from the attributed modules and schemas'}
            continue
        found = owners(path)
        packages.update(package for package, _ in found)
        records[str(relative)] = {'sha256': sha(path), 'sources': [
            {'package': package, 'path': str(original), 'sha256': sha(original), 'modified': sha(path) != sha(original)}
            for package, original in found]}
    text = ('# Linux native components carried by this AppImage\n\n'
            'Libraries and resources below come from the build distribution. linuxdeploy changes ELF '
            'RPATHs and its GTK plugin changes WebKit paths for relocation; the manifest records each '
            'deployed hash and its original package files. GTK caches are generated from these resources. '
            'The AppImage can be extracted and these shared libraries replaced or relinked.\n\n')
    for package in sorted(packages):
        fields = subprocess.check_output(['dpkg-query', '-W', '-f=${binary:Package}\t${Version}\t${source:Package}\t${source:Version}', package], text=True).split('\t')
        binary, version, source, source_version = fields
        copyright_file = Path('/usr/share/doc')/binary.split(':')[0]/'copyright'
        copyright_text = copyright_file.read_text(encoding='utf-8')
        text += f'## {binary} {version}\n\nSource: https://launchpad.net/ubuntu/+source/{source}/{source_version}\n\n'+fenced(copyright_text)
        for name in sorted(set(re.findall(r'/usr/share/common-licenses/([A-Za-z0-9.+-]+)', copyright_text))):
            name = name.rstrip('.')
            license_path = Path('/usr/share/common-licenses')/name
            # Some packages name `GPL-2.0` what Debian and Ubuntu keep as `GPL-2`.
            if not license_path.is_file() and name.endswith('.0'):
                license_path = license_path.with_name(name[:-2])
            if not license_path.is_file():
                raise FileNotFoundError(f'{binary} refers to /usr/share/common-licenses/{name}, which is not there')
            text += f'### {name}\n\n'+fenced(license_path.read_text())
    if hooks:
        text += ('## GTK deployment hook\n\nThe linuxdeploy GTK plugin by TheAssassin and the linuxdeploy contributors '
                 'is modified and embedded by Tauri CLI 2.12.1; '
                 'upstream source: https://github.com/linuxdeploy/linuxdeploy-plugin-gtk/tree/35058592f5b4b38c7f37358aaf274a3183a50ba9 . '
                 'source and modifications: https://github.com/tauri-apps/tauri/tree/tauri-cli-v2.12.1/crates/tauri-bundler/src/bundle/linux/appimage\n\n'
                 'Tauri is distributed under Apache-2.0 or MIT. Its MIT license follows.\n\n')
        for name, expected_hash in (
                ('linuxdeploy-gtk-MIT.txt', '0718f6cadf38d2662f397591b777884c5389e65b9919aae6aca2bea87a52eac4'),
                ('tauri-MIT.txt', '9dd42ea92cff2ede5cd477cbfcce051b2d0115c0ac7f368ee88cb545055dff1d')):
            notice = ROOT/'gui/licenses'/name
            if sha(notice) != expected_hash:
                raise ValueError(f'Review the changed GTK hook notice: {notice}')
            text += fenced(notice.read_text())
        for hook in hooks:
            records[str(hook.relative_to(appdir))] = {'sha256': sha(hook), 'source': 'Tauri CLI 2.12.1 GTK deployment script'}
    return text, records


def runtime():
    directory = ROOT/'gui/licenses/linux-runtime'
    sources = json.loads((directory/'sources.json').read_text())
    text = '# AppImage runtime\n\n'+sources['description']+'\n\n'
    for entry in sources['notices']:
        path = directory/entry['file']
        if sha(path) != entry['sha256']:
            raise ValueError(f'Review the changed runtime notice: {path}')
        text += 'Source: '+entry['url']+'\n\n'+fenced(path.read_text())
    data = gzip.decompress((directory/'runtime-x86_64.gz').read_bytes())
    if hashlib.sha256(data).hexdigest() != sources['runtime']['sha256']:
        raise ValueError('AppImage runtime differs from the reviewed upstream binary')
    return data, text


def verify_payload(directory, expected):
    kernels = [path for path in directory.rglob('fastctx') if path.is_file() and not path.is_symlink()]
    kernel = one(kernels)
    check_binary(argparse.Namespace(binary=kernel, target='x86_64-unknown-linux-gnu', notices=ROOT/'dist/THIRD_PARTY_LICENSES.md'))
    if kernel.read_bytes() != expected['kernel']:
        raise ValueError(f'The GUI carries another kernel: {kernel}')
    resources = one(path for path in directory.rglob('THIRD_PARTY_LICENSES.md') if path.parent.name == 'licenses')
    for name in ('LICENSE-APACHE', 'NOTICE', 'THIRD_PARTY_LICENSES.md'):
        if (resources.parent/name).read_bytes() != expected[name]:
            raise ValueError(f'Installed GUI legal resource differs: {name}')
    if gui_input_marker(ROOT).encode() not in resources.read_bytes():
        raise ValueError('The GUI notices do not match this build')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    bundle = args.target_dir/'release/bundle'
    appdir = one((bundle/'appimage').glob('*.AppDir'))
    image = one((bundle/'appimage').glob('*.AppImage'))
    deb = one((bundle/'deb').glob('*.deb'))
    version = json.loads((ROOT/'gui/src-tauri/tauri.conf.json').read_text())['version']
    if subprocess.check_output(['dpkg-deb', '-f', str(deb), 'Version'], text=True).strip() != version:
        raise ValueError('Debian package version differs from the GUI')
    # The uninstaller removes the package by this name (crates/fastctx-uninstall/src/package.rs).
    if subprocess.check_output(['dpkg-deb', '-f', str(deb), 'Package'], text=True).strip() != 'fastctx':
        raise ValueError('The Debian package is not named fastctx, the name the uninstaller removes')
    launcher = appdir/'AppRun'
    if launcher.is_symlink():
        launcher.unlink()
    shutil.copyfile(ROOT/'gui/scripts/AppRun', launcher)
    launcher.chmod(0o755)
    wrapped = appdir/'AppRun.wrapped'
    if wrapped.exists():
        wrapped.unlink()
    runtime_data, runtime_text = runtime()
    native_text, inventory = native_notices(appdir)
    notices = ROOT/'gui/src-tauri/licenses/THIRD_PARTY_LICENSES.md'
    inventory_data = (json.dumps(inventory, sort_keys=True, indent=2)+'\n').encode()
    marker = '<!-- world-native-inputs: '+hashlib.sha256(inventory_data).hexdigest()+' -->\n'
    full_notices = notices.read_bytes()+('\n'+native_text+'\n'+runtime_text+'\n'+marker).encode()
    resources = one(path for path in appdir.rglob('THIRD_PARTY_LICENSES.md') if path.parent.name == 'licenses')
    resources.write_bytes(full_notices)
    (resources.parent/'THIRD_PARTY_LINUX.json').write_bytes(inventory_data)
    documentation = appdir/'usr/share/doc/fastctx'
    documentation.mkdir(parents=True, exist_ok=True)
    (documentation/'THIRD_PARTY_LICENSES.md').write_bytes(full_notices)
    (documentation/'copyright').write_bytes((ROOT/'gui/src-tauri/licenses/copyright').read_bytes()+('\n'+native_text+'\n'+runtime_text+'\n'+marker).encode())
    expected = {name: (ROOT/'gui/src-tauri/licenses'/name).read_bytes() for name in ('LICENSE-APACHE', 'NOTICE', 'THIRD_PARTY_LICENSES.md')}
    expected['THIRD_PARTY_LICENSES.md'] = full_notices
    expected['kernel'] = (args.target_dir/'x86_64-unknown-linux-musl/release/fastctx').read_bytes()
    expected['gui'] = (appdir/'usr/bin/fastctx-app').read_bytes()
    verify_payload(appdir, expected)
    with tempfile.TemporaryDirectory(dir=bundle) as temporary:
        work = Path(temporary)
        squashfs = work/'payload.squashfs'
        run('mksquashfs', appdir, squashfs, '-noappend', '-comp', 'gzip', '-all-root', '-no-xattrs', '-processors', '4', '-no-progress')
        staged = work/image.name
        with staged.open('wb') as output, squashfs.open('rb') as payload:
            output.write(runtime_data)
            shutil.copyfileobj(payload, output)
        staged.chmod(0o755)
        offset = int(subprocess.check_output([staged, '--appimage-offset'], text=True))
        if offset != len(runtime_data):
            raise ValueError(f'Runtime offset {offset} differs from reviewed runtime size {len(runtime_data)}')
        def packed(relative):
            return subprocess.check_output(['unsquashfs', '-processors', '4', '-cat', '-o', str(offset), str(staged), str(relative)])
        if packed('AppRun') != launcher.read_bytes() or packed('usr/bin/fastctx-app') != expected['gui']:
            raise ValueError('The sealed AppImage launcher or graphical program differs')
        kernel_path = one(path for path in appdir.rglob('fastctx') if path.is_file() and not path.is_symlink()).relative_to(appdir)
        if packed(kernel_path) != expected['kernel']:
            raise ValueError('The sealed AppImage carries another kernel')
        for name in ('LICENSE-APACHE', 'NOTICE', 'THIRD_PARTY_LICENSES.md', 'THIRD_PARTY_LINUX.json'):
            if packed((resources.parent/name).relative_to(appdir)) != (resources.parent/name).read_bytes():
                raise ValueError(f'The sealed AppImage legal resource differs: {name}')
        for relative, record in inventory.items():
            if hashlib.sha256(packed(relative)).hexdigest() != record['sha256']:
                raise ValueError(f'Packaged native component changed: {relative}')
        staged.replace(image)
        debdir = work/'deb'
        run('dpkg-deb', '-x', deb, debdir)
        run('dpkg-deb', '-e', deb, debdir/'DEBIAN')
        shutil.copyfile(ROOT/'gui/scripts/deb-prerm', debdir/'DEBIAN/prerm')
        (debdir/'DEBIAN/prerm').chmod(0o755)
        run('dpkg-deb', '--root-owner-group', '--build', debdir, work/'fastctx.deb')
        shutil.copyfile(work/'fastctx.deb', deb)
        deb_expected = dict(expected)
        deb_expected['THIRD_PARTY_LICENSES.md'] = notices.read_bytes()
        verify_payload(debdir, deb_expected)
        if (debdir/'usr/share/doc/fastctx/copyright').read_bytes() != (ROOT/'gui/src-tauri/licenses/copyright').read_bytes():
            raise ValueError('Debian copyright resource differs')
    args.output.mkdir(parents=True, exist_ok=True)
    for artifact in (deb, image):
        shutil.copy2(artifact, args.output/artifact.name)
    (args.output/'fastctx-linux-native.json').write_text(json.dumps(inventory, indent=2)+'\n')
    (args.output/'GUI-SHA256SUMS').write_text(''.join(f'{sha(path)}  {path.name}\n' for path in (deb, image)))
    print(f'Verified deb and AppImage, {len(inventory)} native files, matching kernel and legal resources')


if __name__ == '__main__':
    main()
