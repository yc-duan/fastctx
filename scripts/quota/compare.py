"""The node's reckoning against the independent model's, over every series
in FOLDER, three ways:

- before each reading: the week, whether it is the vendor's, the figure and
  what was used (SERIES.rust.txt against SERIES.py.txt);
- each week as last measured (SERIES.rust-weeks.txt against
  SERIES.py-weeks.txt);
- the node's own count of how far its reckoning stood from each reading
  (SERIES.checks.json), against the same count made here from the replay's
  reckoning and the readings.

    python compare.py FOLDER

The node's side comes from the replay test (`reckoning.rs`,
`replays_vendor_series`); the model's from `model.py`.
"""
import bisect, collections, glob, json, pathlib, sys

SAME = 120_000


def lines(path):
    seen = collections.Counter()
    out = {}
    for line in path.read_text().split("\n"):
        if not line.strip():
            continue
        at, window, end, figure, used = (int(x) for x in line.split())
        seen[at] += 1
        out[(at, seen[at])] = (window, end, figure, used)
    return out


def counted(series, rust):
    """How the node counts its readings (`checks.rs`), from the replay."""
    rows = [(at, 0, i) for i, (at, *_) in enumerate(series["readings"])] + [(at, 1, account) for at, account in series["accounts"]]
    rows.sort(key=lambda row: (row[0], row[1]))
    # The accounts known as each reading came, as the node knows them then;
    # the login is in the latest.
    known = []

    def account_at(at):
        place = bisect.bisect_right([since for since, _ in known], at)
        return known[place - 1][1] if place else (known[0][1] if known else None)

    told = iter(line for line in rust.read_text().split("\n") if line.strip())
    count = {"compared": 0, "within_one": 0, "within_two": 0}
    for at, order, index in rows:
        if order == 1:
            known.append((at, index))
            continue
        _, window, end, figure, used = (int(x) for x in next(told).split())
        _, r_end, tenths, written = series["readings"][index]
        current = known[-1][1] if known else None
        read_as = written if written is not None else account_at(at)
        if not window or figure == 0 or read_as != current or abs(r_end - end) > SAME:
            continue
        reckoned = min(used * 1000 // figure, 2**32 - 1)
        count["compared"] += 1
        count["within_one"] += abs(reckoned - tenths) <= 10
        count["within_two"] += abs(reckoned - tenths) <= 20
    return count


def main():
    folder = pathlib.Path(sys.argv[1])
    compared = apart = 0
    weeks_same = weeks_apart = checks_same = checks_apart = 0
    for py in sorted(folder.glob("*.py.txt")):
        stem = py.name[: -len(".py.txt")]
        rust = folder / f"{stem}.rust.txt"
        if not rust.exists():
            print(f"{stem}: no replay of it")
            continue
        theirs, ours = lines(py), lines(rust)
        differ = collections.Counter()
        first = []
        for key, want in theirs.items():
            got = ours.get(key)
            if got is None:
                differ["missing"] += 1
                continue
            compared += 1
            if got != want:
                apart += 1
                for name, a, b in zip(("window", "end", "figure", "used"), got, want):
                    differ[name] += a != b
                if len(first) < 3:
                    first.append((key[0], got, want))
        if differ:
            print(f"{stem}: {len(theirs)} readings, apart {dict(differ)}")
            for at, got, want in first:
                print(f"    at {at}: node {got} model {want}")
        model_weeks, node_weeks = folder / f"{stem}.py-weeks.txt", folder / f"{stem}.rust-weeks.txt"
        if model_weeks.exists() and node_weeks.exists():
            if model_weeks.read_text().split() == node_weeks.read_text().split():
                weeks_same += 1
            else:
                weeks_apart += 1
                print(f"{stem}: its weeks as measured differ")
        series = json.loads((folder / f"{stem}.json").read_text(encoding="utf-8"))
        kept = folder / f"{stem}.checks.json"
        node = json.loads(kept.read_text()) if kept.exists() else {"compared": 0, "within_one": 0, "within_two": 0}
        here = counted(series, rust)
        if {key: node[key] for key in here} == here:
            checks_same += 1
        else:
            checks_apart += 1
            print(f"{stem}: the node counted {({key: node[key] for key in here})}, here {here}")
    print(f"readings: {compared} compared, {apart} apart; weeks: {weeks_same} series alike, {weeks_apart} not; counts: {checks_same} alike, {checks_apart} not")
    sys.exit(1 if apart or weeks_apart or checks_apart else 0)


main()
