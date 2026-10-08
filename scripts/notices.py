"""Combine release notices into the one file accepted by old upgraders."""
import argparse
import subprocess
import tomllib
from pathlib import Path

import ripgrep

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--syntax-notices', type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['cargo', 'about', 'generate', '--locked', '--offline', '--fail',
                    str(ROOT/'about.hbs'), '--output-file', str(args.output)], cwd=ROOT, check=True)
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    standard = sysroot/'share/doc/rust/COPYRIGHT-library.html'
    # rust-docs supplies the notices for the same toolchain that built the binary.
    copyright_text = standard.read_text(encoding='utf-8')
    # Static Linux releases also carry musl. Rust's library-only notices do
    # not cover its bundled native runtimes; the toolchain's full notices do.
    native_copyright = (sysroot/'share/doc/rust/COPYRIGHT.html').read_text(encoding='utf-8')
    packages = tomllib.loads((ROOT/'Cargo.lock').read_text(encoding='utf-8'))['package']
    patch_notice = (ROOT/'crates/fastctx-hands/src/write/patch/NOTICE').read_text(encoding='utf-8')
    with args.output.open('a', encoding='utf-8', newline='\n') as output:
        output.write(ripgrep.notices())
        output.write('\n'+args.syntax_notices.read_text(encoding='utf-8'))
        output.write('\n## OpenAI Codex portions\n\n'+patch_notice)
        output.write('\n## Rust standard library\n\n'+copyright_text+'\n')
        output.write('\n## Rust toolchain and bundled native runtime notices\n\n'+native_copyright+'\n')
        # MPL dependencies need a source location, not just a license text.
        # Registry archives identify the exact locked dependency versions.
        output.write('\n## Dependency source archives\n\n'
                     'Source archives for the crates.io dependencies recorded by this build follow. '
                     'Each dependency retains the license stated in its source and in the notices above.\n\n')
        for package in sorted(packages, key=lambda package: (package['name'], package['version'])):
            if package.get('source') == 'registry+https://github.com/rust-lang/crates.io-index':
                name, version = package['name'], package['version']
                output.write(f'- {name} {version}: https://static.crates.io/crates/{name}/{name}-{version}.crate\n')


if __name__ == '__main__':
    main()
