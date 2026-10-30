"""Boundary samples for the admitted tool contract, including nested variants.

The decoder is taken from git, not implemented here. GUI wire fixtures supply
rich, shared world/page/attachment records; this corpus adds tool-only shapes.
"""

from copy import deepcopy
import json

AT = "2026-10-04T12:00:00Z"
ADDRESS = {"machine": "desktop", "resident": "codex-ab12"}
HOST = {"agent": "codex", "client": "ipc-contract", "version": "1"}
PEER = {"kind": "session", "host": HOST}
SENTINEL = {"sentinel": "s-0123456789abcdef", "generation": 1, "machine": "desktop", "at": AT, "due": AT,
            "condition": "the build finishes", "tell": "read its log",
            "event": {"kind": "satisfied", "reason": "it finished", "evidence": [
                {"id": "o-1", "at": 1790000000000, "source": "C:/build.log", "text": "done",
                 "exit_code": None, "full_path": None}]}}
# The World's facts after the five protocol 13 shipped with.
LATER_FACTS = ["delivery_failed", "woke"]
# The protocol each kind of host control's result came with; the rest came with 13.
CONTROLLED_SINCE = {"done": 5, "forked": 5, "reported": 10, "refused": 10, "model_menu": 10}


def frame(reply):
    return {"kind": "reply", "id": 37, "reply": {"Ok": reply}}


def replies(fixtures):
    for offers in fixtures["control"]["offers"]:
        yield {"reply": "offers", "resident": ADDRESS, "offers": offers}
    for controlled in fixtures["control"]["controlled"]:
        yield {"reply": "controlled", "resident": ADDRESS, "controlled": controlled}
    yield {"reply": "done"}
    yield {"reply": "server_invitation", "command": "(set -e;echo x86_64 unavailable >&2;exit 1)"}
    yield {"reply": "arrived", "resident": ADDRESS["resident"], "town": "project"}
    yield {"reply": "machines", "here": "desktop", "others": ["laptop"]}
    # Colleagues lent by another account: fields older readers ignore.
    yield {"reply": "machines", "here": "desktop", "others": ["laptop"],
           "colleagues": "gpu.acme", "colleagues_lent": True}
    yield {"reply": "machines", "here": "desktop", "others": ["laptop"], "collab": {
        "callees": [{"name": "deep", "account": "ada", "use_when": "Check a precise point", "lives_on": "laptop"}],
        "prompt": "The account's collaboration text"}}
    # A computer in no network offers its own callees, which work for no
    # account; an older reader is offered only those that do.
    yield {"reply": "machines", "here": "desktop", "others": [], "collab": {
        "callees": [{"name": "quick", "use_when": "Check one file", "lives_on": "desktop"},
                    {"name": "deep", "account": "ada", "use_when": "Check a precise point", "lives_on": "laptop"}],
        "prompt": "This computer's collaboration text"}}
    yield {"reply": "asked", "colleague": ADDRESS}
    for landed in [{"kind": "uncertain", "why": "interrupted"}, {"kind": "new_turn", "turn": "a"},
                   {"kind": "joined_turn", "turn": "b"}, {"kind": "waiting", "why": "offline", "answered": True},
                   {"kind": "sent"}, {"kind": "handed", "id": "notice-ab12"}]:
        yield {"reply": "spoken", "to": ADDRESS, "landed": landed}
    changes = [{"path": "C:/data/example.txt", "kind": kind, "before_hash": None,
                "after_hash": "a" * 64, "encoding": "UTF-8"} for kind in ["added", "modified", "deleted"]]
    yield {"reply": "written", "outcome": {"Ok": {"text": "committed", "changes": changes,
                                                    "changed_since_seen": ["C:/data/example.txt"]}}}
    yield {"reply": "written", "outcome": {"Err": {"message": "later section failed", "changes": changes}}}
    selected = deepcopy(changes)
    for change in selected:
        change["encoding_selection"] = "utf-8,gb18030"
    yield {"reply": "written", "outcome": {"Ok": {"text": "committed", "changes": selected,
                                                    "changed_since_seen": []}}}
    yield {"reply": "copied", "from": "C:/a", "from_machine": "desktop", "to": "C:/b", "to_machine": "laptop",
           "files": 1, "bytes": 9, "directories": 1, "links": 1, "unchanged": 1, "problems": ["missing file"]}
    pieces = [{"kind": "text", "first_line": 1, "text": "hello\n"},
              {"kind": "part_of_line", "line": 2, "from": 1, "to": 4, "of": 900, "text": "part"},
              {"kind": "omitted", "first_line": 3, "last_line": 6},
              {"kind": "image", "mime": "image/png", "width": 1, "height": 1, "base64": "AA=="},
              {"kind": "unreadable", "line": 7, "reason": "unsupported graphics"}]
    states = [{"kind": "ended", "status": {"code": 0}}, {"kind": "ended", "status": {"code": 3221225477}},
              {"kind": "exited", "status": {"signal": 9}, "remaining": 2}, {"kind": "running"},
              {"kind": "stopped"}, {"kind": "interrupted", "reason": "supervisor ended"},
              {"kind": "failed", "error": "spawn failed"}]
    for state in states:
        for legacy in [[], [{"encoding": "GBK", "lines": 2}],
                       [{"encoding": "GBK", "lines": 2}, {"encoding": "windows-1252", "lines": 3}]]:
            yield {"reply": "run", "machine": "desktop", "job": "j-abc123", "state": state,
                   "output": {"log": "C:/job.log", "bytes": 900, "lines": 7, "pieces": pieces,
                              "legacy": legacy, "escaped_lines": 1}, "visited": [["laptop", "j-def456"]]}
    # Why a call answered while its job ran: a field older readers ignore.
    for answered in [{"kind": "wait_over", "seconds": 0}, {"kind": "words", "seconds": 5}]:
        yield {"reply": "run", "machine": "desktop", "job": "j-abc123", "state": {"kind": "running"},
               "output": {"log": "C:/job.log", "bytes": 0, "lines": 0, "pieces": [], "legacy": [],
                          "escaped_lines": 0}, "answered": answered}
    yield {"reply": "run", "machine": "desktop", "job": "j-abc123", "state": {"kind": "ended", "status": {"code": 0}},
           "output": {"log": "C:/job.log", "bytes": 0, "lines": 0, "pieces": [], "legacy": [], "escaped_lines": 0},
           "consumed": ["desktop-j-def456", "letter-consumed456"]}
    notices = []
    for how in [{"kind": "finished"}, {"kind": "stopped"}, {"kind": "interrupted", "reason": "restart"},
                {"kind": "failed", "error": "disk full"}]:
        notices.append({"kind": "job_ended", "job": "j-abc123", "machine": "desktop", "how": how,
                        "status": {"code": 17}, "at": AT, "output": "C:/job.log",
                        "last": {"text": "last output\n", "whole": False}})
    for how in ["completed", "failed", "interrupted", "provider-other"]:
        notices.append({"kind": "answer", "colleague": ADDRESS, "label": "task", "turn": "turn-1",
                        "end": {"how": how, "reason": "reason", "error": {"code": "err", "from": {"kind": "page"}, "owner": {"page": "opaque host detail"}},
                                "duration_ms": 123, "last_message": "answer",
                                "facts": {"models": ["reported-model"], "efforts": [{"kind": "requested", "value": "low"}],
                                          "tokens": {"host": "claude", "input_tokens": 10, "output_tokens": 20,
                                                     "cache_read_input_tokens": 30, "cache_creation_input_tokens": 40}}}})
    for kind in ["reconnecting", "safety", "rerouted", "tools_missing", "tool_server", "warning", "error"]:
        notices.append({"kind": "health", "colleague": ADDRESS, "label": "task", "health": {"kind": kind, "text": kind}})
    for speaker in [{"kind": "user", "machine": "desktop"}, {"kind": "resident", "address": ADDRESS},
                    {"kind": "world"}, fixtures["pages"]["speaker"]]:
        notices.append({"kind": "words", "from": speaker, "text": "verbatim words"})
    for fact in [{"fact": "desk_taken", "by": ADDRESS}, {"fact": "desk_outside"}, {"fact": "desk_free", "by": ADDRESS},
                 {"fact": "moved", "to": "project"}, {"fact": "delivery_failed", "id": "notice-ab12"},
                 {"fact": "woke", "why": {"kind": "due"}}]:
        notices.append({"kind": "world", "fact": fact, "text": "the World's words"})
    notices.append({"kind": "sentinel", "observation": SENTINEL})
    for i, notice in enumerate(notices):
        yield {"reply": "notices", "notices": [{"id": f"notice-{i}", "at": AT, "notice": notice}], "notes": ["town note"]}
    yield next(r for r in fixtures["attachments"]["replies"] if r["reply"] == "notices")
    for who in fixtures["world"]["who"]:
        yield {"reply": "who", **who}
    for nodes in fixtures["world"]["nodes"]:
        yield {"reply": "nodes", **nodes}
    yield {"reply": "towns", "towns": fixtures["world"]["towns"]}
    yield {"reply": "journals", "town": "project", "journals": fixtures["world"]["journals"]}
    page_map = fixtures["pages"]["map"]
    for writer in [{"kind": "user"}, {"kind": "resident", "address": ADDRESS},
                   {"kind": "session", "agent": "claude"}, fixtures["pages"]["writer"],
                   {"kind": "elsewhere", "page": False}, {"kind": "elsewhere", "page": True}]:
        record = {**page_map["record"], "by": writer}
        stamp = {"by": writer, "on": "desktop", "at": AT}
        # As told to whoever may not work on the machine it was written on.
        if writer["kind"] == "elsewhere":
            record.pop("on", None)
            stamp.pop("on")
        yield {"reply": "record", "town": "project", "record": record, "opened": stamp}
        for canon in [None, {"Ok": [{"file": "design.md", "title": "Design", "scope": "system"}]}, {"Err": "missing"}]:
            mapping = {**page_map, "record": record, "opened": stamp, "canon": canon}
            yield {"reply": "map", "map": mapping}
            yield {"reply": "entered", "town": "project", "map": mapping}
        attachments = [{"name": "file", "kind": kind, "bytes": 17, "sha256": "b" * 64, "kept": kept}
                       for kind in ["text", "image", "pdf", "binary"]
                       for kept in [{"kind": "in_entry"}, {"kind": "on_machine", "location": {"machine": "desktop", "path": "C:/file"}}]]
        entry = {"id": "20261004T120000Z-desktop", "at": AT, "machine": "desktop", "by": writer,
                 "agent": HOST, "model": {"name": "model", "self_reported": True}, "session": "thread",
                 "cites": ["20261003T120000Z-desktop"], "attachments": attachments}
        yield {"reply": "entry_added", "town": "project", "journal": "notes", "id": entry["id"], "attachments": attachments}
        yield {"reply": "journal", "view": {"town": "project", "journal": "notes", "directory": "C:/notes",
               "description": {"version": 1, "text": "notes", "by": writer, "on": "desktop", "at": AT}, "first": 1,
               "entries": [{"stamp": entry, "text": "entry", "mentioned_by": []}], "total": 1, "writers": 1}}
    for reply in fixtures["pages"]["replies"]:
        if reply["reply"] in ["page", "pages"]:
            yield reply
    record = {"id": "j-abc123", "machine": "desktop", "owner": ADDRESS, "town": "project", "script": "echo hello",
              "cwd": "C:/data", "output": "C:/job.log", "started_at": AT, "supervisor": None, "pid": 17,
              "exit": {"status": {"code": 0}, "at": AT}, "output_ended_at": AT, "remaining": 0,
              "output_lost": {"kept_bytes": 5, "error": "disk full"}, "end": {"at": AT, "how": {"kind": "finished"}},
              "ends_with_node": False, "stderr": "C:/job.err", "stderr_lost": {"kept_bytes": 4, "error": "full"}}
    resident = fixtures["world"]["who"][0]["machines"][0]["reply"]["residents"][0]
    yield {"reply": "waited", "records": [{**record, "owner": ADDRESS}]}
    jobs = fixtures["world"]["who"][0]["machines"][0]["reply"]["jobs"]
    for owner in [ADDRESS, fixtures["pages"]["job_owner"]]:
        yield {"reply": "looked", "looked": [{"id": "j-abc123", "found": [{"kind": "job", "record": {**record, "owner": owner}}], "unasked": ["offline"]}]}
        who = deepcopy(fixtures["world"]["who"][0])
        who["machines"][0]["reply"]["jobs"][0]["owner"] = owner
        yield {"reply": "who", **who}
    for speaker in [{"kind": "world"}, fixtures["pages"]["speaker"]]:
        detail = {"resident": resident, "records": "C:/resident", "actions": [resident["last_action"]],
                  "actions_total": 1, "asked": "task", "answers": [{"at": AT, "kind": "heard", "from": speaker, "turn": "turn-1", "text": "words"}], "jobs": jobs}
        yield {"reply": "looked", "looked": [{"id": ADDRESS["resident"], "found": [{"kind": "resident", **detail}], "unasked": []}]}


def calls(fixtures):
    yield {"call": "arrive", "host": HOST, "hints": {"resident": None, "thread": "test", "was": None}}, PEER, True
    yield {"call": "run", "resident": ADDRESS["resident"], "request": {"script": "echo hello", "cwd": None}, "node": None}, PEER, True
    for op in [{"tool": "write", "file_path": "C:/a", "content": "hello"},
               {"tool": "edit", "file_path": "C:/a", "old_string": "a", "new_string": "b", "replace_all": True},
               {"tool": "apply_patch", "input": "*** Begin Patch\n*** End Patch"}]:
        yield {"call": "write", "resident": ADDRESS["resident"], "op": op, "node": None}, PEER, True
    yield {"call": "copy", "from": {"machine": "desktop", "path": "C:/a"}, "to": {"machine": "laptop", "path": "C:/b"}}, PEER, True
    yield {"call": "ask", "resident": ADDRESS["resident"], "label": "test", "prompt": "test", "node": None}, PEER, True
    yield {"call": "message", "resident": ADDRESS["resident"], "to": "codex-ab34", "machine": None, "text": "test"}, PEER, True
    yield {"call": "collect", "resident": ADDRESS["resident"]}, PEER, True
    yield {"call": "taken", "resident": ADDRESS["resident"], "ids": ["notice-a"]}, PEER, True
    yield {"call": "taken", "resident": ADDRESS["resident"], "ids": [], "shown": None,
           "completed": {"machine": "desktop", "job": "j-abc123", "output": "C:/job.log"}}, PEER, True
    for receipt in [False, True]:
        yield {"call": "wait", "jobs": ["j-abc123"], "receipt": receipt, "parent": "j-def456"}, {"kind": "command", "resident": ADDRESS, "session": None}, True
    yield {"call": "who", "all": True, "town": None}, PEER, True
    yield {"call": "look", "ids": [ADDRESS["resident"], "j-abc123"]}, PEER, True
    yield {"call": "saw", "owner": ADDRESS, "town": None, "seen": [{"path": "C:/a", "at": AT, "sha256": None, "size": 1, "modified": None, "encoding": "UTF-8"}]}, PEER, True
    yield {"call": "saw", "owner": ADDRESS, "town": None, "seen": [{"path": "C:/a", "at": AT, "sha256": None,
           "size": 1, "modified": None, "encoding": None, "encoding_selection": "utf-8,gb18030"}]}, PEER, True
    for action in ["new", "edit", "list", "show", "map", "enter"]:
        yield {"call": "project", "do": action, "town": "project", "fields": {}}, PEER, True
    for action in ["add", "describe", "read", "list"]:
        yield {"call": "journal", "do": action, "town": "project", "journal": "notes", "text": "entry",
               "description": "notes", "cites": [], "attach": [], "model": None, "before": None}, PEER, True
    for name in ["nodes", "machines"]:
        for peer in [PEER, {"kind": "command", "resident": ADDRESS, "session": None},
                     {"kind": "command", "resident": None, "session": {"kind": "codex", "thread": "test"}}]:
            yield {"call": name}, peer, True
    for peer in [{"kind": "supervisor", "job": "j-abc123"}, {"kind": "supervisor", "job": "j-def456"}]:
        yield {"call": "job_changed", "job": "j-abc123", "owner": ADDRESS, "town": None}, peer, peer["job"] == "j-abc123"
        yield {"call": "nodes"}, peer, False
    yield {"call": "nodes"}, {"kind": "command", "resident": None, "session": None}, False
    yield {"call": "watch", "resident": ADDRESS["resident"], "machine": None}, PEER, False
    yield {"call": "since", "since": AT, "town": None}, PEER, False
    yield {"call": "agents", "machine": None}, PEER, False
    for call in fixtures["pages"]["calls"]:
        yield call, PEER, call["do"] in ["publish", "read", "list"]


def page_identity(value):
    if isinstance(value, dict):
        return any((key == "owner" and isinstance(item, dict) and "page" in item)
                   or (key in ["by", "from"] and isinstance(item, dict) and item.get("kind") == "page")
                   or page_identity(item) for key, item in value.items())
    return isinstance(value, list) and any(map(page_identity, value))


def elsewhere(value):
    """Whether a reply tells a writer as no more than a conversation or a
    page elsewhere, or a stamp without its machine: shapes no historical
    reader knows, said to them as text."""
    if isinstance(value, dict):
        if value.get("kind") == "elsewhere" and "page" in value:
            return True
        return any(map(elsewhere, value.values()))
    return isinstance(value, list) and any(map(elsewhere, value))


def samples(root):
    fixtures = json.loads((root / "gui/fixtures/wire.json").read_text(encoding="utf-8"))
    for protocol in [5, 6, 7, 8, 9, 10]:
        # New GUI reads are parsed by today's node and refused before use
        # on the frozen tool-adapter path; old clients cannot encode them.
        for call in [
            {"call": "read_attachment", "attachment": {"owner": ADDRESS, "host_image": {"host": "codex", "turn": "turn-1", "message": 0, "image": 0}}},
            {"call": "attachment_thumbnail", "attachment": {"owner": ADDRESS, "host_image": {"host": "claude", "message": "message-uuid", "image": 0}}, "max": 96},
            {"call": "thumbnail", "machine": "laptop", "request": {"path": "C:/a.png", "max": 96}},
            {"call": "lines", "machine": "laptop", "request": {"kind": "open", "path": "C:/a.txt", "decoding": {"kind": "file", "encoding": None}}},
            {"call": "read_lines", "machine": "laptop", "request": {"source": "line-source", "start": 0, "count": 5}},
        ]:
            yield {"mode": "current_request", "protocol": protocol, "peer": PEER, "admitted": False,
                   "value": {"kind": "call", "id": 37, "call": call}}
        for lifetime in [None, fixtures["world"]["nodes"][0]["facts"]["node_lifetime"]]:
            welcome = {"kind": "accepted", "machine": "desktop", "version": "1.0.0"}
            if lifetime is not None:
                welcome["node_lifetime"] = lifetime
            yield {"mode": "welcome", "protocol": protocol, "value": welcome}
        for reply in replies(fixtures):
            if protocol == 5 and reply["reply"] == "server_invitation":
                continue
            if reply["reply"] == "waited" and protocol < 10:
                continue
            if protocol == 5 and reply["reply"] in ["page", "pages", "machines"]:
                continue  # These calls did not exist in the frozen protocol-5 binary.
            case = {"mode": "reply", "protocol": protocol, "value": frame(reply)}
            case["collaboration"] = bool(reply.get("collab"))
            if reply["reply"] == "run":
                case["encodings"] = [part["encoding"] for part in reply["output"]["legacy"]]
                case["receipts"] = bool(reply.get("consumed"))
            case["attached"] = reply["reply"] == "notices" and reply["notices"][0]["notice"]["kind"] == "attached"
            # The World's facts are its words before protocol 11, and a
            # sentinel's finding to every reader that says no later notices.
            case["world"] = reply["reply"] == "notices" and reply["notices"][0]["notice"]["kind"] == "world"
            case["sentinel"] = reply["reply"] == "notices" and reply["notices"][0]["notice"]["kind"] == "sentinel"
            case["turn_facts"] = reply["reply"] == "notices" and reply["notices"][0]["notice"]["kind"] == "answer"
            # Hub-by-hub nodes have no shape before protocol 8 and are said as text.
            case["text_result"] = (protocol == 5 and reply["reply"] != "notices" and page_identity(reply)) or (reply["reply"] == "nodes" and protocol < 8) or (reply["reply"] == "controlled" and protocol < CONTROLLED_SINCE.get(reply["controlled"]["kind"], 13)) or elsewhere(reply)
            yield case
        yield {"mode": "reply", "protocol": protocol, "value": {"kind": "reply", "id": 37, "reply": {"Err": {"message": "not executed"}}}}
        if protocol >= 8:
            yield {"mode": "request", "protocol": protocol,
                   "peer": {"kind": "command", "resident": None, "session": None}, "admitted": protocol == 10,
                   "value": {"kind": "call", "id": 37, "call": {"call": "server_command", "hub": None,
                             "account": "ada", "name": None, "expires": AT}}}
        for call, peer, admitted in calls(fixtures):
            if call["call"] == "wait" and protocol < 10:
                continue
            if protocol == 10 and peer["kind"] == "command":
                admitted = True
            if protocol == 5 and call["call"] in ["page", "machines"]:
                continue
            yield {"mode": "request", "protocol": protocol, "peer": peer, "admitted": admitted,
                   "value": {"kind": "call", "id": 37, "call": call}}
    # The releases of protocol 13 read today's replies with the types they
    # shipped: a tool process outlives its node's change of program, so a
    # beta 1, 2 or 11 tool reads a later node's answers as it is. It says no
    # later notices, so a sentinel's finding and the later World facts are
    # words to it.
    for reader in ["v13_beta1", "v13_beta2", "v13_beta11"]:
        for lifetime in [None, fixtures["world"]["nodes"][0]["facts"]["node_lifetime"]]:
            welcome = {"kind": "accepted", "machine": "desktop", "version": "1.0.0"}
            if lifetime is not None:
                welcome["node_lifetime"] = lifetime
            yield {"mode": "welcome", "protocol": 13, "reader": reader, "value": welcome}
        for reply in replies(fixtures):
            notice = reply["notices"][0]["notice"] if reply["reply"] == "notices" else {}
            yield {"mode": "reply", "protocol": 13, "reader": reader, "value": frame(reply),
                   "sentinel": notice.get("kind") == "sentinel",
                   "world": notice.get("kind") == "world" and notice["fact"]["fact"] in LATER_FACTS,
                   "text_result": elsewhere(reply)}
        yield {"mode": "reply", "protocol": 13, "reader": reader,
               "value": {"kind": "reply", "id": 37, "reply": {"Err": {"message": "not executed"}}}}
