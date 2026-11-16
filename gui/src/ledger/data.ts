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
   *  week I may use, of a hundred; with none I share its whole quota and
   *  see its windows as its owner does, and with one I see only my part.
   *  `used` and `fable` are what I used, of a hundred of its week. */
  lent?: { by: string; limit: number | null; used: number; fable: number; until: string | null };
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
  /** A week the vendor cleared early counts anew from then. */
  resetInPlace?: boolean;
  /** The models a conversation in it starts on; whether they were changed. */
  models?: string[];
  modelsChanged?: boolean;
  /** When its windows were last read, as the page says it. */
  readAt?: string;
}

export interface Grant {
  login: string;
  person: string;
  /** Percent of the login's week this person may use; none for no limit. */
  limit: number | null;
  /** The specimen's own figures of what they used, of a hundred of the
   *  week; the app takes them from the login's usage. */
  used: number;
  fable: number;
  until: string | null;
  paused?: boolean;
}

/** What holds for everyone a login assists. */
export interface Line {
  fiveStop: number | null;
  turns: number | null;
}

/** A login's week in quota dollars, which its percents are reckoned of. Its
 *  owner sets it, 3,000 where they have not (用户 2026-11-15: set by hand
 *  only, for now). */
export const PLATE = 3000;

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
  /** This week's quota dollars by person: none is the login's owner. */
  people: { name: string | null; week: number; fable: number }[];
  /** What each of its last whole weeks came to by the readings, the newest
   *  last, for the owner to see how the figure set for its week stands. */
  weeks: { start: Date; value: number }[];
  /** The weekly resets on record, the newest first; the owner's alone. */
  resets: { at: string; how: string; from: number; to: number }[] | null;
  /** Of a hundred of the week, what the reading counts beyond the ledger. */
  elsewhere: number;
  /** Quota dollars the ledger counts in the five-hour window now open. */
  fiveSpent: number | null;
}

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
  resetInPlace(login: Login, on: boolean): Promise<void>;
  plan(login: Login, plan: PlanLabel | null): Promise<void>;
  plate(login: Login, dollars: number): Promise<void>;
  assist(login: Login, person: string, limit: number | null, until: Date | null): Promise<void>;
  endAssist(login: Login, person: string): Promise<void>;
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
  /** Each login's week as its owner set it, in quota dollars. */
  plates: Record<string, number>;
  /** The reader administers the network: a token's plan is theirs to set. */
  admin: boolean;
}

export let NOW = new Date();
export let machines: Machine[] = [];
export let logins: Login[] = [];
export let grants: Grant[] = [];
export let lines: Record<string, Line> = {};
export let people: string[] = [];
export let plates: Record<string, number> = {};
export let admin = false;
export let byId = new Map<string, Login>();
export let feed: Feed = { days: () => new Promise(() => {}), calls: () => new Promise(() => {}) };
export let deeds: Deeds | null = null;

/** Takes what the host now holds; the pages are drawn from it next. */
export function setNet(net: Net): void {
  NOW = net.now;
  machines = net.machines;
  logins = net.logins;
  grants = net.grants;
  lines = net.lines;
  people = net.people;
  plates = net.plates;
  admin = net.admin;
  byId = new Map(logins.map((login) => [login.id, login]));
}

export function setHost(given: { feed: Feed; deeds?: Deeds | null }): void {
  feed = given.feed;
  deeds = given.deeds ?? null;
  kept.clear();
}

export const grantsOf = (login: Login) => grants.filter((grant) => grant.login === login.id);
export const plateOf = (login: Login): number => plates[login.id] ?? PLATE;

/** A model's multiplier: what a dollar of its API price uses of the quota,
 *  beside a dollar of Opus 5.5's. One for every model but Fable, which uses
 *  the quota faster than its price says (用户 2026-11-15). */
export const FABLE_TIMES = 2.5;
export const isFable = (model: string) => /fable/i.test(model);
export const timesOf = (model: string) => (isFable(model) ? FABLE_TIMES : 1);

/** How a figure set for a login's week stands against its readings: the
 *  middle of what its weeks came to, and how far above or below it the
 *  figure is, of a hundred. None where no week has been read yet. */
export function plateCheck(weeks: number[], set: number): { read: number; off: number } | null {
  if (!weeks.length) return null;
  const sorted = [...weeks].sort((a, b) => a - b);
  const read = sorted.length % 2 ? sorted[(sorted.length - 1) / 2]! : (sorted[sorted.length / 2 - 1]! + sorted[sorted.length / 2]!) / 2;
  return { read, off: Math.round(((set - read) / read) * 100) };
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
    kept.set(key, entry);
    ask().then(
      (value) => {
        kept.set(key, { at: Date.now(), value, waiting: false });
        arrived(key);
      },
      (error: unknown) => {
        kept.set(key, { at: Date.now(), value: error instanceof Error ? error : new Error(String(error)), waiting: false });
        arrived(key);
      },
    );
  }
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
