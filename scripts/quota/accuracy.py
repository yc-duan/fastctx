"""How far a reckoning of a login's week stands from its vendor's own 7-day
readings over every series in FOLDER: before each reading, what was used of
the week over its figure, against what the reading says, in points.

    python accuracy.py FOLDER [SUFFIX]

SUFFIX names the reckoning read beside each series, every reading of it:
`.rust.txt` (the default) from the replay test, or a `model.py` output such
as `.old.txt` from `model.py SERIES.json 1 --old`. Readings of a week after
the vendor gave some of it back (a fall of five points or more) are counted
apart: what was given back is set right by a reset made by hand, not by the
figure.
"""
import bisect, glob, json, pathlib, sys

SAME = 120_000


def blank():
    return {"n": 0, 1: 0, 2: 0, 5: 0, "off": 0.0}


def add(stat, off):
    stat["n"] += 1
    stat["off"] += off
    for points in (1, 2, 5):
        stat[points] += off <= points


def said(name, stat):
    n = stat["n"] or 1
    return f"{name:38} {stat['n']:7} readings  within 1: {stat[1] / n:6.1%}  2: {stat[2] / n:6.1%}  5: {stat[5] / n:6.1%}  mean {stat['off'] / n:5.2f} points"


def main():
    folder = sys.argv[1]
    suffix = sys.argv[2] if len(sys.argv) > 2 else ".rust.txt"
    before, after = blank(), blank()
    for path in sorted(glob.glob(folder + "/*.json")):
        if path.endswith(".checks.json"):
            continue
        series = json.load(open(path, encoding="utf-8"))
        told = pathlib.Path(path).with_suffix(suffix)
        if not told.exists():
            continue
        rows = [(at, 0, i) for i, (at, *_) in enumerate(series["readings"])] + [(at, 1, account) for at, account in series["accounts"]]
        rows.sort(key=lambda row: (row[0], row[1]))
        # The accounts known as each reading came, as the node knows them
        # then; the login is in the latest.
        known = []

        def account_at(at):
            place = bisect.bisect_right([since for since, _ in known], at)
            return known[place - 1][1] if place else (known[0][1] if known else None)

        lines = iter(line for line in told.read_text().split("\n") if line.strip())
        own = blank()
        high, given = {}, set()
        for at, order, index in rows:
            if order == 1:
                known.append((at, index))
                continue
            r_at, window, end, figure, used = (int(x) for x in next(lines).split())
            assert r_at == at, f"{told} is not every reading of {path}"
            _, r_end, tenths, written = series["readings"][index]
            current = known[-1][1] if known else None
            read_as = written if written is not None else account_at(at)
            if not window or abs(r_end - end) > SAME or read_as != current or figure <= 0 or at >= r_end:
                continue
            week = (read_as, round(r_end / 600_000))
            if tenths + 50 <= high.get(week, 0):
                given.add(week)
            high[week] = max(high.get(week, 0), tenths)
            off = abs(used * 1000 / figure - tenths) / 10
            if week in given:
                add(after, off)
            else:
                add(own, off)
                add(before, off)
        print(said(pathlib.Path(path).stem[:38], own))
    print(said("all, before the vendor gave any back", before))
    print(said("all, after it did", after))


main()
