"""Random series of a login's vendor readings and the quota it used, in the
shape `extract.py` writes, with the troubles production rarely shows:
readings that dip and come back, weeks the vendor gives back in full or in
part, use the ledger never saw, account switches, readings without an
account, long gaps, stale readings past their week's end, the week's end
said a moment apart or moved, readings that are not whole percents, several
rows at one moment.

    python fuzz.py COUNT FOLDER SEED [--harsh]

`--harsh` makes the rare troubles ten times as common.
"""
import json, pathlib, random, sys

DAY = 24 * 3600 * 1000
WEEK = 7 * DAY


def series(rng, harsh):
    rare = 10 if harsh else 1
    base = rng.choice([1_650_000_000, 2_900_000_000, 3_100_000_000, 3_300_000_000, 500_000_000])
    swing = rng.choice([10, 10, 10, 0, 5, 30])
    now = 1_792_000_000_000 + rng.randrange(0, 50 * DAY)
    end = now + rng.randrange(DAY // 2, WEEK)
    weeks = rng.randrange(2, 9)
    readings, uses, accounts = [], [], []
    account = "acct-a"
    if rng.random() < 0.8:
        accounts.append((now - rng.randrange(0, DAY), account))
    unnamed = rng.random() < 0.3      # an old mod: readings without an account
    for _ in range(weeks):
        true = base * (1 + rng.uniform(-0.35, 0.35))
        counted = 0.0                  # what the vendor counts this week
        at = now
        busy = rng.uniform(0.02, 0.3)  # uses a minute
        # Each use's mean, for the week to come to a share of what it holds.
        mean = rng.uniform(0.05, 1.4) * true / (busy * 7 * 24 * 60)
        whole = rng.random() < 0.85
        dip_left, dip = 0, 0
        while at < end:
            gap = int(rng.expovariate(busy) * 60_000) + 1
            if rng.random() < 0.01 * rare:
                gap += rng.randrange(3600_000, 2 * DAY)
            at += gap
            if at >= end:
                break
            usd = int(mean * rng.lognormvariate(-0.72, 1.2))
            if rng.random() < 0.97:
                uses.append((at, usd))
            counted += usd             # what the ledger missed still counts
            if rng.random() < 0.002 * rare:
                counted += true * rng.uniform(0.0, 0.05)
            if rng.random() < 0.0008 * rare:
                counted *= rng.choice([0.0, rng.uniform(0.2, 0.9)])
            if rng.random() < 0.15:
                if dip_left == 0 and rng.random() < 0.01 * rare:
                    dip_left, dip = rng.randrange(1, 14), rng.randrange(10, 300)
                percent = min(counted / true * 100, 100)
                tenths = int(percent) * 10 if whole else int(percent * 10)
                if dip_left:
                    tenths, dip_left = max(tenths - dip, 0), dip_left - 1
                jitter = rng.choice([0, 0, 0, rng.randrange(-120_000, 120_001), rng.randrange(-200_000, 200_001)])
                name = None if unnamed or rng.random() < 0.02 else account
                readings.append((at, end + jitter, tenths, name))
                if rng.random() < 0.01 * rare:
                    readings.append((at, end, tenths, name))
                if rng.random() < 0.003 * rare:
                    readings.append((at, end - WEEK, rng.randrange(0, 1001), name))
            if rng.random() < 0.0005 * rare:
                account = rng.choice(["acct-a", "acct-b", "acct-c"])
                accounts.append((at + rng.randrange(0, 5), account))
            if rng.random() < 0.0002 * rare:
                end += rng.randrange(-DAY, DAY)
        # A reading or two past the week's end, before the next week's come.
        for _ in range(rng.randrange(0, 3)):
            readings.append((end + rng.randrange(0, 3600_000), end, rng.randrange(0, 1001), account))
        now = end
        end = end + WEEK + rng.choice([0, 0, 0, rng.randrange(-3600_000, 3600_001)])
        if rng.random() < 0.15:
            now = end - WEEK + rng.randrange(DAY, 3 * DAY)   # a week begun late
    return {"base": base, "swing": swing, "readings": readings, "uses": uses, "accounts": accounts}


def main():
    args = [arg for arg in sys.argv[1:] if not arg.startswith("--")]
    count, folder, seed = int(args[0]), pathlib.Path(args[1]), int(args[2])
    harsh = "--harsh" in sys.argv
    folder.mkdir(parents=True, exist_ok=True)
    for one in range(count):
        made = series(random.Random(seed * 100_003 + one), harsh)
        (folder / f"fuzz-{seed}-{one:03}.json").write_text(json.dumps(made))
        print(f"fuzz-{seed}-{one:03}: {len(made['readings'])} readings, {len(made['uses'])} uses, {len(made['accounts'])} accounts")


main()
