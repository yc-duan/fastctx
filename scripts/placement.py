"""Real-node placement drill. The model and every network endpoint live on isolated loopback.

Run with --kernel, --drill and --codex absolute executable paths. Only the outer launcher
uses sudo, to enter private network, mount and PID namespaces; it installs no user or service.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from model_mock import Mock
from placement_model import GoalMock


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("kernel", "drill", "codex"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--root", type=Path)
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    return parser.parse_args()


def mount_file(base, name, target, body):
    path = base / name
    path.write_text(body)
    subprocess.run(["mount", "--bind", str(path), target], check=True)


def verify(requests, expectations):
    checked = 0
    def check(condition, message):
        nonlocal checked
        if not condition:
            raise AssertionError(message)
        checked += 1
        print("PASS", message, flush=True)

    serialized = [json.dumps(request["body"], ensure_ascii=False) for request in requests]
    for account in ("alice", "bob", "owner", "mobile"):
        marker = account.upper() + "_OPEN"
        matching = [body for body in serialized if marker in body]
        check(bool(matching), f"the real host sent {account}'s model input")
        for other in ("alice", "bob", "owner", "mobile"):
            if account != other:
                check(all(other.upper() + "_SKILL_DESCRIPTION" not in body for body in matching),
                      f"{account}'s input excludes {other}'s skills")
        check(any(account.upper() + "_SKILL_DESCRIPTION" in body for body in matching),
              f"{account}'s input contains its account's skill index")
    alice = [body for body in serialized if "ALICE_OPEN" in body]
    check(any(expectations["old"] in body for body in alice), "the assisted model received the working-computer plan path")
    check(all(expectations["owner_plan"] not in body for body in alice), "the assisted model never received the owner's plan path")
    check(any("ALICE_UI_CANARY" in body for body in alice if "ALICE_READ_AFTER_UI" in body), "a hub-edited plan reached the real model input")
    check(any(expectations["new"] in body for body in alice if "ALICE_NEW_PLACE" in body), "the next turn received its replacement working path")
    outputs = [item.get("output", "") for request in requests for item in request["body"].get("input", [])
               if isinstance(item, dict) and item.get("type") == "function_call_output"]
    outputs = json.dumps(outputs, ensure_ascii=False)
    check("ALICE_SKILL_EXECUTED" in outputs and "ALICE_REFERENCE" in outputs,
          "a real model tool call read auxiliary files and executed the skill script on the working computer")
    colleagues = [body for body in serialized if "COLLEAGUE_CASE" in body]
    check(bool(colleagues), "a real delegated conversation reached the model")
    check(any("OFFLINE_EDIT" in body for body in colleagues), "the colleague received its requester's current plan from the hub")
    check(all("BOB_SKILL_DESCRIPTION" not in body and "OWNER_SKILL_DESCRIPTION" not in body for body in colleagues),
          "the delegated conversation retained its account isolation")
    memory = [body for body in serialized if "ALICE_MEMORY_CASE" in body]
    check(bool(memory) and "Specs (" in memory[0] and "on alice-two)" in memory[0] and "placement-contract: Placement fixture" in memory[0],
          "the assisted model received its working-computer project-memory map")
    check("ALICE_JOURNAL_CANARY" in outputs and "ALICE_SPEC_CANARY" in outputs,
          "a real model tool read both project-memory documents on the working computer")
    print(f"MODEL INPUT: {checked} checks passed", flush=True)


def inside(args):
    base = args.root
    subprocess.run(["mount", "--make-rprivate", "/"], check=True)
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    for name in ("bin", "user", "codex", "claude"):
        (base / name).mkdir(parents=True, exist_ok=True)
    for name, source in (("codex", args.codex), ("fastctx", args.kernel)):
        (base / "bin" / name).symlink_to(source)
    path = f"{base}/bin:/usr/bin:/bin"
    if shutil.which("claude", path=path):
        raise RuntimeError("the drill PATH must not contain Claude")
    passwd = Path("/etc/passwd").read_text().splitlines()
    for index, line in enumerate(passwd):
        fields = line.split(":")
        if fields[2] == "0":
            fields[5] = str(base / "user")
            passwd[index] = ":".join(fields)
    mount_file(base, "passwd", "/etc/passwd", "\n".join(passwd) + "\n")
    mount_file(base, "environment", "/etc/environment", f"PATH={path}\nLANG=C.UTF-8\n")
    mount_file(base, "profile", "/etc/profile", f"export PATH={path}\n")
    mock = Mock()
    goal = GoalMock()
    (base / "codex/config.toml").write_text(
        f'model="mock-model"\nmodel_provider="mock"\n[model_providers.mock]\nname="mock"\n'
        f'base_url="{mock.base_url}"\nwire_api="responses"\nrequires_openai_auth=false\n'
        'request_max_retries=0\nstream_max_retries=0\n')
    try:
        subprocess.run([str(args.drill), str(base), str(args.kernel), goal.endpoint], check=True, timeout=900,
                       env={"PATH": path, "HOME": str(base / "user"), "LANG": "C.UTF-8", "TMPDIR": str(base)})
        verify(mock.requests, json.loads((base / "expectations.json").read_text()))
        if goal.errors:
            raise AssertionError(goal.errors)
        goal_text=json.dumps(goal.requests)
        if "WORLD_FACTS_READ" not in goal_text or "PLACEMENT_GOAL_CANARY" not in goal_text:
            raise AssertionError("the goal sentinel did not receive the copied full evidence")
        print("PASS the goal model received full working-computer evidence through the real look tool",flush=True)
    finally:
        (base / "requests.json").write_text(json.dumps(mock.requests, ensure_ascii=False, indent=2))
        (base / "goal-requests.json").write_text(json.dumps(goal.requests,ensure_ascii=False,indent=2))
        (base / "goal-errors.json").write_text(json.dumps(goal.errors))
        goal.close()
        mock.close()
        print("ARTIFACTS", base, flush=True)


def main():
    args = arguments()
    for name in ("kernel", "drill", "codex"):
        path = getattr(args, name).resolve(strict=True)
        if not path.is_file():
            raise ValueError(f"{name} is not a file")
        setattr(args, name, path)
    if args.inside:
        inside(args)
        return
    args.root = args.root.resolve() if args.root else Path(tempfile.mkdtemp(prefix="placement-", dir=os.environ.get("RUNNER_TEMP")))
    args.root.mkdir(parents=True, exist_ok=True)
    command = ["unshare", "--net", "--mount", "--pid", "--fork", "--kill-child", "--mount-proc", sys.executable,
               str(Path(__file__).resolve()), "--inside", "--root", str(args.root)]
    for name in ("kernel", "drill", "codex"):
        command.extend(["--" + name, str(getattr(args, name))])
    subprocess.run(([] if os.geteuid() == 0 else ["sudo"]) + command, check=True)


if __name__ == "__main__":
    main()
