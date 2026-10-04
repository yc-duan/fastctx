"""Combine release notices into the one file accepted by old upgraders."""
import argparse
import subprocess
from pathlib import Path

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
    patch_notice = (ROOT/'crates/fastctx-hands/src/write/patch/NOTICE').read_text(encoding='utf-8')
    with args.output.open('a', encoding='utf-8', newline='\n') as output:
        output.write('\n'+args.syntax_notices.read_text(encoding='utf-8'))
        output.write('\n## OpenAI Codex portions\n\n'+patch_notice)
        output.write('\n## Rust standard library\n\n'+copyright_text+'\n')


if __name__ == '__main__':
    main()
