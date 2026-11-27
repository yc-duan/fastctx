"""Writes, for every Claude Code ledger on this machine, the series the
reckoning replay reads (`reckoning.rs`, `replays_vendor_series`; `model.py`):
its expected week and swing, each 7-day reading (when, its week's end, tenths
used, account), each charge in quota dollars (Fable at two and a half times)
and each change of account. Only figures and times, never words; the ledgers
are only read.

    python3 extract.py OUT_FOLDER [USAGE_DIR]

USAGE_DIR defaults to ~/.fastctx/world/usage.
"""
import glob, json, os, sys
from datetime import datetime, timezone

WEEK_MINUTES = 10080


def moment(text):
    text = text.rstrip("Z")
    if "." in text:
        whole, part = text.split(".")
        text = whole + "." + part[:6]
    return int(datetime.fromisoformat(text).replace(tzinfo=timezone.utc).timestamp() * 1000)


def is_fable(model):
    return "fable" in (model or "").lower()


def main():
    out_dir = os.path.expanduser(sys.argv[1])
    root = os.path.expanduser(sys.argv[2] if len(sys.argv) > 2 else "~/.fastctx/world/usage")
    os.makedirs(out_dir, exist_ok=True)
    for ledger in sorted(glob.glob(os.path.join(root, "claude-*"))):
        if not os.path.isdir(ledger):
            continue
        try:
            base = int(open(os.path.join(ledger, "week")).read().strip())
        except (OSError, ValueError):
            base = 3000 * 10**6
        try:
            kept = json.load(open(os.path.join(ledger, "reckoning.json")))
        except (OSError, ValueError):
            kept = {}
        readings, uses, accounts = [], [], []
        for day in sorted(glob.glob(os.path.join(ledger, "20*.jsonl"))):
            for line in open(day, encoding="utf-8", errors="replace"):
                head = line[:40]
                try:
                    if '"reading"' in head:
                        row = json.loads(line)
                        for window in row.get("windows", []):
                            if window.get("minutes") == WEEK_MINUTES and window.get("resets_at") and window.get("used_tenths") is not None:
                                readings.append([moment(row["at"]), moment(window["resets_at"]), window["used_tenths"], row.get("account")])
                    elif '"account"' in head:
                        row = json.loads(line)
                        accounts.append([moment(row["at"]), row["account"]])
                    elif any(kind in head for kind in ('"meter"', '"spent"', '"cost"', '"summary"')):
                        row = json.loads(line)
                        kind = row["kind"]
                        if kind == "cost":
                            items = [{"model": row.get("model"), "usd": row.get("usd")}]
                        elif kind == "spent":
                            items = row.get("uses", [])
                        elif kind == "summary":
                            items = [row]
                        else:
                            items = [row["usage"]] if row.get("usage") else []
                        for usage in items:
                            usd = usage.get("usd")
                            if not usd:
                                continue
                            # A background call no session saw is not Fable's (`ledger.rs`).
                            call = usage.get("call") or {}
                            model = "" if call.get("source") == "background" and not call.get("observed") else usage.get("model")
                            uses.append([moment(row["at"]), usd * 5 // 2 if is_fable(model) else usd])
                except (ValueError, KeyError, TypeError):
                    continue
        if not readings:
            continue
        readings.sort(key=lambda reading: reading[0])
        uses.sort()
        accounts.sort()
        name = os.path.basename(ledger)
        data = {"login": name, "base": base, "swing": kept.get("swing", 10), "readings": readings, "uses": uses, "accounts": accounts}
        with open(os.path.join(out_dir, name + ".json"), "w", encoding="utf-8") as out:
            json.dump(data, out, separators=(",", ":"))
        print(name, len(readings), "readings", len(uses), "uses", len(accounts), "accounts")


main()
