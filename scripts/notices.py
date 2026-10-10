"""Combine release notices into the one file accepted by old upgraders."""
import argparse
import json
import os
import subprocess
import tempfile
import tomllib
from pathlib import Path

import ripgrep
from license_text import component_notices, fenced, input_marker, share_html_notices

ROOT = Path(__file__).resolve().parents[1]
TOOLCHAIN = '+1.92.0'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--syntax-notices', type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(dir=args.output.parent, prefix=args.output.name+'.', suffix='.tmp')
    os.close(descriptor)
    staged = Path(name)
    try:
        generate(staged, args.syntax_notices)
        if not args.output.exists() or staged.read_bytes() != args.output.read_bytes():
            staged.replace(args.output)
    finally:
        staged.unlink(missing_ok=True)


def generate(destination, syntax_notices):
    packages = tomllib.loads((ROOT/'Cargo.lock').read_text(encoding='utf-8'))['package']
    check_jemalloc_notice(packages)
    subprocess.run(['cargo', TOOLCHAIN, 'about', 'generate', '--locked', '--offline', '--fail',
                    str(ROOT/'about.hbs'), '--output-file', str(destination)], cwd=ROOT, check=True)
    sysroot = Path(subprocess.check_output(['rustc', TOOLCHAIN, '--print', 'sysroot'], text=True).strip())
    standard = sysroot/'share/doc/rust/COPYRIGHT-library.html'
    # rustc supplies these copyright files for the installed build toolchain.
    copyright_text = standard.read_text(encoding='utf-8')
    # Compiler notices are supplemental provenance, not a substitute for
    # independently linked native runtimes such as Zig's musl or jemalloc.
    native_copyright = (sysroot/'share/doc/rust/COPYRIGHT.html').read_text(encoding='utf-8')
    patch_notice = (ROOT/'crates/fastctx-hands/src/write/patch/NOTICE').read_text(encoding='utf-8')
    pdf_notice = (ROOT/'crates/fastctx-pdf/src/NOTICE').read_text(encoding='utf-8')
    oxide_notice = (ROOT/'vendor/pdf_oxide/NOTICE').read_text(encoding='utf-8')
    office_notice = (ROOT/'crates/fastctx-office/NOTICE').read_text(encoding='utf-8')
    with destination.open('a', encoding='utf-8', newline='\n') as output:
        output.write(ripgrep.notices())
        output.write('\n'+syntax_notices.read_text(encoding='utf-8'))
        output.write('\n## OpenAI Codex portions\n\n'+patch_notice)
        output.write('\n'+pdf_notice)
        output.write('\n## PDFOxide upstream NOTICE\n\n'+oxide_notice)
        output.write('\nLocal modifications to pdf_oxide 0.3.78 are described in '
                     'vendor/pdf_oxide/FASTCTX-PATCH.md in the fastctx source distribution.\n')
        output.write('\n## Apache POI portions used by Office\n\n'+office_notice+'\n')
        output.write('\n## Supplemental component notices\n\n')
        for name, text in component_notices(ROOT):
            output.write(f'\n### {name}\n\n'+fenced(text))
        output.write('\n## Rust standard library and toolchain notices\n\n'
                     'The full toolchain inventory also names compiler and documentation '
                     'components that are not linked into fastctx. Identical license text '
                     'is reproduced once; each original component mapping is retained.\n\n')
        output.write(share_html_notices(copyright_text+'\n'+native_copyright)+'\n')
        # MPL dependencies need a source location, not just a license text.
        # Registry archives identify the exact locked dependency versions.
        output.write('\n## Dependency source archives\n\n'
                     'Source archives for the crates.io dependencies recorded by this build follow. '
                     'Each dependency retains the license stated in its source and in the notices above.\n\n')
        for package in sorted(packages, key=lambda package: (package['name'], package['version'])):
            if package.get('source') == 'registry+https://github.com/rust-lang/crates.io-index':
                name, version = package['name'], package['version']
                output.write(f'- {name} {version}: https://static.crates.io/crates/{name}/{name}-{version}.crate\n')
        output.write('\n'+input_marker(ROOT)+'\n')
    destination.write_text(share_html_notices(destination.read_text(encoding='utf-8')), encoding='utf-8', newline='\n')


def check_jemalloc_notice(packages):
    # The Rust bindings' license does not cover the native allocator's COPYING.
    # A dependency update must also review that version's native notice.
    records = json.loads((ROOT/'licenses/components/sources.json').read_text(encoding='utf-8'))
    reviewed = {record.get('crate') for record in records}
    for package in packages:
        if package['name'] == 'tikv-jemalloc-sys':
            component = f"{package['name']}-{package['version']}"
            if component not in reviewed:
                raise ValueError(f'Review and bundle jemalloc/COPYING for {component} in licenses/components')


if __name__ == '__main__':
    main()
