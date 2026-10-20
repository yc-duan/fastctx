"""A test-only systemd generator, scoped to one smoke test's temporary root."""
import json
from pathlib import Path
import re
import sys


def words(command):
    for value in re.findall(r'"([^"\n]*)"', command):
        raw = re.sub(rb"\\x([0-9a-fA-F]{2})", lambda match: bytes([int(match[1], 16)]), value.encode())
        yield raw.decode("utf-8", "surrogateescape").replace("%%", "%").replace("$$", "$")


def quoted(value):
    return '"' + ''.join(chr(byte) if chr(byte).isalnum() and byte < 128 or byte in b"/._-" else f"\\x{byte:02x}" for byte in value.encode()) + '"'


def main():
    settings = json.loads(Path(sys.argv[1]).read_text())
    output = Path(sys.argv[2])
    root = Path(settings["root"])
    for unit in Path("/etc/systemd/system").glob("fastctx-service-*.service"):
        text = unit.read_text()
        if not text.startswith("# fastctx service v1:"):
            continue
        command = next((line[len("ExecStart="):] for line in text.splitlines() if line.startswith("ExecStart=")), None)
        if not command:
            continue
        arguments = list(words(command))
        if "--dir" not in arguments:
            continue
        directory = Path(arguments[arguments.index("--dir") + 1])
        if not directory.is_relative_to(root):
            continue
        prefix = ":" if command.startswith(":") else ""
        command = command.removeprefix(":")
        folder = output / (unit.name + ".d")
        folder.mkdir(parents=True, exist_ok=True)
        (folder / "oldkernel.conf").write_text("[Service]\nExecStart=\nExecStart=" + prefix
            + quoted(settings["wrapper"]) + " 314 " + command + "\n")


if __name__ == "__main__":
    main()
