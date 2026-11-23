// What the agents' pages draw, wherever it comes from: the machines, the
// agents found on each, their logins, the people some logins assist, and,
// fetched when a page wants them, each login's usage and ledger. The
// specimen fills it from a made-up network (`fake.ts`), the app from the
// World (`live.ts`); the pages only read it, and ask what they do of the
// deeds the host gives (`deeds`).
//
// It follows how the real one is kept: a login lives on one machine and has
// one ledger; a request is the machine's own (`who` none) unless the
// conversation that made it works for an assisted account; a conversation
// is named only to the one it belongs to.

import type { IdleCompaction } from "../world/logins";
import type { RetentionAnswer } from "../world/retention";

export type Family = string;
const named: Record<string, string> = { claude: "Claude Code", codex: "Codex", antigravity: "Antigravity", cursor: "Cursor", opencode: "OpenCode" };
export const familyName = (family: Family): string => named[family] ?? family;
/** The agents the workbench talks with; the others are only connected. */
export const converses = (family: Family) => family === "claude" || family === "codex";

export interface Machine {
  id: string;
  kind: "desktop" | "laptop" | "server";
  here: boolean;
  online: boolean;
  /** Mine, or one whose agent assists me, named by its owner. */
  owner: string | null;
  scanned: boolean;
  /** Why it did not answer, where it is on but said nothing. */
  silent?: string;
  /** A scan running there now. */
  scanning?: boolean;
  /** Why its last scan did not finish. */
  scanFailed?: string;
  /** The folders searched there for logins, with how each search stands. */
  folders?: { dir: string; said: string; busy: boolean }[];
  /** The agents it has none of, said when it was scanned. */
  missing?: string[];
}

export interface Reading {
  used: number;
  /** When it starts over, as the page says it. */
  resets: string;
  /** Cleared by the vendor before its end; drawn empty. */
  reset?: boolean;
}

/** The plans a long-lived token's login can be marked with by the
 *  administrator, the token naming none (用户 2026-11-15: Ultra is
 *  Anthropic's newer name for what was Max 20x). */
export type PlanLabel = "max" | "ultra" | "pro" | "free";
export const planNames: Record<PlanLabel, string> = { max: "Max", ultra: "Ultra", pro: "Pro", free: "Free" };

export interface Login {
  id: string;
  family: Family;
  machine: string;
  dir: string;
  /** The account's email, or the directory's name where none is known. */
  who: string;
  /** The plan as shown: as the agent named it, or as set for a token. */
  plan: string | null;
  standing: "in" | "found" | "renew";
  workbench: boolean;
  isDefault?: boolean;
  /** Claude Code logged in with a long-lived token: no account, no plan. */
  token?: boolean;
  /** The plan the administrator set for a token's login. */
  planSet?: PlanLabel | null;
  /** The folder it was found under, where it is not in the usual place. */
  under?: string;
  five?: Reading;
  week?: Reading;
  /** Why its windows are not shown, where they were not read. */
  unread?: string;
  /** What Codex may still spend past its windows. */
  credits?: string;
  /** Fable's part of the week as the ledger reckons it, of a hundred, and
   *  whether the whole login's line has stopped it. */
  fable?: number;
  fableHeld?: boolean;
  /** Another person's login that assists me. `limit` is the part of its
   *  week I may use, of a hundred. Where its owner has me manage it with
   *  them (`manages`) I see the account whole and set the shares of those
   *  it assists, my own among them, and may have no limit; otherwise I see
   *  only my part. `used` and `fable` are what I used, of a hundred of its
   *  week. */
  lent?: { by: string; limit: number | null; used: number; fable: number; until: string | null; manages?: boolean };
  /** What keeps it from working: logged out, its program too old, its
   *  connection changed by hand, a change under way. */
  loggedOut?: boolean;
  tooOld?: string;
  drift?: string;
  busy?: string;
  /** Its program, as found. */
  program?: string;
  /** Codex: new conversations are named as the official client names them. */
  autoTitle?: boolean;
  /** The models a conversation in it starts on; whether they were changed. */
  models?: string[];
  modelsChanged?: boolean;
  /** Compacting conversations idle a while, as the reader set it for this
   *  login; absent where they never did. */
  idle?: IdleCompaction;
  /** When its windows were last read, as the page says it. */
  readAt?: string;
  /** Whether its machine takes direct connections: the machines of those
   *  it assists link with it directly then, each as its owner says. */
  allowsDirect?: boolean;
  /** Whether its machine is the reader's own, not one shared with them
   *  whole: only its owner says whose machines link with it directly. */
  ownMachine?: boolean;
}

export interface Grant {
  login: string;
  person: string;
  /** Percent of the login's week this person may use; none for no limit,
   *  which only one who manages the login with its owner may have. */
  limit: number | null;
  /** They manage the login with its owner: they see the account whole and
   *  set the shares of those it assists, and nothing else. */
  manages?: boolean;
  /** The specimen's own figures of what they used, of a hundred of the
   *  week; the app takes them from the login's usage. */
  used: number;
  fable: number;
  until: string | null;
  paused?: boolean;
  /** False once the login's owner said their machines do not link with its
   *  machine directly: all between them goes through the hub then. */
  direct?: boolean;
}

/** What holds for everyone a login assists. */
export interface Line {
  fiveStop: number | null;
  turns: number | null;
}

/** A login's week in quota dollars, which its percents are reckoned of,
 *  where its owner expected none. */
export const PLATE = 3000;
/** How far a week's figure may stand from what its owner expects, of a
 *  hundred either way, where they set no other. */
export const SWING = 10;
/** The widest reach its owner may set, of a hundred either way. */
export const WIDEST = 20;
/** The reader themselves, where a deed takes a person: one who manages a
 *  login with its owner sets their own share by it. */
export const ME = "@me";

/** What a login's owner expects its week to hold, in quota dollars, and how
 *  far the figure reckoned for a week may stand from it. The week's own
 *  figure is fixed as the week begins, from what the week before came to by
 *  the readings, held within that reach. */
export interface Expect {
  base: number;
  swing: number;
}

/** A reset of the week's usage its owner made by hand: to nothing, or to a
 *  part of the week, everyone's share of it scaled alike. */
export interface Reset {
  at: string;
  how: string;
  from: number;
  to: number;
  /** The owner's own and the newest: it can be taken back. */
  undo?: boolean;
}

export type Outcome = "done" | "failed" | "stopped" | "held";
export type Source = "main" | "agent" | "background";

export interface Call {
  at: Date;
  /** The assisted person it worked for; none is the machine's own. */
  who: string | null;
  model: string;
  effort: string;
  source: Source;
  /** A subagent's name. */
  agent: string | null;
  /** The model's multiplier this request was reckoned by. */
  times: number;
  fresh: number;
  cacheRead: number;
  cacheWrite: number;
  output: number;
  quota: number;
  api: number;
  first: number;
  took: number;
  outcome: Outcome;
  /** The conversation's name, to the one it belongs to. */
  talk: string | null;
  /** Where its conversation can be opened, to the one it belongs to. */
  opens?: { conversation: string; turn: string } | null;
}

export type Split = "model" | "person" | "kind" | "source";
export const splitNames: Record<Split, string> = { model: "型号", person: "人", kind: "输入与输出", source: "来源" };
export type Unit = "quota" | "tokens";

export interface Series {
  names: string[];
  values: number[][];
}

/** What a login used, as its usage page draws it. */
export interface Days {
  dates: Date[];
  /** A day's whole, in each unit. */
  total: Record<Unit, number[]>;
  /** A day's parts, in each unit, for each way of splitting them the
   *  login's ledger can tell. */
  parts: Record<Unit, Partial<Record<Split, Series>>>;
  /** By weekday (Monday first) and hour, over the last 90 days; requests,
   *  or quota dollars where the counts are not kept. */
  hours: number[][];
  hoursAre: "requests" | "quota" | "tokens";
  /** This week's quota dollars by person: none is the login's owner, and
   *  `me` the reader where the login is another's. */
  people: { name: string | null; week: number; fable: number; me?: boolean }[];
  /** What each of its last whole weeks came to by the readings, the newest
   *  last, for the owner to see where what they expect stands. */
  weeks: { start: Date; value: number }[];
  /** The resets on record, the newest first; the owner's alone. */
  resets: Reset[] | null;
  /** Of a hundred of the week, how far the vendor's reading stands above
   *  what the ledger comes to, or below it. */
  gap: number;
  /** When the week these are of ends, as its machine said, and whose it
   *  is; a reset is of this week. */
  weekEnd?: string | null;
  holder?: string | null;
  /** When the vendor's week starts over, as the reading its usage is
   *  reckoned by says it: told to everyone the login works for, its windows
   *  to those who see it whole alone. None where no reading told it. */
  resetsAt?: Date | null;
}

/** The week a reset is asked of, as the sheet that asks it showed it. */
export interface Looked {
  weekEnd: string | null;
  holder: string | null;
}

/** Whether a gap between reading and ledger is told: past what the week's
 *  figure and a change in how the login is used can make of themselves. */
export const gapTold = (gap: number, read: number): boolean => Math.abs(gap) > Math.max(5, read * 0.15);

/** A page of a login's requests asked for. */
export interface CallsQuery {
  page: number;
  size: number;
  from: Date | null;
  until: Date | null;
  who: string | null;
  model: string | null;
  source: Source | null;
  outcome: Outcome | null;
  /** One turn's requests alone. */
  turn?: { conversation: string; turn: string } | null;
}

export interface CallsPage {
  rows: Call[];
  /** All the rows the query matches, where the ledger counts them. */
  total: number | null;
  /** What all of them add up to, where the ledger says. */
  sum: { input: number; output: number; quota: number } | null;
  /** The models and people seen in the ledger, to filter by. */
  models: string[];
  people: string[];
}

/** What the host fetches when a page wants it. */
export interface Feed {
  days(login: Login): Promise<Days>;
  calls(login: Login, query: CallsQuery): Promise<CallsPage>;
}

/** What a press asks of the host. Each answers once the World has, the page
 *  drawn again from what it now holds. */
export interface Deeds {
  attach(login: Login): void;
  detach(login: Login): void;
  bench(login: Login, open: boolean): void;
  scan(machine: Machine): void;
  addFolder(machine: Machine): void;
  /** Stops a folder's search, or no longer searches it. */
  folder(machine: Machine, dir: string, change: "stop" | "forget"): void;
  newTalk(login: Login): void;
  openTurn(login: Login, call: Call): void;
  reread(login: Login): void;
  models(login: Login): void;
  autoTitle(login: Login, on: boolean): Promise<void>;
  /** Compacting the reader's idle conversations in it: turned on or off,
   *  or its conditions changed in a sheet of their own. */
  idleOn(login: Login, on: boolean): Promise<void>;
  idle(login: Login): void;
  /** How long the machine of a login lent to the reader keeps their idle
   *  conversations: read, or set fewer days than its owner's for their own
   *  (`null` follows the owner's). */
  retention?(login: Login, days?: number | null): Promise<RetentionAnswer>;
  plan(login: Login, plan: PlanLabel | null): Promise<void>;
  /** What the owner expects the login's week to hold, and its reach. */
  expect(login: Login, expect: Expect): Promise<void>;
  /** Resets the week's usage to a part of the week, of a hundred; `id`
   *  names the reset, the same however often it is asked. */
  resetUsage(login: Login, to: number, id: string, looked: Looked): Promise<void>;
  /** Takes the newest reset back. */
  undoReset(login: Login): Promise<void>;
  /** Gives, or changes, what a person may use of the login: their share,
   *  until when, and, where the owner says, whether they manage it too and
   *  whether their machines link with its machine directly. */
  assist(login: Login, person: string, limit: number | null, until: Date | null, manages?: boolean, direct?: boolean): Promise<void>;
  endAssist(login: Login, person: string, shown: string[]): Promise<void>;
  /** The grants the assistance of `person` is, as shown now. */
  assistGrants(login: Login, person: string): string[];
  line(login: Login, line: Line): Promise<void>;
  say(text: string): void;
}

export interface Net {
  now: Date;
  machines: Machine[];
  logins: Login[];
  grants: Grant[];
  lines: Record<string, Line>;
  /** The network's people, for a name typed into "assist whom". */
  people: string[];
  /** Each login's week as reckoned for this week, in quota dollars. */
  plates: Record<string, number>;
  /** What each login's owner expects of its week. */
  expects: Record<string, Expect>;
  /** The reader administers the network: a token's plan is theirs to set. */
  admin: boolean;
  /** No machine's logins have been read yet: what is drawn is being
   *  waited for, not found empty. */
  unread?: boolean;
}

export let NOW = new Date();
export let machines: Machine[] = [];
export let logins: Login[] = [];
export let grants: Grant[] = [];
export let lines: Record<string, Line> = {};
export let people: string[] = [];
export let plates: Record<string, number> = {};
export let expects: Record<string, Expect> = {};
export let admin = false;
export let unread = false;
export let byId = new Map<string, Login>();
export let feed: Feed = { days: () => new Promise(() => {}), calls: () => new Promise(() => {}) };
export let deeds: Deeds | null = null;
/** What the specimen does of itself where the app asks the World: its
 *  made-up network changed in place. */
export interface Sample {
  expect(login: Login, expect: Expect): void;
  resetUsage(login: Login, to: number): void;
  undoReset(login: Login): void;
}
export let sample: Sample | null = null;

/** Takes what the host now holds; the pages are drawn from it next. */
export function setNet(net: Net): void {
  NOW = net.now;
  machines = net.machines;
  logins = net.logins;
  grants = net.grants;
  lines = net.lines;
  people = net.people;
  plates = net.plates;
  expects = net.expects;
  admin = net.admin;
  unread = net.unread ?? false;
  byId = new Map(logins.map((login) => [login.id, login]));
}

export function setHost(given: { feed: Feed; deeds?: Deeds | null; sample?: Sample | null }): void {
  feed = given.feed;
  deeds = given.deeds ?? null;
  sample = given.sample ?? null;
  kept.clear();
}

export const grantsOf = (login: Login) => grants.filter((grant) => grant.login === login.id);
export const plateOf = (login: Login): number => plates[login.id] ?? expects[login.id]?.base ?? PLATE;
/** What is expected of a login's week: as its owner set it, else the week's
 *  own figure with the usual reach. */
export const expectOf = (login: Login): Expect => expects[login.id] ?? { base: plateOf(login), swing: SWING };
/** A week's figure held within the reach of what is expected. */
export const within = (value: number, expect: Expect): number => Math.min(expect.base * (1 + expect.swing / 100), Math.max(expect.base * (1 - expect.swing / 100), value));

/** A model's multiplier: what a dollar of its API price uses of the quota,
 *  beside a dollar of Opus 5.5's. One for every model but Fable, which uses
 *  the quota faster than its price says (用户 2026-11-15). */
export const FABLE_TIMES = 2.5;
export const isFable = (model: string) => /fable/i.test(model);
export const timesOf = (model: string) => (isFable(model) ? FABLE_TIMES : 1);

/** The middle of what a login's weeks came to by the readings, which is
 *  what to expect of it: none where no week has been read yet. A week that
 *  cost nothing in dollars (Codex keeps no prices) tells nothing. */
export function weeksMiddle(weeks: number[]): number | null {
  const sorted = weeks.filter((week) => Number.isFinite(week) && week > 0).sort((a, b) => a - b);
  if (!sorted.length) return null;
  return sorted.length % 2 ? sorted[(sorted.length - 1) / 2]! : (sorted[sorted.length / 2 - 1]! + sorted[sorted.length / 2]!) / 2;
}
/** Which side of its reach the last `run` weeks all came to, where they
 *  did: the owner is then asked to look at what they expect. */
export function pinned(weeks: number[], expect: Expect, run = 2): "low" | "high" | null {
  const last = weeks.slice(-run);
  if (last.length < run) return null;
  if (last.every((week) => week < expect.base * (1 - expect.swing / 100))) return "low";
  if (last.every((week) => week > expect.base * (1 + expect.swing / 100))) return "high";
  return null;
}

/** What has come of a fetch: the answer, its failure, or nothing yet. */
export type Fetched<T> = T | Error | null;

interface Kept {
  at: number;
  value: unknown;
  waiting: boolean;
}
const kept = new Map<string, Kept>();
/** How long a fetched answer is drawn before it is asked for again. */
const FRESH = 60_000;
/** The answers kept at most, the least lately wanted let go first: each
 *  page of calls asked for is one. */
const KEPT_MOST = 64;

/** Keeps `entry` under `key` as the most lately wanted. */
function keepAs(key: string, entry: Kept): void {
  kept.delete(key);
  kept.set(key, entry);
  for (const [old, one] of kept) {
    if (kept.size <= KEPT_MOST) break;
    if (!one.waiting) kept.delete(old);
  }
}
let arrived: (key: string) => void = () => {};
/** What the pages do when something they asked for comes. */
export function whenArrived(then: (key: string) => void): void {
  arrived = then;
}

function fetched<T>(key: string, ask: () => Promise<T>): Fetched<T> {
  const had = kept.get(key);
  const stale = !had || Date.now() - had.at > FRESH;
  if (stale && !had?.waiting) {
    const entry: Kept = { at: had?.at ?? 0, value: had?.value ?? null, waiting: true };
    keepAs(key, entry);
    // An answer to what was forgotten meanwhile is let go.
    const settle = (value: unknown) => {
      if (kept.get(key) !== entry) return;
      keepAs(key, { at: Date.now(), value, waiting: false });
      arrived(key);
    };
    ask().then(settle, (error: unknown) => settle(error instanceof Error ? error : new Error(String(error))));
  } else if (had) keepAs(key, had);
  return (kept.get(key)?.value as Fetched<T>) ?? null;
}

/** A login's usage, fetched when first wanted and again once a minute old. */
export const daysOf = (login: Login): Fetched<Days> => fetched(`days:${login.id}`, () => feed.days(login));
/** A page of a login's requests. */
export const callsOf = (login: Login, query: CallsQuery): Fetched<CallsPage> => fetched(`calls:${login.id}:${JSON.stringify(query)}`, () => feed.calls(login, query));
/** Forgets what was fetched for a login, so it is asked for again. */
export function forget(login?: Login): void {
  for (const key of [...kept.keys()]) if (!login || key.includes(`:${login.id}`)) kept.delete(key);
}
export const usable = <T>(value: Fetched<T>): T | null => (value && !(value instanceof Error) ? value : null);
