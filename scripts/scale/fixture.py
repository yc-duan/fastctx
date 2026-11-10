"""Deterministic, bounded-memory source fixtures for the scale read drill."""
import base64
import hashlib
import json
from pathlib import Path
import struct
import zlib
from manifest import FORMAT_VERSION, write_manifest

MIB = 1 << 20
GIB = 1 << 30
SEED = 20261108
STAMP = "2026-11-08T00:00:00Z"
PROFILES = {
    "quick": dict(conversations=50, rows=100_000, hot_rows=30_000, row_bytes=4096,
                  long_line=28_700_000, output_bytes=512 * MIB, attachment_bytes=256 * MIB, image_side=2048),
    "full": dict(conversations=500, rows=13_333_336, hot_rows=400_000, row_bytes=640,
                 long_line=287_000_000, output_bytes=10 * GIB, attachment_bytes=256 * MIB, image_side=4096),
}


def encoded(value):
    return json.dumps(value, ensure_ascii=True, separators=(",", ":")).encode()


class Sink:
    """Hash and count bytes as they are written, without keeping file contents."""
    def __init__(self, root, relative, kind, entries):
        self.relative, self.kind, self.entries = relative, kind, entries
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        self.file = path.open("xb", buffering=MIB)
        self.hash = hashlib.sha256()
        self.size = self.lines = self.longest = self.tail = 0

    def write(self, data):
        self.file.write(data)
        self.hash.update(data)
        self.size += len(data)
        # Chunk boundaries do not change the measured longest line.
        parts = data.split(b"\n")
        if len(parts) > 1:
            self.longest = max(self.longest, self.tail + len(parts[0]) + 1,
                               max(map(len, parts[1:-1]), default=-1) + 1)
            self.lines += len(parts) - 1
            self.tail = len(parts[-1])
        else:
            self.tail += len(data)

    def close(self, **extra):
        self.file.close()
        entry = dict(path=self.relative, kind=self.kind, bytes=self.size, sha256=self.hash.hexdigest(), **extra)
        if self.kind in ("claude", "subagent"):
            entry.update(lines=self.lines, longest_line=max(self.longest, self.tail))
        self.entries.append(entry)
        return entry


def noise(seed, key, size):
    return hashlib.shake_256(f"{seed}:{key}".encode()).hexdigest((size + 1) // 2).encode()[:size]


def padded(sink, value, size, seed, key):
    before, after = encoded(value).split(b'"FILL"')
    needed = size - len(before) - len(after) - 3
    if needed < 0:
        raise ValueError(f"row budget too small: {size} for {key}")
    sink.write(before + b'"')
    block = noise(seed, key, min(65536, needed))
    while needed:
        part = block[:needed]
        sink.write(part)
        needed -= len(part)
    sink.write(b'"' + after + b"\n")


def row(session, index, agent=None):
    # Negative prefix indices keep the newest 30,000 rows identical at 1x and 10x.
    ident = f"{session}:{index}"
    phase = index % 4
    value = dict(type="user" if phase in (0, 2) else "assistant", uuid=ident, timestamp=STAMP)
    if agent:
        value.update(agentId=agent, isSidechain=True)
    if phase == 0:
        content = "FILL"
    elif phase == 1:
        content = [dict(type="tool_use", id=f"call:{session}:{index}", name="Bash", input=dict(command="FILL"))]
    elif phase == 2:
        content = [dict(type="tool_result", tool_use_id=f"call:{session}:{index - 1}", content="FILL", is_error=False)]
        value["sourceToolAssistantUUID"] = f"{session}:{index - 1}"
    else:
        content = [dict(type="text", text="FILL")]
    value["message"] = dict(role=value["type"], content=content)
    return value


def png(root, side, seed, entries):
    sink = Sink(root, "assets/picture.png", "image", entries)
    sink.write(b"\x89PNG\r\n\x1a\n")

    def chunk(tag, payload):
        sink.write(struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", zlib.crc32(tag + payload)))

    chunk(b"IHDR", struct.pack(">IIBBBBB", side, side, 8, 2, 0, 0, 0))
    compressor = zlib.compressobj(0)
    for y in range(side):
        # SHAKE gives deterministic, incompressible RGB pixels, one scanline at a time.
        pixels = hashlib.shake_256(f"{seed}:png:{y}".encode()).digest(side * 3)
        part = compressor.compress(b"\0" + pixels)
        if part:
            chunk(b"IDAT", part)
    chunk(b"IDAT", compressor.flush())
    chunk(b"IEND", b"")
    return sink.close(width=side, height=side)


def image_row(sink, root, session):
    value = dict(type="user", uuid=f"{session}:picture", timestamp=STAMP,
                 message=dict(role="user", content=[dict(type="text", text="scale image"),
                     dict(type="image", source=dict(type="base64", media_type="image/png", data="IMAGE"))]))
    before, after = encoded(value).split(b'"IMAGE"')
    sink.write(before + b'"')
    with (root / "assets/picture.png").open("rb") as source:
        while part := source.read(3 * 16384):
            sink.write(base64.b64encode(part))
    sink.write(b'"' + after + b"\n")


def spawn_rows(sink, session, children):
    for index, agent in enumerate(children):
        call = f"spawn:{session}:{index}"
        use = dict(type="assistant", uuid=call, timestamp=STAMP, message=dict(role="assistant", content=[
            dict(type="tool_use", id=call, name="Agent", input=dict(subagent_type="general-purpose",
                 description=f"Synthetic child {index}", prompt="Read synthetic data"))]))
        result = dict(type="user", uuid=call + ":result", timestamp=STAMP,
                      message=dict(role="user", content=[dict(type="tool_result", tool_use_id=call, content="done")]),
                      toolUseResult=dict(agentId=agent, status="completed", content=[dict(type="text", text="done")]))
        sink.write(encoded(use) + b"\n")
        sink.write(encoded(result) + b"\n")


def blob(root, relative, size, seed, kind, entries):
    sink = Sink(root, relative, kind, entries)
    block = noise(seed, relative, MIB - 1) + b"\n"
    remaining = size
    while remaining:
        part = block[:remaining]
        sink.write(part)
        remaining -= len(part)
    return sink.close()


def generate(root, profile="quick", history=1, seed=SEED):
    if history not in (1, 10):
        raise ValueError("history must be 1 or 10")
    if root.exists():
        raise ValueError("fixture root must not exist; never overwrite a dataset")
    settings = dict(PROFILES[profile])
    # Full is already the 10-million-row stress case; its 10x can be generated on a larger disk.
    count = settings["conversations"]
    main_rows = settings["rows"] * history * 56 // 100 // 4 * 4
    side_rows = settings["rows"] * history - main_rows
    hot_rows = settings["hot_rows"] * history
    if hot_rows > main_rows:
        raise ValueError("hot conversation exceeds total rows")
    root.mkdir(parents=True)
    entries, conversations = [], []
    picture = png(root, settings["image_side"], seed, entries)
    main_bytes = 0
    for number in range(count):
        session = f"scale-{number:04d}"
        resident = f"claude-{number:08x}"
        rows = hot_rows if number == 0 else ((main_rows - hot_rows) // (count - 1) // 4 * 4)
        if number == count - 1:
            rows = main_rows - hot_rows - ((main_rows - hot_rows) // (count - 1) // 4 * 4) * (count - 2)
        children = [f"child-{number:04d}-{child}" for child in range(2)]
        relative = f"claude/{session}.jsonl"
        sink = Sink(root, relative, "claude", entries)
        spawn_rows(sink, session, children)
        if number == 0:
            image_row(sink, root, session)
        width = 4096 if number == 0 else settings["row_bytes"]
        for index in range(settings["hot_rows"] - rows, settings["hot_rows"]):
            padded(sink, row(session, index), width, seed, f"{session}:{index}")
        entry = sink.close(records=rows + 4 + int(number == 0), expected_events=rows // 4 * 3 + 6 + int(number == 0),
                           uuids=rows + 4 + int(number == 0))
        main_bytes += entry["bytes"]
        conversations.append(dict(resident=resident, session=session, path=relative, children=children,
                                  rows=rows, expected_events=entry["expected_events"],
                                  colleague_of="claude-00000000" if 1 <= number <= 4 else None))
    # Calibrate sidechain bytes against the production 1.12 / (2.52 - 1.12) ratio.
    target_side_bytes = main_bytes * 4 // 5
    remaining_rows, remaining_bytes = side_rows, target_side_bytes
    for number, conversation in enumerate(conversations):
        for child_index, agent in enumerate(conversation["children"]):
            remaining_files = (count - number) * 2 - child_index
            rows = remaining_rows if remaining_files == 1 else remaining_rows // remaining_files // 4 * 4
            size = remaining_bytes if remaining_files == 1 else remaining_bytes // remaining_files
            width, surplus = divmod(size, rows)
            relative = f"claude/{conversation['session']}/subagents/agent-{agent}.jsonl"
            sink = Sink(root, relative, "subagent", entries)
            for index in range(rows):
                padded(sink, row(agent, index, agent), width + int(index < surplus), seed, f"{agent}:{index}")
            sink.close(records=rows, expected_events=rows // 4 * 3, uuids=rows, agent=agent)
            meta = dict(toolUseId=f"spawn:{conversation['session']}:{child_index}", agentType="general-purpose",
                        description=f"Synthetic child {child_index}", requestShape="foreground")
            sink = Sink(root, relative.replace(".jsonl", ".meta.json"), "metadata", entries)
            sink.write(encoded(meta) + b"\n")
            sink.close()
            remaining_rows -= rows
            remaining_bytes -= size
    # A separate source keeps the pathological single line visible without changing the hot tail.
    sink = Sink(root, "claude/long-line.jsonl", "claude", entries)
    padded(sink, row("long-line", 1), 512, seed, "long-use")
    padded(sink, row("long-line", 2), settings["long_line"], seed, "long-result")
    sink.close(records=2, expected_events=1, uuids=2, adversarial=True)
    blob(root, "assets/attachment.bin", settings["attachment_bytes"], seed, "attachment", entries)
    blob(root, "jobs/output.log", settings["output_bytes"], seed, "job-output", entries)
    for index in range(8):
        sink = Sink(root, f"skills/scale-{index}/SKILL.md", "skill", entries)
        sink.write(f"---\nname: scale-{index}\ndescription: Synthetic scale fixture\n---\nRead the synthetic fixture.\n".encode())
        sink.close()
    total = sum(entry["bytes"] for entry in entries)
    manifest = dict(format=FORMAT_VERSION, seed=seed, profile=profile, history=history, settings=settings,
                    conversations=conversations, files=entries, total_bytes=total,
                    workload_rows=settings["rows"] * history, main_bytes=main_bytes,
                    subagent_bytes=target_side_bytes, subagent_fraction=target_side_bytes / (main_bytes + target_side_bytes),
                    jobs=500 * history, picture=picture, newest_row=settings["hot_rows"] - 1,
                    expected_events=sum(entry.get("expected_events", 0) for entry in entries),
                    not_implemented=["codex app-server ingestion", "new-device account synchronization"])
    write_manifest(root, manifest)
    return manifest
