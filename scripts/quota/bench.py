"""Quota bench: how well a Claude Code login's weekly quota, as the World
reckons it from its own ledger, agrees with the share of the week its vendor
reads out. The vendor's 7-day readings are the reference: at every one of
them, the share the ledger reckons at that moment is compared with it.

Run it on the machine that keeps the ledgers; it only reads them:
    python3 scripts/quota/bench.py [USAGE_DIR]
USAGE_DIR defaults to ~/.fastctx/world/usage; every `claude-*` ledger in it
is replayed. What it prints are counts and figures, never words.

The weekly figure, as each way would reckon it at each reading:
  whole      the last week before, measured from its first reading to its
             last (the World's way before 1.0.1-beta.29)
  stretches  the last week before, measured in stretches split where its
             readings fell and stayed down, as where the vendor gave the
             week's use back
  this week  this week's own stretches so far, once they rose 25 points;
             `stretches` until then
each held within the login's expected week and its swing, as the World holds
it. A reset made by hand scales what was used before it, as the World does.

For each login it prints, first, its own check: each week the World measured
and wrote in `reckoning.json`, measured again here the way that comes nearer
(from end to end, as nodes before 1.0.1-beta.29 did, or in stretches); then,
per way, how far the reckoned share stood from the vendor's, in points, and
how far each way's figure stood from what the week came to measured whole in
stretches, in percent.
"""

import bisect
import glob
import json
import os
import statistics
import sys
from datetime import datetime, timedelta, timezone

WEEK = timedelta(days=7)
WEEK_MINUTES = 10080
# Tenths of a point a week must rise to say what it came to (`reckoning.rs`, ROSE).
ROSE = 250
# A fall of this many tenths below a stretch's highest reading, held by the
# next readings, splits a week (`reckoning.rs`, FELL and FELL_FOR).
FELL = 50
FELL_FOR = 10


def moment(text):
    text = text.rstrip("Z")
    if "." in text:
        whole, part = text.split(".")
        text = whole + "." + part[:6]
    return datetime.fromisoformat(text).replace(tzinfo=timezone.utc)


def is_fable(model):
    return "fable" in (model or "").lower()


def load(ledger):
    """The ledger's 7-day readings, what was used in quota millionths, and
    the accounts it was in, each in time order."""
    readings, uses, accounts = [], [], []
    for day in sorted(glob.glob(os.path.join(ledger, "20*.jsonl"))):
        for line in open(day, encoding="utf-8", errors="replace"):
            head = line[:40]
            try:
                if '"reading"' in head:
                    row = json.loads(line)
                    for window in row.get("windows", []):
                        if window.get("minutes") == WEEK_MINUTES and window.get("resets_at") and window.get("used_tenths") is not None:
                            readings.append((moment(row["at"]), moment(window["resets_at"]), window["used_tenths"]))
                elif '"account"' in head:
                    row = json.loads(line)
                    accounts.append((moment(row["at"]), row["account"]))
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
                        call = usage.get("call") or {}
                        # A background call no one observed is not known to be Fable's.
                        model = "" if call.get("source") == "background" and not call.get("observed") else usage.get("model")
                        uses.append((moment(row["at"]), usd * 5 // 2 if is_fable(model) else usd))
            except (ValueError, KeyError, TypeError):
                continue
    readings.sort(key=lambda reading: reading[0])
    uses.sort()
    accounts.sort()
    return readings, uses, accounts


class Spent:
    """What was used between two moments, from running sums."""

    def __init__(self, uses):
        self.at = [at for at, _ in uses]
        self.sums = [0]
        for _, used in uses:
            self.sums.append(self.sums[-1] + used)

    def between(self, first, last):
        return self.sums[bisect.bisect_left(self.at, last)] - self.sums[bisect.bisect_left(self.at, first)]


def stretches(points):
    """A week's readings `(at, tenths)` split where they fell and stayed down."""
    out, current, high = [], [], -1
    for index, (at, used) in enumerate(points):
        if current and all(later <= high - FELL for _, later in points[index:index + FELL_FOR]):
            out.append(current)
            current, high = [], -1
        current.append((at, used))
        high = max(high, used)
    if current:
        out.append(current)
    return out


def whole(points, spent):
    (first, low), (last, high) = points[0], points[-1]
    rise = high - low
    used = spent.between(first, last)
    return (used * 1000 / rise if rise >= ROSE else None), used, rise


def in_stretches(points, spent):
    used = rise = 0
    for stretch in stretches(points):
        (first, low), (last, _) = stretch[0], stretch[-1]
        high = max(tenths for _, tenths in stretch)
        if high <= low:
            continue
        used += spent.between(first, last)
        rise += high - low
    return (used * 1000 / rise if rise >= ROSE else None), used, rise


def held(base, swing, came_to):
    if came_to is None:
        return base
    return min(max(came_to, base * (100 - swing) / 100), base * (100 + swing) / 100)


def replay(ledger):
    try:
        base = int(open(os.path.join(ledger, "week")).read().strip())
    except (OSError, ValueError):
        base = 3000 * 10**6
    try:
        kept = json.load(open(os.path.join(ledger, "reckoning.json")))
    except (OSError, ValueError):
        kept = {}
    swing = kept.get("swing", 10)
    resets = [
        (moment(made["at"]), moment(made["week_end"]), made["to"] / made["from"])
        for made in kept.get("resets", [])
        if made.get("from") and not made.get("undone")
    ]
    readings, uses, _ = load(ledger)
    if not readings:
        return None
    spent = Spent(uses)
    weeks = {}
    for at, end, used in readings:
        weeks.setdefault(end, []).append((at, used))
    ends = sorted(weeks)
    checks = []
    for measured in kept.get("weeks", []):
        points = weeks.get(moment(measured["end"]))
        if not points:
            continue
        ways = [(name, *way(points, spent)[1:]) for name, way in (("end to end", whole), ("in stretches", in_stretches))]
        name, used, rise = min(ways, key=lambda one: abs(one[1] - measured["spent"]) + abs(one[2] - measured["rise_tenths"]))
        checks.append((measured["end"][:10], measured["spent"], used, measured["rise_tenths"], rise, name))

    def week_before(end, way):
        for earlier in reversed([one for one in ends if one <= end - WEEK + timedelta(seconds=120)]):
            came_to = way(weeks[earlier], spent)[0]
            if came_to is not None:
                return came_to
        return None

    before_whole = {end: week_before(end, whole) for end in ends}
    before_stretches = {end: week_before(end, in_stretches) for end in ends}
    came_to = {end: in_stretches(weeks[end], spent)[0] for end in ends}
    last = readings[-1][0]
    rows = []
    for at, end, used in readings:
        start = end - WEEK
        if at < start:
            continue
        so_far = [point for point in weeks[end] if point[0] < at]
        week_used = spent.between(start, at)
        for made, week_end, part in resets:
            if abs((week_end - end).total_seconds()) < 120 and made <= at:
                week_used -= (1 - part) * spent.between(start, made)
        figures = {
            "whole": held(base, swing, before_whole[end]),
            "stretches": held(base, swing, before_stretches[end]),
        }
        this_week = in_stretches(so_far, spent)[0] if so_far else None
        figures["this week"] = held(base, swing, this_week) if this_week is not None else figures["stretches"]
        rows.append({
            "official": used / 10,
            "shares": {way: week_used / figure * 100 for way, figure in figures.items()},
            "figures": figures,
            "came_to": came_to[end] if end <= last else None,
        })
    return {"base": base, "swing": swing, "checks": checks, "rows": rows}


def spread(errors):
    if not errors:
        return "none compared"
    errors = sorted(abs(error) for error in errors)
    at = lambda part: errors[min(len(errors) - 1, int(part * len(errors)))]
    within = sum(1 for error in errors if error <= 2) / len(errors)
    return f"n={len(errors)} median={statistics.median(errors):.1f} p90={at(0.9):.1f} max={errors[-1]:.1f} within 2: {within:.0%}"


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/.fastctx/world/usage")
    for ledger in sorted(glob.glob(os.path.join(root, "claude-*"))):
        if not os.path.isdir(ledger):
            continue
        out = replay(ledger)
        if not out or not out["rows"]:
            continue
        print(f"== {os.path.basename(ledger)} expected ${out['base'] / 1e6:,.0f} swing {out['swing']}%")
        for week, node, ours, node_rise, our_rise, way in out["checks"]:
            print(f"   check, week to {week}: World ${node / 1e6:,.0f} over {node_rise / 10:.0f} points, "
                  f"bench {way} ${ours / 1e6:,.0f} over {our_rise / 10:.0f} points, {abs(ours - node) / max(node, 1):.1%} apart")
        # Readings below ten percent say too little of a share to compare.
        rows = [row for row in out["rows"] if row["official"] >= 10]
        for way in ("whole", "stretches", "this week"):
            print(f"   {way:9} share against the vendor's, points: {spread([row['shares'][way] - row['official'] for row in rows])}")
        for way in ("whole", "stretches", "this week"):
            errors = [(row["figures"][way] - row["came_to"]) / row["came_to"] * 100 for row in rows if row["came_to"]]
            print(f"   {way:9} figure against the week's own, percent: {spread(errors)}")


if __name__ == "__main__":
    main()
