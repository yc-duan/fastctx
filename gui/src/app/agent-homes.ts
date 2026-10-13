// An agent's homes on a machine (`world-gui.md`, 账号、机器、共享与协助): each is
// one login of that agent, Codex's `CODEX_HOME` or Claude Code's
// `CLAUDE_CONFIG_DIR`, and the machine answers once for each. Who is logged
// in there, the plan, and how much of each usage window is spent are said as
// the agent itself read them on that machine, with when; a window not read
// is not shown, never shown as unused.

import { escape } from "../conversation/prose";
import type { AgentFacts, AgentLimits, Fact, LimitWindow } from "../world/agents";

/** The homes of `agent` in what a machine said, its default one first. */
export function homesOf(facts: AgentFacts[], agent: string): AgentFacts[] {
  const all = facts.filter((f) => f.agent === agent);
  return [...all.filter(isDefault), ...all.filter((f) => !isDefault(f))];
}

/** A machine that knew one home per agent answered for its default one. */
export function isDefault(facts: AgentFacts): boolean {
  return !facts.home || facts.home.found === "default";
}

/** The home as the World is told it: its directory, or null for the
 *  default one, which follows the user's environment there. */
export function homeChoice(facts: AgentFacts): string | null {
  return isDefault(facts) ? null : facts.home!.dir;
}

/** The last name of a directory: ".codex-work". */
export function dirName(dir: string): string {
  const parts = dir.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? dir;
}

export function installed(facts: AgentFacts): boolean {
  return !(facts.program.kind === "known" && (!facts.program.value || !facts.program.value.exists));
}

export function loggedOut(facts: AgentFacts): boolean {
  return facts.login.kind === "known" && facts.login.value.kind === "logged_out";
}

/** A login in its machine's agent list: one the World's tools are put
 *  into, the only kind the World uses. */
export function listed(facts: AgentFacts): boolean {
  return facts.integration.kind === "known" && facts.integration.value === "in";
}

/** A login an agent can start in: its program is there and it is not
 *  known to be logged out. */
export function usableHome(facts: AgentFacts): boolean {
  return installed(facts) && !loggedOut(facts);
}

/** The program's version, as numbers: "0.160.0". */
export function versionOf(facts: AgentFacts): string | null {
  return facts.version.kind === "known" ? (/\d+(?:\.\d+)+/.exec(facts.version.value)?.[0] ?? facts.version.value) : null;
}

/** Who is logged in there, as the agent says: the address and the plan,
 *  either of which it may not say. */
export function accountOf(facts: AgentFacts): { email: string | null; plan: string | null } {
  const account = facts.account?.kind === "known" ? facts.account.value : null;
  return { email: account?.email ?? null, plan: account?.plan ? planName(account.plan) : null };
}

/** "plus" as the agents' own pages write it: "Plus". */
export function planName(plan: string): string {
  return plan.charAt(0).toUpperCase() + plan.slice(1);
}

/** A home in a few words: who is logged in there, else the directory's
 *  name. */
export function homeLabel(facts: AgentFacts, words: HomeWords): string {
  return accountOf(facts).email ?? (facts.home ? dirName(facts.home.dir) : words.defaultHome);
}

export interface HomeWords {
  defaultHome: string;
  /** How the World knows of a home. */
  found: string;
  added: string;
  /** One usage window: "5 小时". `minutes` is its length, when said. */
  window(name: string, minutes: number | null): string;
  /** When the window starts afresh, from now. */
  resets(at: Date, now: Date): string;
  /** When the agent read the numbers: "14:05" or "昨天" from `when`. */
  observed(when: string): string;
  /** What the account goes on with once a window is used up. */
  credits(balance: string | null, unlimited: boolean): string;
  /** No session of this Claude home has read its limits yet. */
  notRead: string;
  limitsFailed(why: string): string;
}

/** A window's length, from its minutes or the agent's own name for it. */
function length(name: string, minutes: number | null): { n: number; unit: "minute" | "hour" | "day" } | null {
  const m = minutes ?? ({ five_hour: 300, seven_day: 10080 } as Record<string, number>)[name] ?? null;
  if (m === null) return null;
  return m % 1440 === 0 ? { n: m / 1440, unit: "day" } : m % 60 === 0 ? { n: m / 60, unit: "hour" } : { n: m, unit: "minute" };
}

const pad = (n: number) => String(n).padStart(2, "0");

export const zhHomes: HomeWords = {
  defaultHome: "默认",
  found: "自动发现",
  added: "手动添加",
  window: (name, minutes) => {
    const model = /^seven_day_(\w+)$/.exec(name);
    if (model) return `7 天 · ${planName(model[1]!)}`;
    const span = length(name, minutes);
    return span ? `${span.n} ${{ minute: "分钟", hour: "小时", day: "天" }[span.unit]}` : name === "spend_limit" ? "花费上限" : name;
  },
  resets: (at, now) => {
    const minutes = Math.round((at.getTime() - now.getTime()) / 60000);
    if (minutes < 1) return "马上重置";
    if (minutes < 60) return `${minutes} 分钟后重置`;
    if (minutes < 24 * 60) return `${Math.floor(minutes / 60)} 小时${minutes % 60 ? ` ${minutes % 60} 分` : ""}后重置`;
    const clock = `${pad(at.getHours())}:${pad(at.getMinutes())}`;
    if (minutes < 7 * 24 * 60) return `周${"日一二三四五六"[at.getDay()]} ${clock} 重置`;
    return `${at.getMonth() + 1}月${at.getDate()}日重置`;
  },
  observed: (when) => `${when} 更新`,
  credits: (balance, unlimited) => (unlimited ? "超出后使用 credits（不限）" : balance ? `超出后使用 credits（余 ${balance}）` : "超出后使用 credits"),
  notRead: "运行一轮后显示额度",
  limitsFailed: () => "无法获取额度",
};

export const enHomes: HomeWords = {
  defaultHome: "default",
  found: "detected",
  added: "added manually",
  window: (name, minutes) => {
    const model = /^seven_day_(\w+)$/.exec(name);
    if (model) return `7 days · ${planName(model[1]!)}`;
    const span = length(name, minutes);
    return span ? `${span.n} ${span.unit}${span.n === 1 ? "" : "s"}` : name === "spend_limit" ? "spend limit" : name;
  },
  resets: (at, now) => {
    const minutes = Math.round((at.getTime() - now.getTime()) / 60000);
    if (minutes < 1) return "resets now";
    if (minutes < 60) return `resets in ${minutes} min`;
    if (minutes < 24 * 60) return `resets in ${Math.floor(minutes / 60)} h${minutes % 60 ? ` ${minutes % 60} min` : ""}`;
    if (minutes < 7 * 24 * 60) return `resets ${at.toLocaleString("en", { weekday: "short", hour: "2-digit", minute: "2-digit", hour12: false })}`;
    return `resets ${at.toLocaleString("en", { month: "short", day: "numeric" })}`;
  },
  observed: (when) => `updated ${when}`,
  credits: (balance, unlimited) =>
    unlimited ? "then credits, unlimited" : balance ? `then credits, ${balance} left` : "then credits",
  notRead: "Limits appear after one turn",
  limitsFailed: () => "Couldn't load limits",
};

/** How full a window is: past nine tenths it is said with care, and spent
 *  once it is full. */
function level(window: LimitWindow): string {
  return window.used_tenths >= 1000 ? " spent" : window.used_tenths >= 900 ? " high" : "";
}

/** A balance as a whole number with its digits grouped: "52,230". */
function amount(balance: string): string {
  const value = Number(balance);
  return Number.isFinite(value) ? Math.round(value).toLocaleString("en") : balance;
}

function percent(window: LimitWindow): string {
  return window.used_tenths > 0 && window.used_tenths < 10 ? "<1%" : `${Math.round(window.used_tenths / 10)}%`;
}

/** A home's usage windows in a few characters each, as Claude Code's own
 *  status line writes them: "5h", "7d" and how much is used; the first
 *  limit's windows only, for those of another limit and of one model are
 *  named on the machines page. */
export function windowsShort(limits: Fact<AgentLimits> | undefined): { label: string; percent: string; level: "" | "high" | "spent" }[] {
  if (limits?.kind !== "known") return [];
  const first = limits.value.windows[0]?.name;
  return limits.value.windows
    .filter((window) => window.name === first || /^(five_hour|seven_day)$/.test(window.name))
    .map((window) => {
      const span = length(window.name, window.minutes ?? null);
      const label = span ? `${span.n}${{ minute: "m", hour: "h", day: "d" }[span.unit]}` : window.name;
      return { label, percent: percent(window), level: window.used_tenths >= 1000 ? "spent" : window.used_tenths >= 900 ? "high" : "" };
    });
}

/** The usage windows of one home, a bar each, and when they were read.
 *  Windows of another limit than the first are named by it. `when` says
 *  a past moment as the page does. */
export function limitsHtml(
  limits: Fact<AgentLimits> | undefined,
  words: HomeWords,
  now: Date,
  when: (at: Date, now: Date) => string,
  quiet: { notRead: boolean },
): string {
  if (!limits) return "";
  if (limits.kind === "failed") return `<p class="home-limits-said" title="${escape(limits.reason)}">${escape(words.limitsFailed(limits.reason))}</p>`;
  if (limits.kind === "unavailable") return quiet.notRead ? `<p class="home-limits-said">${escape(words.notRead)}</p>` : "";
  const windows = limits.value.windows;
  if (!windows.length) return "";
  // Claude Code names each window; Codex names each limit, whose windows
  // its length tells apart: a window of another limit than the first is
  // named by that limit.
  const first = windows[0]!.name;
  const rows = windows.map((window) => {
    const name = words.window(window.name, window.minutes ?? null);
    const claude = /^(five_hour|seven_day|spend_limit)/.test(window.name);
    const label = claude || window.name === first || window.minutes === undefined ? name : `${window.name} · ${name}`;
    const width = Math.min(100, window.used_tenths / 10).toFixed(1);
    const reset = window.resets_at ? `<span class="limit-reset">${escape(words.resets(new Date(window.resets_at), now))}</span>` : "";
    return `<span class="limit${level(window)}"><span class="limit-name">${escape(label)}</span><span class="limit-bar" aria-hidden="true"><span style="width:${width}%"></span></span><span class="limit-used">${escape(percent(window))}</span>${reset}</span>`;
  });
  const credits = limits.value.credits;
  const balance = credits?.balance !== undefined ? amount(credits.balance) : null;
  const more = credits ? `<span class="limit-credits">${escape(words.credits(balance, credits.unlimited))}</span>` : "";
  const read = `<span class="limit-seen">${escape(words.observed(when(new Date(limits.value.observed_at), now)))}</span>`;
  return `<div class="home-limits">${rows.join("")}${more}${read}</div>`;
}
