// A listed login on the agents page (`world-gui.md`, 智能体页): a row with
// who it is and its plan, and who lets it assist the reader, and with the
// workbench on, a bar for each usage window, what it used this week and its
// last seventeen weeks as a small calendar; the row opens the login's own
// page of usage. Its 「⋯」 holds the rest, whom it assists among it: a cross
// on a row is too easily pressed by one opening it.
//
// To an account the login assists, the bars are its own allowances, not
// the login's account's windows, which are its owner's.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AgentLimits, Fact, LimitWindow } from "../world/agents";
import { isReset } from "./agent-homes";
import type { AssistSeen } from "../world/accounts";
import type { FoundProgram, Login } from "../world/logins";
import { wholeDollars } from "../world/money";
import type { AccessWords } from "./account-words";
import type { AgentLoginsWords } from "./agent-logins-words";
import { link } from "./places";
import { miniCalendar } from "./usage/draw";
import { byDay, dayOf, valueOf, type Usage } from "./usage/model";
import type { UsageWords } from "./usage/words";

type Agent = "codex" | "claude";

/** The oldest Claude Code the World starts conversations in
 *  (`fastctx_hosts::claude::OLDEST`). */
export const CLAUDE_OLDEST = "2.1.300";

/** The newest version among the Claude Code programs a login uses, where
 *  every one whose version is known is older than the World works with. */
export function claudeTooOld(agent: Agent, programs: FoundProgram[], login: Login): string | null {
  if (agent !== "claude") return null;
  const versions = login.programs.map((index) => programs[index]?.version).filter((version): version is string => !!version);
  if (!versions.length) return null;
  const newest = versions.reduce((best, version) => (newer(version, best) ? version : best));
  return newer(CLAUDE_OLDEST, newest) ? newest : null;
}

/** Whether version `a` is later than `b`, by their dotted numbers. */
function newer(a: string, b: string): boolean {
  const parts = (version: string) => (/^\D*(\d+(?:\.\d+)*)/.exec(version)?.[1] ?? "0").split(".").map(Number);
  const [x, y] = [parts(a), parts(b)];
  for (let index = 0; index < Math.max(x.length, y.length); index += 1) {
    const difference = (x[index] ?? 0) - (y[index] ?? 0);
    if (difference) return difference > 0;
  }
  return false;
}

export interface RowFacts {
  machine: string;
  agent: Agent;
  login: Login;
  /** The programs that use it, a line each, for its mark's hover. */
  versions: string;
  /** The newest Claude Code it uses, where that is older than the World
   *  starts conversations in. */
  old: string | null;
  plan: string | null;
  /** The assistance it gives the reader, where that is how they use it. */
  assistsMe: { assist: AssistSeen; owner: string | null } | null;
  /** Its usage as last read; none before the first read. */
  usage: Usage | null;
  workbench: boolean;
  fresh: string;
  note: { text: string; detail: string } | undefined;
  now: Date;
}

export interface RowWords {
  words: AgentLoginsWords;
  access: AccessWords;
  usage: UsageWords;
  when(at: Date, now: Date): string;
}

export function loginRow(facts: RowFacts, { words, access, usage: usageWords, when }: RowWords): string {
  const { machine, agent, login, workbench, now } = facts;
  const identity = login.identity?.who.kind === "known" ? login.identity.who.value : null;
  const token = identity?.method === "oauth_token";
  const who = identity?.email ?? dirName(login.dir);
  const changing = login.change && !login.change.end;
  const at = `data-machine="${escape(machine)}" data-dir="${escape(login.dir)}"`;
  const tags = [
    facts.plan ? `<span class="tag">${escape(planName(facts.plan))}</span>` : "",
    token ? `<span class="tag tag-token" title="${escape(words.tokenSaid)}">${escape(words.token)}</span>` : "",
    identity && !identity.logged_in ? `<span class="tag tag-warn">${escape(words.loggedOut)}</span>` : "",
    facts.old ? `<span class="tag tag-warn" title="${escape(words.tooOldSaid(facts.old, CLAUDE_OLDEST))}">${escape(words.tooOld)}</span>` : "",
    login.drift && !changing ? driftTag(machine, login, words) : "",
  ].join("");
  const mine = facts.assistsMe ? assistedTag(facts.assistsMe, now, words, access, usageWords) : "";
  const stamp = changing ? `<span class="logins-stamp busy">${escape(login.change!.into ? words.applying : words.removing)}</span>` : "";
  const sub = stamp || mine ? `<div class="logins-row-sub">${stamp}${mine}</div>` : "";
  const href = link({ kind: "usage", machine, agent, dir: login.dir });
  const name = workbench
    ? `<a class="logins-row-who-name" href="${href}" title="${escape(`${words.openUsage}\n${login.dir}`)}">${escape(who)}</a>`
    : `<span class="logins-row-who-name" title="${escape(login.dir)}">${escape(who)}</span>`;
  const more = `<button class="icon-button logins-more" type="button" data-act="menu" ${at} data-menu-key="${escape(JSON.stringify([machine, login.dir]))}" aria-haspopup="menu" aria-label="${escape(words.more)}" title="${escape(words.more)}">${glyph("more", 16)}</button>`;
  const head = `<div class="logins-row-who"><div class="logins-row-name"><span class="logins-slip-mark" title="${escape(facts.versions)}">${glyph(agent, 18)}</span>${name}${tags}</div>${sub}</div>`;
  const note = facts.note ? `<p class="feature-note logins-row-note" title="${escape(facts.note.detail)}">${escape(facts.note.text)}</p>` : "";
  if (!workbench) return `<li class="logins-row plain${changing ? " changing" : ""}${facts.fresh}>${head}${more}${note}</li>`;
  return `<li class="logins-row${changing ? " changing" : ""}${facts.fresh} data-open="${href}">
    ${head}
    ${facts.assistsMe ? allowanceBars(facts, usageWords, words) : bars(agent, login.limits, now, words, when)}
    ${week(facts, usageWords, words)}
    <div class="logins-row-mini">${miniCalendar(facts.usage ? byDay(facts.usage, facts.usage.usd ? "usd" : "tokens") : new Map(), dayOf(now.getTime()), `${machine}:${login.dir}`)}</div>
    ${more}${note}
  </li>`;
}

/** What it used this week: dollars first for Claude Code, then tokens. */
function week(facts: RowFacts, usageWords: UsageWords, words: AgentLoginsWords): string {
  const { usage } = facts;
  const key = facts.assistsMe ? words.myWeek : words.week;
  if (!usage) return `<div class="logins-row-week"><span class="logins-row-key">${escape(key)}</span><span class="logins-row-big quiet">–</span></div>`;
  const tokens = usage.people.reduce((sum, person) => sum + person.week.tokens, 0);
  if (usage.usd) {
    const usd = usage.people.reduce((sum, person) => sum + valueOf(person.week, "usd"), 0);
    return `<div class="logins-row-week"><span class="logins-row-key">${escape(key)}</span><span class="logins-row-big" title="${escape(usageWords.aboutWhy)}">${escape(usageWords.money(usd))}</span><span class="logins-row-small">${escape(usageWords.tokens(tokens))} ${escape(usageWords.tokenUnit)}</span></div>`;
  }
  return `<div class="logins-row-week"><span class="logins-row-key">${escape(key)}</span><span class="logins-row-big">${escape(usageWords.tokens(tokens))}</span><span class="logins-row-small">${escape(usageWords.tokenUnit)}</span></div>`;
}

/** A row for each usage window: its name, a short bar, how much of it is
 *  used and how long until it starts again, in columns shared by the
 *  rows; the rest a hover away. A window never read is a dashed empty
 *  bar; one past the reset it was read before, an empty bar that says it
 *  has reset where its time would be. */
export function bars(agent: Agent, limits: Fact<AgentLimits>, now: Date, words: AgentLoginsWords, when: (at: Date, now: Date) => string): string {
  if (limits.kind !== "known") {
    const why = limits.kind === "failed" ? limits.reason : words.notRead(agent);
    return `<div class="logins-bars" title="${escape(why)}"><span class="logins-bar unread"><span class="logins-bar-name">5h</span><span class="logins-bar-track"></span><span class="logins-bar-used"></span><span class="logins-bar-reset"></span></span><span class="logins-bar unread"><span class="logins-bar-name">7d</span><span class="logins-bar-track"></span><span class="logins-bar-used"></span><span class="logins-bar-reset"></span></span></div>`;
  }
  const value = limits.value;
  const credits = value.credits;
  const read = words.observed(when(new Date(value.observed_at), now));
  const rows = value.windows.map((window) => {
    // A whole number, rounded: hosts often give no more.
    const percent = `${Math.round(window.used_tenths / 10)}%`;
    const name = escape(words.window(window.name, window.minutes ?? null));
    if (isReset(window, now.getTime())) {
      const said = [words.reset, words.usedBefore(percent), read].join("\n");
      return `<span class="logins-bar reset" title="${escape(said)}"><span class="logins-bar-name">${name}</span><span class="logins-bar-track"></span><span class="logins-bar-used"></span><span class="logins-bar-reset">${escape(words.reset)}</span></span>`;
    }
    const said = [words.used(percent), window.resets_at ? words.resets(new Date(window.resets_at), now) : "", read, credits ? words.credits(credits.balance ?? null, credits.unlimited) : ""].filter(Boolean).join("\n");
    const width = Math.min(100, window.used_tenths / 10).toFixed(1);
    const onCredits = window.used_tenths >= 1000 && !!credits;
    const left = window.resets_at ? words.resetsIn(new Date(window.resets_at), now) : "";
    const used = `${escape(percent)}${onCredits ? `<span class="logins-credits" title="${escape(words.creditsMark)}">¢</span>` : ""}`;
    return `<span class="logins-bar${levelOf(window)}" title="${escape(said)}"><span class="logins-bar-name">${name}</span><span class="logins-bar-track"><span style="width:${width}%"></span></span><span class="logins-bar-used">${used}</span><span class="logins-bar-reset">${left ? `${glyph("history", 12)}${escape(left)}` : ""}</span></span>`;
  });
  return `<div class="logins-bars">${rows.join("")}</div>`;
}

/** The allowances the reader has of a login that assists it, a bar each
 *  as the windows have theirs: what it used of each, the week's with the
 *  login's own reset. One without allowances has none to draw; one whose
 *  use is not read yet, empty dashed bars. */
function allowanceBars(facts: RowFacts, usageWords: UsageWords, words: AgentLoginsWords): string {
  const { assist } = facts.assistsMe!;
  const { usage, now } = facts;
  const me = usage?.people.find((person) => person.me) ?? null;
  const row = (name: string, spent: number | null, cap: number, left: string, said: string): string => {
    if (spent === null) return `<span class="logins-bar unread" title="${escape(said)}"><span class="logins-bar-name">${escape(name)}</span><span class="logins-bar-track"></span><span class="logins-bar-used"></span><span class="logins-bar-reset"></span></span>`;
    const share = spent / cap;
    const level = share >= 1 ? " spent" : share >= 0.9 ? " high" : "";
    return `<span class="logins-bar${level}" title="${escape(said)}"><span class="logins-bar-name">${escape(name)}</span><span class="logins-bar-track"><span style="width:${Math.min(100, share * 100).toFixed(1)}%"></span></span><span class="logins-bar-used">${Math.round(share * 100)}%</span><span class="logins-bar-reset">${left ? `${glyph("history", 12)}${escape(left)}` : ""}</span></span>`;
  };
  const rows: string[] = [];
  if (assist.daily) {
    const cap = assist.daily / 1_000_000;
    const spent = usage ? (me?.day?.usd ?? 0) : null;
    rows.push(row(usageWords.aDay, spent, cap, "", usageWords.dayCapSaid(wholeDollars(cap), usageWords.money(spent ?? 0))));
  }
  if (assist.weekly) {
    const cap = assist.weekly / 1_000_000;
    const spent = usage ? (me?.week.usd ?? 0) : null;
    const left = usage?.week.window ? words.resetsIn(new Date(usage.week.end), now) : "";
    rows.push(row(usageWords.aWeek, spent, cap, left, usageWords.weekCapSaid(wholeDollars(cap), usageWords.money(spent ?? 0))));
  }
  return `<div class="logins-bars">${rows.join("")}</div>`;
}

/** The assistance the reader has of it: from whom and until when; what it
 *  may spend, its bars say, and the rest a hover away. */
function assistedTag(mine: { assist: AssistSeen; owner: string | null }, now: Date, words: AgentLoginsWords, access: AccessWords, usageWords: UsageWords): string {
  const { assist, owner } = mine;
  const until = assist.until ? new Date(assist.until) : null;
  const weekly = assist.weekly ? assist.weekly / 1_000_000 : null;
  const daily = assist.daily ? assist.daily / 1_000_000 : null;
  const said = [
    usageWords.assistUntil(until, now),
    daily ? usageWords.daily(wholeDollars(daily)) : "",
    weekly ? usageWords.weekly(wholeDollars(weekly)) : "",
    daily || weekly ? "" : usageWords.noCap,
    assist.five_hour ? usageWords.fiveHourCap(assist.five_hour) : "",
    assist.turns ? usageWords.turnsCap(assist.turns) : "",
  ]
    .filter(Boolean)
    .join("\n");
  return `<span class="tag logins-assisted" title="${escape(said)}">${escape(words.assistedFrom(owner ?? "–"))}${until ? `<span class="assist-tag-until">${glyph("history", 12)}${escape(access.moment.short(until, now))}</span>` : ""}</span>`;
}

/** The World's tools in it could not be brought up to date by themselves:
 *  why on hover, and pressed, the card that puts them in again. */
function driftTag(machine: string, login: Login, words: AgentLoginsWords): string {
  const drift = login.drift!;
  const said = drift.kind === "changed" ? words.driftChanged : words.driftAsks;
  const detail = drift.kind === "changed" ? `${drift.location}\n${drift.reason}` : `${drift.limit.location}: ${drift.limit.current} → ${drift.limit.requested}`;
  return `<button class="tag tag-warn logins-drift" type="button" data-act="drift" data-machine="${escape(machine)}" data-dir="${escape(login.dir)}" title="${escape(detail)}">${escape(said)}</button>`;
}

export function dirName(dir: string): string {
  return dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? dir;
}

export function planName(plan: string): string {
  return plan.length ? plan[0]!.toUpperCase() + plan.slice(1) : plan;
}

export function programLine(program: FoundProgram, words: AgentLoginsWords): string {
  const what = program.within ? words.within(program.within.name, program.within.version ?? null) : words.how[program.how];
  return [program.path ?? program.within?.path ?? "", program.version ?? "", what, program.on_path ? words.onPath : ""].filter(Boolean).join("\n");
}

function levelOf(window: LimitWindow): string {
  return window.used_tenths >= 1000 ? " spent" : window.used_tenths >= 900 ? " high" : "";
}
