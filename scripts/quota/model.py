"""An independent reckoning of a Claude Code login's weekly quota, written
from the rules (`crates/fastctx-node/src/usage/reckoning.rs`, `week_read.rs`)
rather than from the node's code, to compare with what the node reckons over
the same series (`extract.py`, `fuzz.py`): before each 7-day reading, the
week it is in, the figure and how much of the week was used.

    python model.py SERIES.json [EVERY] [WEEKS_OUT] [--old] > SERIES.py.txt

Each printed line is `at window week_end figure used`, in milliseconds and
millionths of a dollar. EVERY prints every so many readings (all are taken
in); WEEKS_OUT gets each week as last measured: `end account spent rise
came_to`. `--old` reckons as nodes before 1.0.1-beta.29 did: each week from
its first reading to its last, and this week's figure what the week before
came to. With MODEL_AT=<ms>, what went into the figure at that reading goes
to stderr.
"""
import bisect, json, math, os, sys

WEEK = 7 * 24 * 3600 * 1000
DAY = 24 * 3600 * 1000
HOUR = 3600 * 1000
# Two resets said this close apart are one (`readings.rs`, same).
SAME = 120_000
# `reckoning.rs`: ROSE, BACK, BEFORE_OFF, READ_OFF, WIDEST;
# `week_read.rs`: FELL, FELL_FOR.
ROSE, FELL, FELL_FOR, BACK = 250, 50, 10, 5
BEFORE_OFF, READ_OFF, WIDEST = 0.08, 10.0, 20
OLD = "--old" in sys.argv


def same(a, b):
    return abs(a - b) <= SAME


def account_at(accounts, at):
    found = None
    for since, account in accounts:
        if since <= at:
            found = account
    if found is None and accounts:
        found = accounts[0][1]
    return found


def stretches(points):
    """A week's readings split where they fell FELL below the highest of the
    stretch before and the next FELL_FOR stayed down."""
    out, start, high = [], 0, 0
    for at in range(len(points)):
        if at > start and all(t + FELL <= high for _, t in points[at:at + FELL_FOR]):
            out.append(points[start:at])
            start, high = at, 0
        high = max(high, points[at][1])
    if start < len(points):
        out.append(points[start:])
    return out


def rose(points, spent_between):
    """What a week's readings say: the dollars used while each stretch rose,
    and how far they rose together, in tenths of a point."""
    if OLD:
        first, last = points[0], points[-1]
        rise = last[1] - first[1]
        return (spent_between(first[0], last[0]), rise) if rise > 0 else (0, 0)
    spent = rise = 0
    for one in stretches(points):
        low = one[0][1]
        high = max(t for _, t in one)
        if high <= low:
            continue
        spent += spent_between(one[0][0], one[-1][0])
        rise += high - low
    return spent, rise


def within(base, swing, came):
    low, high = base * (100 - swing) // 100, base * (100 + swing) // 100
    return base if came is None else min(max(came, low), high)


def half_away(value):
    """Rust's f64::round: halves away from zero."""
    return math.floor(value + 0.5) if value >= 0 else -math.floor(-value + 0.5)


def weighed(base, swing, before, own):
    prior = within(base, swing, before)
    if OLD:
        return prior
    prior_off = BEFORE_OFF if before is not None else max(BEFORE_OFF, swing / 100)
    if not own or own[1] <= 0:
        return prior
    spent, rise = own
    came = spent * 1000.0 / rise
    own_off = READ_OFF / rise
    weight = prior_off * prior_off / (prior_off * prior_off + own_off * own_off)
    return within(base, swing, int(half_away(prior + weight * (came - prior))))


def main():
    args = [arg for arg in sys.argv[1:] if not arg.startswith("--")]
    series = json.load(open(args[0], encoding="utf-8"))
    every = int(args[1]) if len(args) > 1 else 1
    weeks_out = args[2] if len(args) > 2 else None
    base, swing = series["base"], min(series["swing"], WIDEST)
    # At one moment, readings first, then accounts, then uses, each in the
    # order the series gives them.
    rows = [(at, 0, ("reading", end, tenths, account)) for at, end, tenths, account in series["readings"]]
    rows += [(at, 1, ("account", account)) for at, account in series["accounts"]]
    rows += [(at, 2, ("use", usd)) for at, usd in series["uses"]]
    rows.sort(key=lambda row: (row[0], row[1]))
    accounts = []        # (since, account), by since
    latest = {}          # account -> (at, end) of its newest reading
    week_read = {}       # account -> [end, [(at, tenths)]]
    readings = []        # (at, end, tenths, account as written)
    use_at = []
    sums = {None: [0]}   # account -> running sums of its uses, in the uses' order
    total = [0]
    seen = 0
    kept = {"through": None, "through_account": None, "tried": None, "method": None, "weeks": []}

    def current():
        return accounts[-1][1] if accounts else None

    def week_of(now):
        reading = latest.get(current())
        if reading is None:
            return now - WEEK, now, False
        end = reading[1]
        if now >= end:
            end += WEEK * ((now - end) // WEEK + 1)
        return end - WEEK, end, True

    def spent(first, last, account):
        lo, hi = bisect.bisect_left(use_at, first), bisect.bisect_left(use_at, last)
        running = sums.get(account)
        return 0 if running is None else running[hi] - running[lo]

    def measure(start, account):
        since = (start - BACK * WEEK) // DAY * DAY
        read = []
        for at, end, tenths, written in readings:
            read_as = written if written is not None else account_at(accounts, at)
            if read_as == account and end <= start + SAME and at < end and end - WEEK >= since:
                read.append((end, at, tenths))
        read.sort(key=lambda one: (one[0], one[1]))
        groups, out = [], []
        for one in read:
            if groups and same(groups[-1][-1][0], one[0]):
                groups[-1].append(one)
            else:
                groups.append([one])
        for group in groups:
            # In the order read, however a moment apart they said its reset.
            points = sorted(((at, tenths) for _, at, tenths in group), key=lambda point: point[0])
            used, rise = rose(points, lambda a, b: spent(a, b, account))
            came = used * 1000 // rise if rise >= ROSE else None
            out.append((group[0][0], account, came, used, rise))
        return out

    def measure_due(now, start, window, account):
        if not window:
            return
        if (kept["method"] or 0) >= 1 and kept["through"] is not None and same(kept["through"], start) and kept["through_account"] == account:
            return
        if kept["tried"] is not None and kept["tried"] >= start and now < kept["tried"] + HOUR:
            return
        measured = measure(start, account)
        before = any(same(end, start) and came is not None for end, _, came, _, _ in measured)
        for end, who, came, used, rise in measured:
            kept["weeks"] = [w for w in kept["weeks"] if not (w[1] == who and same(w[0], end))]
            kept["weeks"].append((end, who, came, used, rise))
        kept["weeks"].sort(key=lambda w: w[0])
        if before or now >= start + DAY:
            kept.update(through=start, through_account=account, tried=None, method=1)
        else:
            kept["tried"] = now

    for at, _, row in rows:
        kind = row[0]
        if kind == "reading":
            seen += 1
            start, end, window = week_of(at)
            account = current()
            measure_due(at, start, window, account)
            before = next((came for w_end, who, came, _, _ in reversed(kept["weeks"]) if who == account and w_end <= start + SAME and came is not None), None)
            if (seen - 1) % every == 0:
                own = None
                if window:
                    read = week_read.get(account)
                    if read is not None and same(read[0], end):
                        used, rise = rose(read[1], lambda a, b: spent(a, b, account))
                        own = (used, rise) if rise > 0 else None
                figure = weighed(base, swing, before, own)
                if str(at) == os.environ.get("MODEL_AT"):
                    print("before", before, "own", own, "weeks", kept["weeks"], "start", start, "account", account, file=sys.stderr)
                lo, hi = bisect.bisect_left(use_at, start), bisect.bisect_left(use_at, at)
                week_used = spent(start, at, account) if window else total[hi] - total[lo]
                print(at, int(window), end, figure, week_used)
            # Then the reading itself is taken in: the newest is the
            # account's, its week keeping the end first read of it; one of a
            # week before leaves that as it was.
            r_end, tenths, written = row[1], row[2], row[3]
            resolved = written if written is not None else account_at(accounts, at)
            if resolved not in latest or latest[resolved][0] <= at:
                old = latest.get(resolved)
                if old is None or (r_end > old[1] and not same(old[1], r_end)):
                    latest[resolved] = (at, r_end)
                else:
                    latest[resolved] = (at, old[1])
            if at < r_end:
                read = week_read.get(resolved)
                if read is not None and same(read[0], r_end):
                    place = bisect.bisect_right([t for t, _ in read[1]], at)
                    read[1].insert(place, (at, tenths))
                elif read is not None and r_end < read[0]:
                    pass
                else:
                    week_read[resolved] = [r_end, [(at, tenths)]]
            readings.append((at, r_end, tenths, written))
        elif kind == "account":
            if not accounts:
                if None in latest:
                    latest[row[1]] = latest.pop(None)
                if None in week_read:
                    week_read[row[1]] = week_read.pop(None)
                # Uses before any account are the first account's.
                sums[row[1]] = sums.pop(None)
            elif row[1] not in sums:
                sums[row[1]] = [0] * len(total)
            accounts.append((at, row[1]))
            accounts.sort(key=lambda one: one[0])
        else:
            owner = account_at(accounts, at) if accounts else None
            use_at.append(at)
            total.append(total[-1] + row[1])
            for account, running in sums.items():
                running.append(running[-1] + (row[1] if account == owner else 0))
    if weeks_out:
        with open(weeks_out, "w", encoding="utf-8") as out:
            for end, who, came, used, rise in kept["weeks"]:
                out.write(f"{end} {who or '-'} {used} {rise} {came if came is not None else -1}\n")


main()
