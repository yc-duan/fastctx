"""Archive a prepared desktop application for bundling in another job of the same run.

Only the finished executable, frontend and generated notices cross the job boundary.
The kernel and Linux archives arrive separately from their build jobs; no stand-ins or
Cargo caches belong to this artifact. Both jobs use the same host and target triples.
"""
import argparse
import subprocess
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('target')
    parser.add_argument('archive', type=Path)
    args = parser.parse_args()
    host = next(line.removeprefix('host: ') for line in subprocess.check_output(
        ['rustc', '+1.92.0', '-vV'], text=True).splitlines() if line.startswith('host: '))
    directory = ROOT/'gui/src-tauri/target'
    if args.target != host:
        directory /= args.target
    program = directory/'release'/('fastctx-app.exe' if 'windows' in args.target else 'fastctx-app')
    paths = [program, ROOT/'gui/dist', ROOT/'gui/src-tauri/licenses']
    for path in paths:
        if not path.exists():
            raise FileNotFoundError(f'The application was not prepared: {path}')
    args.archive.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(args.archive, 'w:gz', compresslevel=1) as archive:
        for path in paths:
            archive.add(path, arcname=path.relative_to(ROOT).as_posix())
    print(f'Prepared {args.target}: {args.archive} ({args.archive.stat().st_size} bytes)')


if __name__ == '__main__':
    main()
