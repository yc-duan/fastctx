// What a plain browser shows of the logins' usage: a year of hours for each
// login the specimen lists, made up but shaped like real use: working days
// heavier than weekends, mornings and evenings busier than the small hours,
// a run of days now and then with nothing. The desktop's Codex work login
// assists ada and bo; its Claude Code assists ada up to $20 a week; bo's gpu
// is used by bo and, now and then, by the desktop's user; render's Claude
// assists yc, who sees only their own. The week follows each login's
// seven-day window, and its readings climb to what its limits say now.

import type { HubAccount } from "../world/accounts";
import type { UsageHour, UsagePerson, UsageReading, UsageReport } from "../world/usage";

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
}

const home = (account: string): HubAccount => ({ hub: "w-5be1fcee3751dc7a", account });
const acme = (account: string): HubAccount => ({ hub: "w-9a07c2d4e1b36f58", account });

const SPECIMENS: Record<string, Specimen> = {
  "desktop|codex|C:\\Users\\me\\.codex": { people: [{ account: null, daily: 9, weekday: 0.9, weekend: 0.45, since: 400 }, { account: home("me"), daily: 4, weekday: 0.5, weekend: 0.2, since: 200 }], claude: false, week: 0.233, five: 0.412, resetsIn: 3 * DAY },
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
    ],
    claude: true,
    week: 0.64,
    five: 0.41,
    resetsIn: 2 * DAY + 7 * HOUR,
  },
  "gpu|claude|/home/bo/.claude": { people: [{ account: null, daily: 22, weekday: 0.8, weekend: 0.6, since: 220, owl: true }, { account: home("me"), daily: 9, weekday: 0.3, weekend: 0.1, since: 90 }], claude: true, week: 0.38, five: 0.12, resetsIn: 5 * DAY },
  "lab|codex|/home/me/.codex": { people: [{ account: null, daily: 2, weekday: 0.4, weekend: 0.1, since: 120 }], claude: false, week: 0.05, five: 0, resetsIn: 6 * DAY },
  "build|codex|/home/ops/.codex": { people: [{ account: null, daily: 7, weekday: 0.8, weekend: 0.1, since: 300 }, { account: acme("yc"), daily: 4, weekday: 0.6, weekend: 0.1, since: 80 }], claude: false, week: 0.44, five: 0.2, resetsIn: 4 * DAY },
  "render|claude|/home/ops/.claude": { people: [{ account: acme("yc"), daily: 11, weekday: 0.75, weekend: 0.2, since: 70 }], claude: true, week: 0.52, five: 0.33, resetsIn: DAY + 18 * HOUR },
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
  const people: UsagePerson[] = specimen.people.map((person, index) => ({ account: person.account, week: { tokens: weekly[index]!.tokens, ...(specimen.claude ? { usd: weekly[index]!.usd } : {}) } }));
  const tokens = weekly.reduce((sum, one) => sum + one.tokens, 0);
  const spent = weekly.reduce((sum, one) => sum + one.usd, 0);
  const split = specimen.claude ? { input: 0.01, cache_read: 0.905, cache_write: 0.06, output: 0.025, reasoning: 0 } : { input: 0.24, cache_read: 0.69, cache_write: 0, output: 0.05, reasoning: 0.02 };
  const weekTokens = Object.fromEntries(Object.entries(split).map(([kind, share]) => [kind, Math.round(tokens * share)]));
  const weights = { input: split.input, cache_read: split.cache_read * 0.1, cache_write: split.cache_write * 1.25, output: split.output * 5 };
  const weightSum = Object.values(weights).reduce((a, b) => a + b, 0);
  const models = specimen.claude
    ? [
        ["claude-opus-5-5", 0.78],
        ["claude-sonnet-5-5", 0.16],
        ["claude-haiku-5-5", 0.06],
      ]
    : [
        ["gpt-6.1-sol", 0.81],
        ["gpt-6-luna", 0.19],
      ];
  return Promise.resolve({
    people,
    hours,
    week: {
      start: new Date(start).toISOString(),
      end: new Date(end).toISOString(),
      window: true,
      models: models.map(([model, share]) => ({ model: model as string, tokens: Math.round(tokens * (share as number)), ...(specimen.claude ? { usd: Math.round(spent * (share as number)) } : {}) })),
      tokens: weekTokens,
      ...(specimen.claude
        ? {
            kinds_usd: {
              input: Math.round((spent * weights.input) / weightSum),
              cache_read: Math.round((spent * weights.cache_read) / weightSum),
              cache_write: Math.round((spent * weights.cache_write) / weightSum),
              output: Math.round((spent * weights.output) / weightSum),
            },
          }
        : {}),
    },
    readings: readings(hours, start, now, specimen),
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
        { name: "five_hour", minutes: 300, used_tenths: five, resets_at: new Date(block.from + 5 * HOUR).toISOString() },
        { name: "seven_day", minutes: 10080, used_tenths: week, resets_at: new Date(start + 7 * DAY).toISOString() },
      ],
    });
  }
  return out;
}

/** The limits a specimen login says now, as its last reading has them. */
export function specimenLimits(machine: string, agent: string, dir: string): { five: number; week: number; resetsIn: number } | null {
  const specimen = SPECIMENS[`${machine}|${agent}|${dir}`];
  return specimen ? { five: Math.round(specimen.five * 1000), week: Math.round(specimen.week * 1000), resetsIn: specimen.resetsIn } : null;
}
