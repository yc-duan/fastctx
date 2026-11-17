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


def wrap(unit, output, root, wrapper):
    """What became of `unit`: wrapped, or why not."""
    text = unit.read_bytes().decode("utf-8", "surrogateescape")
    if not text.startswith("# fastctx service v1:"):
        return "not a fastctx unit"
    command = next((line[len("ExecStart="):] for line in text.splitlines() if line.startswith("ExecStart=")), None)
    if not command:
        return "no ExecStart"
    arguments = list(words(command))
    if "--dir" not in arguments:
        return "no --dir in " + command
    directory = Path(arguments[arguments.index("--dir") + 1])
    if not directory.is_relative_to(root):
        return "another run's " + str(directory)
    prefix = ":" if command.startswith(":") else ""
    command = command.removeprefix(":")
    folder = output / (unit.name + ".d")
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "oldkernel.conf").write_text("[Service]\nExecStart=\nExecStart=" + prefix
        + quoted(wrapper) + " 314 " + command + "\n")
    return "wrapped"


def main():
    settings = json.loads(Path(sys.argv[1]).read_text())
    output = Path(sys.argv[2])
    root = Path(settings["root"])
    # Said where the test reads it back when a unit was not wrapped: one
    # unit that cannot be read leaves the others wrapped all the same.
    said = []
    for unit in sorted(Path("/etc/systemd/system").glob("fastctx-service-*.service")):
        try:
            said.append(unit.name + ": " + wrap(unit, output, root, settings["wrapper"]))
        except Exception as error:
            said.append(unit.name + ": " + type(error).__name__ + ": " + str(error))
    text = "-- " + " ".join(sys.argv[1:]) + "\n" + "".join(line + "\n" for line in said)
    # A generator may be kept from writing anywhere but its output.
    for log in (root / "generator.log", output / "fastctx-oldkernel.log"):
        try:
            with open(log, "a", encoding="utf-8") as file:
                file.write(text)
            break
        except OSError:
            continue


if __name__ == "__main__":
    main()
