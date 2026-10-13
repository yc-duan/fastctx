"""Prepare the readable legal resources installed beside the graphical app."""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tomllib
from pathlib import Path

from license_text import fenced, share_html_notices, gui_input_marker
from package import legal_files

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel-notices', type=Path, default=ROOT/'dist/THIRD_PARTY_LICENSES.md')
    parser.add_argument('--target')
    args = parser.parse_args()
    legal_files(args.kernel_notices)
    gui = ROOT/'gui'
    output = gui/'src-tauri/licenses'
    output.mkdir(exist_ok=True)
    # Tauri names the target it builds for to its hooks; an Android build
    # runs on a computer whose own target is another.
    target = args.target or os.environ.get('TAURI_ENV_TARGET_TRIPLE') or next(line[6:] for line in subprocess.check_output(
        ['rustc', '-vV'], text=True).splitlines() if line.startswith('host: '))
    native = output/'THIRD_PARTY_RUST.md'
    subprocess.run(['cargo', 'about', 'generate', '--locked', '--offline', '--fail',
                    '--manifest-path', str(gui/'src-tauri/Cargo.toml'), '--config', str(ROOT/'about.toml'),
                    '--target', target, str(ROOT/'about.hbs'), '--output-file', str(native)], check=True, cwd=ROOT)
    text = args.kernel_notices.read_text(encoding='utf-8')
    text += '\n# Graphical application Rust dependencies\n\n'+native.read_text(encoding='utf-8')
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    text += '\n# Graphical application Rust standard library\n\n'
    text += (sysroot/'share/doc/rust/COPYRIGHT-library.html').read_text(encoding='utf-8')
    text += '\n# Graphical application dependency sources\n\n'
    packages = tomllib.loads((gui/'src-tauri/Cargo.lock').read_text(encoding='utf-8'))['package']
    for package in packages:
        if package.get('source') == 'registry+https://github.com/rust-lang/crates.io-index':
            name, version = package['name'], package['version']
            text += f'- {name} {version}: https://static.crates.io/crates/{name}/{name}-{version}.crate\n'
    text += '\n'+(gui/'dist/THIRD_PARTY_FRONTEND.md').read_text(encoding='utf-8')
    for path in sorted((gui/'dist/pdfjs').rglob('LICENSE*')):
        text += '\n## PDF.js '+path.relative_to(gui/'dist/pdfjs').as_posix()+'\n\n'+fenced(path.read_text(encoding='utf-8'))
    if 'windows' in target:
        text += webview_notices(gui, packages)
        text += ('\n# Windows installer components\n\n'
                 'NSIS 3.11, including its unmodified LZMA decoder, is available in source form at '
                 'https://github.com/kichik/nsis/archive/refs/tags/v311.tar.gz. '
                 'The LZMA module remains under CPL-1.0 with the linking exception reproduced below. '
                 'Fastctx does not modify this module.\n\n'
                 'nsis-tauri-utils 0.5.3 source: '
                 'https://github.com/tauri-apps/nsis-tauri-utils/tree/nsis_tauri_utils-v0.5.3\n')
        for name in ('NSIS-3.11-COPYING.txt', 'NSIS-3.11-Modern-UI-2.txt', 'nsis-tauri-utils-0.5.3-MIT.txt'):
            text += section_notice(gui/'licenses'/name)
    for name in ('LICENSE-APACHE', 'NOTICE'):
        shutil.copyfile(ROOT/name, output/name)
    text += '\n'+gui_input_marker(ROOT)+'\n'
    (output/'THIRD_PARTY_LICENSES.md').write_text(share_html_notices(text), encoding='utf-8', newline='\n')
    # The Debian package's copyright file; Android is Linux too, without it.
    if '-linux-gnu' in target:
        (output/'copyright').write_text(
            'Fastctx '+json.loads((gui/'package.json').read_text())['version']+'\n\n'
            +(ROOT/'NOTICE').read_text(encoding='utf-8')+'\n'
            +(ROOT/'LICENSE-APACHE').read_text(encoding='utf-8')+'\n'
            +share_html_notices(text), encoding='utf-8', newline='\n')
    print(f'Prepared GUI legal resources for {target}: {output}')


def section_notice(path):
    return '\n## '+path.name+'\n\n'+fenced(path.read_text(encoding='utf-8'))


def webview_notices(gui, packages):
    """The native loader has a separate license from its Rust bindings."""
    provenance = json.loads((gui/'licenses/webview2-sources.json').read_text(encoding='utf-8'))
    for name, version in provenance['crates'].items():
        actual = {package['version'] for package in packages if package['name'] == name}
        if actual != {version}:
            raise ValueError(f'Review WebView2 notices for {name}: expected {version}, found {actual}')
    text = ('\n# Microsoft WebView2 loader and Rust bindings\n\n'
            'The MSVC build links WebView2LoaderStatic.lib from Microsoft.Web.WebView2 1.0.3800.47. '
            'Its BSD-3-Clause license and SDK notices are separate from the MIT Rust bindings.\n')
    text += '\nRust bindings: '+', '.join(name+' '+version for name, version in provenance['crates'].items())+'.\n'
    for entry in provenance['notices']:
        path = gui/'licenses'/entry['file']
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != entry['sha256']:
            raise ValueError(f'Upstream WebView2 notice changed: {path}')
        text += '\nSource: '+entry['url']+'\n'+section_notice(path)
    return text


if __name__ == '__main__':
    main()
