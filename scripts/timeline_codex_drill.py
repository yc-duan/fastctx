"""Timeline scenarios using a private copy of the existing offline fake app-server."""

from concurrent.futures import ThreadPoolExecutor
import json
import shutil
import time

from timeline_lab import put_json


def turns(count, start=0):
    return [{"id": f"turn-{index:06d}", "status": "completed", "error": None,
             "startedAt": index * 2 + 1, "completedAt": index * 2 + 2,
             "items": [{"type": "userMessage", "id": f"user-{index}",
                        "content": [{"type": "text", "text": f"question {index}"}]},
                       {"type": "agentMessage", "id": f"reply-{index}", "text": f"answer {index} 中文"}]}
            for index in range(start, start + count)]


def prepare(lab, fake_host):
    destination = lab.root / "fake-codex"
    destination.mkdir()
    for name in ("fake_app_server.py", "socket_transport.py"):
        shutil.copy2(fake_host / name, destination / name)
    source = destination / "fake_app_server.py"
    text = source.read_text(encoding="utf-8")
    needle = '    elif method == "thread/read":\n        reply({"thread": snapshot(params["threadId"])})'
    replacement = ('    elif method == "thread/read":\n'
                   '        records.clear()\n'
                   '        records.update(json.loads(store.read_text(encoding="utf-8")).get("records", {}))\n'
                   '        reply({"thread": snapshot(params["threadId"])})')
    if text.count(needle) != 1 or text.count('"updatedAt": int(time.time())') != 1:
        raise RuntimeError("the existing fake app-server no longer matches the drill's two fixture adaptations")
    # Only the private copy reloads fixtures and gives a stable revision between edits.
    source.write_text(text.replace(needle, replacement).replace('"updatedAt": int(time.time())',
                      '"updatedAt": json.loads(store.read_text(encoding="utf-8"))["updated_at"]'), encoding="utf-8")
    launcher = destination / "codex.cmd"
    launcher.write_text('@python -u "%~dp0fake_app_server.py" %*\r\n', encoding="ascii")
    (lab.codex / "config.toml").write_text(
        "[mcp_servers.node_repl.env]\nCODEX_CLI_PATH = '" + launcher.as_posix() + "'\n", encoding="utf-8")
    applied = json.loads((lab.home / "applied.json").read_text(encoding="utf-8"))
    applied["homes"].append({"host": "codex", "home": str(lab.codex), "receipt": {
        "version": 1, "claude_md": None, "settings": None, "created": {"files": [], "directories": []},
        "codex": {"version": 1, "config_path": str(lab.codex / "config.toml"), "config_existed": True,
                  "server": {"body": [], "prefix": [], "inline": False}, "limit": None, "guidance": None}}})
    put_json(lab.home / "applied.json", applied)
    return destination


def run(drill, fake_host):
    lab = drill.lab
    prepare(lab, fake_host)
    resident = "codex-cc11"
    empty = "codex-ee00"
    session = lab.seed(resident, [], agent="codex")
    empty_session = lab.seed(empty, [], agent="codex")
    data = turns(250)
    store = lab.codex / "fake-threads.json"

    generation = 0

    def save():
        nonlocal generation
        generation += 1
        put_json(store, {"updated_at": 1794340000 + generation, "records": {session: data, empty_session: []},
                         "settings": {session: {"cwd": str(lab.root)}, empty_session: {"cwd": str(lab.root)}}})

    save()
    lab.start()
    drill.build_compare("codex-empty", empty)
    drill.build_compare("codex-first-build", resident)
    drill.benchmark(resident)
    build = lab.settled_build(resident)
    drill.result("codex-unchanged-write", build["before"] == build["after"], **build)
    old = lab.stats(resident)
    data.extend(turns(1, start=250))
    drill.build_compare("codex-append-turn", resident, save)
    added = lab.stats(resident)["records"] - old["records"]
    # Four turn events are appended; the changed thread metadata replaces one event.
    drill.result("codex-append-write-count", added == 6, appended_events=4, metadata_replacements=1, new_records=added)
    data[100]["items"][1]["text"] = "middle changed"
    drill.build_compare("codex-middle-change", resident, save)
    del data[101]
    drill.build_compare("codex-middle-delete", resident, save)
    data[-1]["items"][1]["text"] = "codex-large🙂" * 40000
    drill.build_compare("codex-oversized", resident, save)
    data[-1]["items"].append({"type": "contextCompaction", "id": "compact-fixture"})
    drill.build_compare("codex-compaction", resident, save)
    data[:] = data[:30]
    drill.build_compare("codex-rollback", resident, save)
    before = lab.stats(resident)
    lab.stop()
    lab.start()
    drill.compare("codex-restart", resident)
    build = lab.settled_build(resident)
    drill.result("codex-restart-no-rewrite", before == build["after"], **build)
    revision = lab.stats(resident)["revision"]
    began = time.perf_counter()
    moved = lab.timeline(resident, "moved", after=revision)
    elapsed = time.perf_counter() - began
    drill.result("codex-moved-no-change", moved["revision"] == revision and 24 <= elapsed < 30,
                 before=revision, reply=moved, elapsed_s=elapsed)
    with ThreadPoolExecutor(max_workers=1) as executor:
        began = time.perf_counter()
        future = executor.submit(lab.timeline, resident, "moved", after=revision)
        time.sleep(1)
        data.extend(turns(1, start=300))
        save()
        lab.timeline(resident)
        moved = future.result(timeout=40)
    drill.result("codex-moved-change", moved["revision"] > revision,
                 before=revision, reply=moved, elapsed_s=time.perf_counter() - began)
    drill.compare("codex-after-moved", resident)
    drill.delete(resident, "codex-delete")
