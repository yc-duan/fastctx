// What a plain browser shows of the logins' usage: a year of hours for each
// login the specimen lists, made up but shaped like real use: working days
// heavier than weekends, mornings and evenings busier than the small hours,
// a run of days now and then with nothing. The desktop's Codex work login
// assists ada and bo; its Claude Code assists ada up to $20 a week; bo's gpu
// is used by bo and, now and then, by the desktop's user; render's Claude
// assists yc, who sees only their own and nothing of its account's windows.
// The week follows each login's seven-day window, and its readings climb to
// what its limits say now; the reset weeks go back to when the login was
// first read, a dozen weeks at most, each with its share of the window.

import type { HubAccount } from "../world/accounts";
import type { UsageHour, UsagePerson, UsageReading, UsageReport, UsageSpan, WeekUse } from "../world/usage";

const HOUR = 3_600_000;
const DAY = 86_400_000;

interface Someone {
  account: HubAccount | null;
  /** Dollars (Claude Code) or millions of tokens (Codex) on a full day. */
  daily: number;
  /** How likely a working day and a weekend day are used. */
  weekday: number;
  weekend: number;
  /** Days ago its use began. */
  since: number;
  /** Leans its hours late into the night. */
  owl?: boolean;
}

interface Specimen {
  people: Someone[];
  claude: boolean;
  /** The seven-day window's share now, and the five-hour one's. */
  week: number;
  five: number;
  /** When the seven-day window resets, from now. */
  resetsIn: number;
  /** A plan with a weekly window and no five-hour one. */
  weekOnly?: boolean;
  /** Read by an account the login only assists. */
  assisted?: boolean;
}

const home = (account: string): HubAccount => ({ hub: "w-5be1fcee3751dc7a", account });
const acme = (account: string): HubAccount => ({ hub: "w-9a07c2d4e1b36f58", account });

const SPECIMENS: Record<string, Specimen> = {
  "desktop|codex|C:\\Users\\me\\.codex": { people: [{ account: null, daily: 9, weekday: 0.9, weekend: 0.45, since: 400 }, { account: home("me"), daily: 4, weekday: 0.5, weekend: 0.2, since: 200 }], claude: false, week: 0.233, five: 0.412, resetsIn: 3 * DAY, weekOnly: true },
  "desktop|codex|C:\\Users\\me\\.codex-work": {
    people: [
      { account: home("me"), daily: 6, weekday: 0.85, weekend: 0.2, since: 300 },
      { account: home("ada"), daily: 5, weekday: 0.8, weekend: 0.3, since: 60 },
      { account: home("bo"), daily: 3, weekday: 0.5, weekend: 0.5, since: 25, owl: true },
    ],
    claude: false,
    week: 0.61,
    five: 0.88,
    resetsIn: 3 * DAY,
  },
  "desktop|claude|C:\\Users\\me\\.claude": {
    people: [
      { account: null, daily: 34, weekday: 0.9, weekend: 0.5, since: 400 },
      { account: home("me"), daily: 18, weekday: 0.6, weekend: 0.25, since: 150 },
      { account: home("ada"), daily: 6, weekday: 0.7, weekend: 0.2, since: 45 },
      { account: home("bo"), daily: 4, weekday: 0.6, weekend: 0.3, since: 30, owl: true },
      { account: home("chen"), daily: 3, weekday: 0.7, weekend: 0.1, since: 20 },
      { account: home("dai"), daily: 2, weekday: 0.5, weekend: 0.2, since: 18 },
      { account: home("eve"), daily: 1.5, weekday: 0.6, weekend: 0.1, since: 12 },
      { account: home("fang"), daily: 1, weekday: 0.4, weekend: 0.2, since: 10 },
    ],
    claude: true,
    week: 0.64,
    five: 0.41,
    resetsIn: 2 * DAY + 7 * HOUR,
  },
  "gpu|claude|/home/bo/.claude": { people: [{ account: null, daily: 22, weekday: 0.8, weekend: 0.6, since: 220, owl: true }, { account: home("me"), daily: 9, weekday: 0.3, weekend: 0.1, since: 90 }], claude: true, week: 0.38, five: 0.12, resetsIn: 5 * DAY },
  "lab|codex|/home/me/.codex": { people: [{ account: null, daily: 2, weekday: 0.4, weekend: 0.1, since: 120 }], claude: false, week: 0.05, five: 0, resetsIn: 6 * DAY },
  "build|codex|/home/ops/.codex": { people: [{ account: null, daily: 7, weekday: 0.8, weekend: 0.1, since: 300 }, { account: acme("yc"), daily: 4, weekday: 0.6, weekend: 0.1, since: 80 }], claude: false, week: 0.44, five: 0.2, resetsIn: 4 * DAY },
  "render|claude|/home/ops/.claude": { people: [{ account: acme("yc"), daily: 11, weekday: 0.75, weekend: 0.2, since: 70 }], claude: true, week: 0.52, five: 0.33, resetsIn: DAY + 18 * HOUR, assisted: true },
};

/** How a day's use falls over its hours: busy mornings and afternoons, a
 *  lighter evening, little in the small hours unless a night owl's. */
function hourShape(owl: boolean): number[] {
  const day = [0.2, 0.1, 0.05, 0, 0, 0, 0, 0.1, 0.4, 0.9, 1, 1, 0.6, 0.7, 1, 1, 0.9, 0.7, 0.5, 0.6, 0.8, 0.8, 0.6, 0.4];
  const night = [0.9, 1, 0.8, 0.5, 0.2, 0, 0, 0, 0, 0.1, 0.3, 0.4, 0.4, 0.5, 0.6, 0.6, 0.5, 0.5, 0.4, 0.5, 0.7, 0.9, 1, 1];
  const shape = owl ? night : day;
  const sum = shape.reduce((a, b) => a + b, 0);
  return shape.map((value) => value / sum);
}

function seeded(seed: string): () => number {
  let state = 2166136261;
  for (const char of seed) state = Math.imul(state ^ char.charCodeAt(0), 16777619);
  return () => {
    state = Math.imul(state ^ (state >>> 15), 2246822507);
    state = Math.imul(state ^ (state >>> 13), 3266489909);
    state ^= state >>> 16;
    return (state >>> 0) / 4294967296;
  };
}

export function specimenUsage(machine: string, agent: string, dir: string, at: Date): Promise<UsageReport> {
  const key = `${machine}|${agent}|${dir}`;
  const specimen = SPECIMENS[key] ?? { people: [{ account: null, daily: 3, weekday: 0.5, weekend: 0.2, since: 60 }], claude: agent === "claude", week: 0.1, five: 0.05, resetsIn: 4 * DAY };
  const now = at.getTime();
  const hourNow = Math.floor(now / HOUR) * HOUR;
  const random = seeded(key);
  const end = now + specimen.resetsIn;
  const start = end - 7 * DAY;
  const hours: UsageHour[] = [];
  const weekly = specimen.people.map(() => ({ tokens: 0, usd: 0 }));
  // Tokens a dollar buys, mostly cache reads; Codex counts tokens alone.
  const perDollar = 640_000;
  let quiet = 0;
  for (let back = 370; back >= 0; back -= 1) {
    const day = new Date(now - back * DAY);
    day.setHours(0, 0, 0, 0);
    // Now and then a run of days away.
    if (quiet > 0) {
      quiet -= 1;
      continue;
    }
    if (random() < 0.012) quiet = 2 + Math.floor(random() * 6);
    const weekend = day.getDay() === 0 || day.getDay() === 6;
    specimen.people.forEach((person, index) => {
      if (back > person.since || random() > (weekend ? person.weekend : person.weekday)) return;
      const amount = person.daily * (0.25 + random() * random() * 2.2);
      hourShape(!!person.owl).forEach((share, hour) => {
        const at = day.getTime() + hour * HOUR;
        if (at > hourNow || share <= 0 || random() < 0.35) return;
        const value = amount * share * (0.6 + random() * 0.8) * 1.5;
        const usd = specimen.claude ? Math.round(value * 1_000_000) : undefined;
        const tokens = Math.round(specimen.claude ? value * perDollar : value * 1_000_000);
        hours.push({ at: new Date(at).toISOString(), person: index, tokens, ...(usd !== undefined ? { usd } : {}) });
        if (at >= start) {
          weekly[index]!.tokens += tokens;
          weekly[index]!.usd += usd ?? 0;
        }
      });
    });
  }
  hours.sort((a, b) => a.at.localeCompare(b.at));
  const lastDay = specimen.people.map(() => ({ tokens: 0, usd: 0 }));
  for (const hour of hours) {
    if (Date.parse(hour.at) < now - DAY) continue;
    lastDay[hour.person]!.tokens += hour.tokens;
    lastDay[hour.person]!.usd += hour.usd ?? 0;
  }
  // Each person leans to a model and a cache of their own, so their weeks
  // differ from everyone's.
  const people: UsagePerson[] = specimen.people.map((person, index) => ({
    account: person.account,
    week: { tokens: weekly[index]!.tokens, ...(specimen.claude ? { usd: weekly[index]!.usd } : {}) },
    day: { tokens: lastDay[index]!.tokens, ...(specimen.claude ? { usd: lastDay[index]!.usd } : {}) },
    // A Claude login's people put about a third of their week into Fable.
    ...(specimen.claude ? { fable: { tokens: Math.round(weekly[index]!.tokens * 0.34), usd: Math.round(weekly[index]!.usd * 0.34) } } : {}),
    went: went(specimen.claude, weekly[index]!.tokens, weekly[index]!.usd, index),
  }));
  const tokens = weekly.reduce((sum, one) => sum + one.tokens, 0);
  const spent = weekly.reduce((sum, one) => sum + one.usd, 0);
  return Promise.resolve({
    people,
    hours,
    week: { start: new Date(start).toISOString(), end: new Date(end).toISOString(), window: true, ...went(specimen.claude, tokens, spent, 0) },
    readings: specimen.assisted ? [] : readings(hours, start, now, specimen),
    weeks: weeks(hours, start, specimen),
  });
}

/** What a week went to: a person's models and kinds, the first person's
 *  mix being everyone's. */
function went(claude: boolean, tokens: number, spent: number, lean: number): WeekUse {
  const split = claude
    ? { input: 0.01, cache_read: 0.905 - lean * 0.06, cache_write: 0.06 + lean * 0.04, output: 0.025 + lean * 0.02, reasoning: 0 }
    : { input: 0.24 + lean * 0.06, cache_read: 0.69 - lean * 0.08, cache_write: 0, output: 0.05 + lean * 0.015, reasoning: 0.02 + lean * 0.005 };
  const weights = { input: split.input, cache_read: split.cache_read * 0.1, cache_write: split.cache_write * 1.25, output: split.output * 5 };
  const weightSum = Object.values(weights).reduce((a, b) => a + b, 0);
  const models = claude
    ? [
        ["claude-opus-5-5", Math.max(0.1, 0.78 - lean * 0.3)],
        ["claude-sonnet-5-5", 0.16 + lean * 0.22],
        ["claude-haiku-5-5", 0.06 + lean * 0.08],
      ]
    : [
        ["gpt-6.1-sol", Math.max(0.2, 0.81 - lean * 0.25)],
        ["gpt-6-luna", 0.19 + lean * 0.25],
      ];
  const shares = models.reduce((sum, [, share]) => sum + (share as number), 0);
  return {
    models: models
      .map(([model, share]) => ({ model: model as string, tokens: Math.round((tokens * (share as number)) / shares), ...(claude ? { usd: Math.round((spent * (share as number)) / shares) } : {}) }))
      .sort((a, b) => (b.usd ?? 0) - (a.usd ?? 0) || b.tokens - a.tokens),
    tokens: Object.fromEntries(Object.entries(split).map(([kind, share]) => [kind, Math.round(tokens * share)])),
    ...(claude
      ? {
          kinds_usd: {
            input: Math.round((spent * weights.input) / weightSum),
            cache_read: Math.round((spent * weights.cache_read) / weightSum),
            cache_write: Math.round((spent * weights.cache_write) / weightSum),
            output: Math.round((spent * weights.output) / weightSum),
          },
        }
      : {}),
  };
}

/** The login's reset weeks since it was first read, a dozen at most: what
 *  each person used in each, and, but to an account it only assists, the
 *  share of the window each week reached, the past ones in step with what
 *  they used, shared out by what each used. */
function weeks(hours: UsageHour[], start: number, specimen: Specimen): UsageSpan[] {
  const first = Math.min(12, Math.max(1, Math.floor(Math.max(...specimen.people.map((person) => person.since)) / 7)));
  const spans = Array.from({ length: first }, (_, index) => start - (first - 1 - index) * 7 * DAY);
  const sums = spans.map(() => specimen.people.map(() => ({ tokens: 0, usd: 0 })));
  for (const hour of hours) {
    const at = Date.parse(hour.at);
    const index = spans.findIndex((from) => at >= from && at < from + 7 * DAY);
    if (index < 0) continue;
    sums[index]![hour.person]!.tokens += hour.tokens;
    sums[index]![hour.person]!.usd += hour.usd ?? 0;
  }
  const value = (sum: { tokens: number; usd: number }) => (specimen.claude ? sum.usd : sum.tokens);
  const totals = sums.map((week) => week.reduce((all, sum) => all + value(sum), 0));
  const now = totals[totals.length - 1] || 1;
  return spans.map((from, index) => {
    const reached = index === spans.length - 1 ? specimen.week : Math.min(1, (specimen.week * totals[index]!) / now);
    return {
      start: new Date(from).toISOString(),
      end: new Date(from + 7 * DAY).toISOString(),
      ...(specimen.assisted ? {} : { used_tenths: Math.round(reached * 1000) }),
      used: sums[index]!
        .map((sum, person) => ({
          person,
          tokens: sum.tokens,
          ...(specimen.claude ? { usd: sum.usd } : {}),
          ...(specimen.assisted ? {} : { quota_tenths: Math.round((reached * 1000 * value(sum)) / (totals[index] || 1)) }),
        }))
        .filter((share) => share.tokens > 0),
    };
  });
}

/** The windows as read through the week: the seven-day share climbing with
 *  the use to what it says now, the five-hour one rising within each block
 *  of five hours and falling back after. */
function readings(hours: UsageHour[], start: number, now: number, specimen: Specimen): UsageReading[] {
  const inWeek = hours.filter((hour) => Date.parse(hour.at) >= start);
  const value = (hour: UsageHour) => hour.usd ?? hour.tokens;
  const total = inWeek.reduce((sum, hour) => sum + value(hour), 0) || 1;
  const byHour = new Map<number, number>();
  for (const hour of inWeek) byHour.set(Date.parse(hour.at), (byHour.get(Date.parse(hour.at)) ?? 0) + value(hour));
  const ordered = [...byHour.entries()].sort((a, b) => a[0] - b[0]);
  // Each hour's place in its block of five, and the fullest block.
  const blocks: { from: number; used: number }[] = [];
  for (const [at, one] of ordered) {
    const last = blocks[blocks.length - 1];
    if (!last || at >= last.from + 5 * HOUR) blocks.push({ from: at, used: one });
    else last.used += one;
  }
  const fullest = Math.max(1e-9, ...blocks.map((one) => one.used));
  const out: UsageReading[] = [];
  let used = 0;
  let block: { from: number; used: number } | null = null;
  for (const [at, one] of ordered) {
    used += one;
    if (!block || at >= block.from + 5 * HOUR) block = { from: at, used: 0 };
    block.used += one;
    const week = Math.round((used / total) * specimen.week * 1000);
    const five = Math.min(1000, Math.round((block.used / fullest) * 920));
    out.push({
      at: new Date(Math.min(now, at + 50 * 60_000)).toISOString(),
      windows: [
        ...(specimen.weekOnly ? [] : [{ name: "five_hour", minutes: 300, used_tenths: five, resets_at: new Date(block.from + 5 * HOUR).toISOString() }]),
        { name: "seven_day", minutes: 10080, used_tenths: week, resets_at: new Date(start + 7 * DAY).toISOString() },
      ],
    });
  }
  return out;
}

/** The limits a specimen login says now, as its last reading has them. */
export function specimenLimits(machine: string, agent: string, dir: string): { five: number; week: number; resetsIn: number; weekOnly: boolean } | null {
  const specimen = SPECIMENS[`${machine}|${agent}|${dir}`];
  return specimen ? { five: Math.round(specimen.five * 1000), week: Math.round(specimen.week * 1000), resetsIn: specimen.resetsIn, weekOnly: !!specimen.weekOnly } : null;
}
