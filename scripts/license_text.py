"""Preserve every attribution while sharing byte-identical license texts."""
import hashlib
import json
import re


def component_notices(root):
    directory = root/'licenses/components'
    records = json.loads((directory/'sources.json').read_text(encoding='utf-8'))
    expected = {record['file'] for record in records}
    actual = {path.name for path in directory.glob('*.txt')}
    if actual != expected:
        raise ValueError(f'Supplemental license inventory differs: expected {expected}, found {actual}')
    for record in sorted(records, key=lambda item: item['file']):
        path = directory/record['file']
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != record['sha256']:
            raise ValueError(f'Review and record the changed upstream license: {path}')
        yield path.stem, data.decode('utf-8')


def input_marker(root):
    """Bind release notices to their lockfile and reviewed supplemental texts."""
    paths = [root/name for name in ('Cargo.lock', 'about.toml', 'about.hbs', 'LICENSE-APACHE', 'NOTICE',
             'crates/fastctx-hands/src/write/patch/NOTICE', 'crates/fastctx-pdf/src/NOTICE',
             'crates/fastctx-office/NOTICE', 'vendor/pdf_oxide/NOTICE', 'vendor/pdf_oxide/FASTCTX-PATCH.md')]
    paths += [path for path in (root/'licenses/components').iterdir() if path.is_file()]
    paths += [path for path in (root/'licenses/ripgrep-rust').iterdir() if path.is_file()]
    paths += [path for path in (root/'crates/fastctx-ripgrep/vendor').iterdir()
              if path.is_file() and path.suffix != '.gz']
    digest = hashlib.sha256()
    for path in sorted(paths, key=lambda path: path.relative_to(root).as_posix()):
        digest.update(path.relative_to(root).as_posix().encode()+b'\0'+path.read_text(encoding='utf-8').encode()+b'\0')
    return f'<!-- fastctx-notices-inputs: {digest.hexdigest()} -->'


def share_html_notices(document):
    """Replace repeated pre bodies with links; retain all surrounding provenance."""
    seen = {}

    def replace(match):
        body = match.group(1)
        identifier = 'license-' + hashlib.sha256(body.encode('utf-8')).hexdigest()
        if body in seen:
            return f'<p>Identical license text: <a href="#{identifier}">{identifier}</a>.</p>'
        seen[body] = identifier
        return f'<pre id="{identifier}">{body}</pre>'

    return re.sub(r'<pre(?: id="license-[0-9a-f]{64}")?>(.*?)</pre>', replace, document, flags=re.DOTALL)


def fenced(text):
    """Choose a fence that cannot terminate inside an upstream notice."""
    runs = re.findall(r'`+', text)
    fence = '`' * max(3, max(map(len, runs), default=0) + 1)
    return f'{fence}text\n{text}\n{fence}\n'


def gui_input_marker(root):
    """Bind GUI resources to its locks and the frontend actually bundled."""
    paths = [root/name for name in ('gui/package-lock.json', 'gui/src-tauri/Cargo.lock',
             'gui/dist/THIRD_PARTY_FRONTEND.md', 'about.toml', 'about.hbs', 'LICENSE-APACHE', 'NOTICE')]
    paths += [path for path in (root/'gui/licenses').rglob('*') if path.is_file()]
    paths += [path for path in (root/'gui/dist/pdfjs').rglob('LICENSE*') if path.is_file()]
    digest = hashlib.sha256()
    for path in sorted(paths, key=lambda path: path.relative_to(root).as_posix()):
        digest.update(path.relative_to(root).as_posix().encode()+b'\0'+path.read_bytes()+b'\0')
    return f'<!-- world-notices-inputs: {digest.hexdigest()} -->'
