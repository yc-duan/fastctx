// An agent's homes on a machine (`world-gui.md`, 账号、机器、共享与协助): each is
// one login of that agent, Codex's `CODEX_HOME` or Claude Code's
// `CLAUDE_CONFIG_DIR`, and the machine answers once for each. Who is logged
// in there, the plan, and how much of each usage window is spent are said as
// the agent itself read them on that machine, with when; a window not read
// is not shown, never shown as unused.

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
}

/** A window's length, from its minutes or the agent's own name for it. */
function length(name: string, minutes: number | null): { n: number; unit: "minute" | "hour" | "day" } | null {
  const m = minutes ?? ({ five_hour: 300, seven_day: 10080 } as Record<string, number>)[name] ?? null;
  if (m === null) return null;
  return m % 1440 === 0 ? { n: m / 1440, unit: "day" } : m % 60 === 0 ? { n: m / 60, unit: "hour" } : { n: m, unit: "minute" };
}

export const zhHomes: HomeWords = {
  defaultHome: "默认",
  found: "自动发现",
  added: "手动添加",
};

export const enHomes: HomeWords = {
  defaultHome: "default",
  found: "detected",
  added: "added manually",
};

/** Whether a window was read before the reset it has passed since: what it
 *  holds now is not known until a session reads it again, so it is said to
 *  have reset, never shown as it was nor as unused. */
export function isReset(window: LimitWindow, now: number): boolean {
  return !!window.resets_at && Date.parse(window.resets_at) <= now;
}

function percent(window: LimitWindow): string {
  return window.used_tenths > 0 && window.used_tenths < 10 ? "<1%" : `${Math.round(window.used_tenths / 10)}%`;
}

/** A home's usage windows in a few characters each, as Claude Code's own
 *  status line writes them: "5h", "7d" and how much is used, or that it has
 *  reset since; the first limit's windows only, for those of another limit
 *  and of one model are named on the machines page. */
export function windowsShort(
  limits: Fact<AgentLimits> | undefined,
  now: number,
): { label: string; percent: string; level: "" | "high" | "spent"; reset: boolean }[] {
  if (limits?.kind !== "known") return [];
  const first = limits.value.windows[0]?.name;
  return limits.value.windows
    .filter((window) => window.name === first || /^(five_hour|seven_day)$/.test(window.name))
    .map((window) => {
      const span = length(window.name, window.minutes ?? null);
      const label = span ? `${span.n}${{ minute: "m", hour: "h", day: "d" }[span.unit]}` : window.name;
      const reset = isReset(window, now);
      const level = reset ? "" : window.used_tenths >= 1000 ? "spent" : window.used_tenths >= 900 ? "high" : "";
      return { label, percent: percent(window), level, reset };
    });
}
