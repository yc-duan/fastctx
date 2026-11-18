// The World, as the agents' pages draw it (`data.ts`): every machine's
// logins as their nodes recorded them, the assistances and plans the hubs
// keep, a login's usage report and its ledger's rows.

import { pickables, type Pickable } from "../app/agent-machines";
import { claudeTooOld, dirName } from "../app/agent-login-row";
import { sameHome } from "../app/reach";
import type { AssistSeen, HubAccount } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentLimits, Fact, LimitWindow } from "../world/agents";
import type { WorldNow } from "../world/bridge";
import type { AgentLogins, FoundProgram, Login as WorldLogin, MachineLogins, MachineLoginsHeard } from "../world/logins";
import type { UsageReport } from "../world/usage";
import type { CallOutcome, UsageCall, UsageCallsPage, UsageCallsQuery } from "../world/usage-calls";
import { FABLE_TIMES, isFable, planNames, PLATE, SWING, timesOf, type Call, type CallsPage, type CallsQuery, type Days, type Grant, type Line, type Login, type Machine, type Net, type Outcome, type PlanLabel, type Series, type Split, type Unit } from "./data";

/** A login's key in the pages and their addresses. */
export const idOf = (machine: string, agent: string, dir: string) => `${machine}|${agent}|${dir}`;
export function partsOf(id: string): { machine: string; agent: string; dir: string } {
  const [machine = "", agent = "", ...rest] = id.split("|");
  return { machine, agent, dir: rest.join("|") };
}

const DAY = 86400000;
const pad = (n: number) => String(n).padStart(2, "0");

/** When a window starts over, as a row says it: in hours and minutes within
 *  a day, else its weekday and time. */
export function resetsSaid(at: Date, now: Date): string {
  const left = at.getTime() - now.getTime();
  if (left <= 0) return "已重置";
  if (left < DAY) {
    const minutes = Math.round(left / 60000);
    const hours = Math.floor(minutes / 60);
    return hours ? `${hours} 小时 ${pad(minutes % 60)} 分后` : `${minutes} 分后`;
  }
  // Further off, in days and hours: how long is left, not which weekday.
  const hours = Math.round(left / 3600000);
  return hours % 24 ? `${Math.floor(hours / 24)} 天 ${hours % 24} 小时后` : `${hours / 24} 天后`;
}

export function agoSaid(at: Date, now: Date): string {
  const minutes = Math.round((now.getTime() - at.getTime()) / 60000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  if (minutes < 1440) return `${Math.round(minutes / 60)} 小时前`;
  return `${at.getMonth() + 1} 月 ${at.getDate()} 日`;
}

const daySaid = (at: Date) => `${at.getMonth() + 1} 月 ${at.getDate()} 日`;
const momentSaid = (at: Date) => `${daySaid(at)} ${pad(at.getHours())}:${pad(at.getMinutes())}`;

/** The plan as a row shows it: as the agent named it, or for a token's
 *  login, as the administrator marked it. */
const planShown = (plan: string) => plan.charAt(0).toUpperCase() + plan.slice(1);

function windowOf(limits: Fact<AgentLimits>, minutes: number, names: string[]): LimitWindow | undefined {
  if (limits.kind !== "known") return undefined;
  return limits.value.windows.find((one) => one.minutes === minutes) ?? limits.value.windows.find((one) => names.includes(one.name));
}

function reading(window: LimitWindow | undefined, now: Date): Login["five"] {
  if (!window) return undefined;
  const at = window.resets_at ? new Date(window.resets_at) : null;
  const reset = !!at && at.getTime() <= now.getTime();
  return { used: Math.min(100, Math.round(window.used_tenths / 10)), resets: at ? resetsSaid(at, now) : "", ...(reset ? { reset } : {}) };
}

function programSaid(program: FoundProgram): string {
  const name = program.within?.name ?? (program.path ? (program.path.split(/[\\/]/).pop() ?? "").replace(/\.(exe|cmd)$/i, "") : "");
  const how: Record<string, string> = { npm: "npm", pnpm: "pnpm", yarn: "yarn", bun: "bun", official: "官方安装", winget: "winget", scoop: "scoop", chocolatey: "Chocolatey", homebrew: "Homebrew", desktop_app: "桌面应用", extension: "扩展", path: "PATH" } as never;
  return [name, program.version ?? program.within?.version ?? "", how[program.how] ?? program.how, program.on_path ? "在 PATH 中" : ""].filter(Boolean).join("　");
}

export interface Seen {
  /** Each machine's record as last heard, by name. */
  records: Map<string, MachineLoginsHeard>;
  /** Each login's week as its node keeps it, in quota dollars. */
  plates: Map<string, number>;
}

/** The pages' network, from what the World holds now. */
export function netOf(world: WorldNow | null, seen: Seen, now: Date): Net {
  const machines: Machine[] = [];
  const logins: Login[] = [];
  const grants: Grant[] = [];
  const lines: Record<string, Line> = {} as never;
  const plates: Record<string, number> = {} as never;
  const mine = loginsOf(world?.accounts);
  const hubs = world?.accounts.hubs ?? [];
  let admin = false;
  for (const machine of pickables(world)) {
    if (machine.hold === "others") continue;
    const heard = seen.records.get(machine.name) ?? null;
    const record = heard?.kind === "answered" ? heard.logins : null;
    const owner = machine.hold === "assist" || machine.hold === "shared" ? machine.owner : null;
    const ownerLogin = machine.ownerLogin;
    if (ownerLogin && hubs.find((hub) => hub.hub === ownerLogin.hub)?.accounts.some((account) => account.level === "super" && account.name === ownerLogin.account)) admin = true;
    machines.push({
      id: machine.name,
      kind: machine.form,
      here: machine.here,
      online: machine.online !== false && heard?.kind !== "offline",
      owner: machine.hold === "assist" ? owner : null,
      scanned: !!record?.scan || !!record && [record.claude, record.codex].some((side) => side.listed.length + side.candidates.length > 0),
      ...(heard?.kind === "no_answer" ? { silent: heard.reason } : {}),
      ...(record?.scan?.usual.state === "running" ? { scanning: true } : {}),
      ...(record?.scan?.usual.state === "failed" ? { scanFailed: record.scan.usual.reason } : {}),
      ...(record && machine.hold !== "assist" ? { folders: foldersOf(record), missing: missingOf(record) } : {}),
    });
    if (!record) continue;
    for (const [agent, side] of sidesOf(record)) {
      for (const [one, listed] of [...side.listed.map((one) => [one, true] as const), ...side.candidates.map((one) => [one, false] as const)]) {
        const login = loginOf(world, machine, agent, side, one, listed, now);
        const plate = one.week ? one.week / 1_000_000 : seen.plates.get(login.id);
        if (plate) plates[login.id] = plate;
        logins.push(login);
        if (machine.hold === "assist") continue;
        const given = assistsOn(world, machine.name, agent, one);
        for (const { assist } of given) {
          grants.push({ login: login.id, person: assist.account, limit: limitOf(assist, plate ?? PLATE), used: 0, fable: 0, until: assist.until ? momentSaid(new Date(assist.until)) : null });
        }
        const first = given[0]?.assist;
        if (first && (first.five_hour || first.turns)) lines[login.id] = { fiveStop: first.five_hour ?? null, turns: first.turns ?? null };
      }
    }
    // What a folder's search found and no one has taken up yet.
    for (const folder of record.folders ?? []) {
      for (const found of folder.search?.found ?? []) {
        const dir = joined(folder.dir, found.path);
        if (logins.some((one) => one.machine === machine.name && one.dir === dir)) continue;
        logins.push({ id: `found:${idOf(machine.name, found.agent, dir)}`, family: found.agent, machine: machine.name, dir, who: dirName(dir), plan: null, standing: "found", workbench: false, under: folder.dir });
      }
    }
  }
  const people = [...new Set(hubs.flatMap((hub) => hub.accounts.map((account) => account.name)).filter((name) => !mine.some((login) => login.account === name)))].sort();
  return { now, machines, logins, grants, lines, people, plates, expects: Object.fromEntries(Object.entries(plates).map(([id, base]) => [id, { base, swing: SWING }])), admin };
}

function joined(folder: string, path: string): string {
  const slash = folder.includes("\\") && !folder.includes("/") ? "\\" : "/";
  return folder.endsWith(slash) ? folder + path : folder + slash + path;
}

function sidesOf(record: MachineLogins): [string, AgentLogins][] {
  return [["claude", record.claude], ["codex", record.codex], ...Object.entries(record.others ?? {})];
}

function foldersOf(record: MachineLogins): NonNullable<Machine["folders"]> {
  return (record.folders ?? []).map((folder) => {
    const search = folder.search;
    const stage = search?.stage;
    const said =
      !search || !stage
        ? "尚未搜索"
        : stage.state === "running"
          ? `正在搜索　已扫描 ${search.directories.toLocaleString("en-US")} 个文件夹`
          : stage.state === "failed"
            ? `搜索失败：${stage.reason}`
            : stage.state === "stopped"
              ? `已停止　已扫描 ${search.directories.toLocaleString("en-US")} 个文件夹`
              : `找到 ${search.found.length} 个登录`;
    return { dir: folder.dir, said, busy: stage?.state === "running" };
  });
}

function missingOf(record: MachineLogins): string[] {
  if (!record.scan || record.scan.usual.state === "running") return [];
  const names: [string, AgentLogins, string][] = [["claude", record.claude, "Claude Code"], ["codex", record.codex, "Codex"]];
  return names.filter(([, side]) => !side.programs.length && !side.listed.length && !side.candidates.length).map(([, , name]) => name);
}

function assistsOn(world: WorldNow | null, machine: string, agent: string, login: WorldLogin): { hub: string; assist: AssistSeen }[] {
  const home = login.default ? null : login.dir;
  return (world?.accounts.hubs ?? []).flatMap((hub) => hub.assists.filter((assist) => assist.machine === machine && assist.agent === agent && sameHome(assist.home, home)).map((assist) => ({ hub: hub.hub, assist })));
}

/** The part of the week an assistance allows, of a hundred: its share where
 *  the hub keeps one, else its weekly dollars over the login's week. */
export function limitOf(assist: AssistSeen, plate: number): number | null {
  if (assist.share) return assist.share;
  if (!assist.weekly) return null;
  return Math.max(1, Math.round(((assist.weekly / 1_000_000) / plate) * 100));
}

function loginOf(world: WorldNow | null, machine: Pickable, agent: string, side: AgentLogins, one: WorldLogin, listed: boolean, now: Date): Login {
  const identity = one.identity?.who.kind === "known" ? one.identity.who.value : null;
  const token = identity?.method === "oauth_token";
  const plan = planOf(world, machine.name, agent, one);
  const id = idOf(machine.name, agent, one.dir);
  const five = reading(windowOf(one.limits, 300, ["five_hour", "primary"]), now);
  const week = reading(windowOf(one.limits, 10080, ["seven_day", "secondary"]), now);
  const program = one.programs.map((index) => side.programs[index]).find(Boolean);
  const old = agent === "claude" ? claudeTooOld("claude", side.programs, one) : null;
  const unread = one.limits.kind === "known" ? undefined : one.limits.reason;
  const lent = machine.hold === "assist" ? lentOf(world, machine, agent, one) : undefined;
  const credits = one.limits.kind === "known" ? one.limits.value.credits : undefined;
  return {
    id,
    family: agent,
    machine: machine.name,
    dir: one.dir,
    who: identity?.email ?? dirName(one.dir),
    plan: token ? (plan ? planNames[plan] : null) : identity?.plan ? planShown(identity.plan) : null,
    standing: !listed ? "found" : one.integration === "old" || one.drift ? "renew" : "in",
    workbench: listed && !!one.workbench,
    ...(one.default ? { isDefault: true } : {}),
    ...(token ? { token: true, planSet: plan } : {}),
    ...(five ? { five } : {}),
    ...(week ? { week } : {}),
    ...(unread && listed ? { unread } : {}),
    ...(credits ? { credits: credits.unlimited ? "不限" : (credits.balance ?? "") } : {}),
    ...(lent ? { lent } : {}),
    ...(identity && !identity.logged_in ? { loggedOut: true } : {}),
    ...(old ? { tooOld: old } : {}),
    ...(one.drift ? { drift: one.drift.kind === "changed" ? "接入设置已被修改" : "Codex 输出上限需调高" } : {}),
    ...(one.change && !one.change.end ? { busy: one.change.into ? "正在接入…" : "正在移除…" } : {}),
    ...(program ? { program: programSaid(program) } : {}),
    ...(one.auto_title !== undefined ? { autoTitle: one.auto_title } : {}),
    ...(one.models ? { models: one.models.map((model) => model.display_name || model.model || model.id) } : {}),
    ...(one.model_edits && ((one.model_edits.added?.length ?? 0) + (one.model_edits.removed?.length ?? 0) > 0) ? { modelsChanged: true } : {}),
    ...(one.limits.kind === "known" ? { readAt: agoSaid(new Date(one.limits.value.observed_at), now) } : {}),
  };
}

function planOf(world: WorldNow | null, machine: string, agent: string, one: WorldLogin): PlanLabel | null {
  const home = one.default ? null : one.dir;
  for (const hub of world?.accounts.hubs ?? []) {
    const seen = hub.plans?.find((plan) => plan.machine === machine && plan.agent === agent && sameHome(plan.home, home)) as ({ plan: PlanLabel; ultra?: boolean } | undefined);
    if (seen) return seen.plan === "max" && seen.ultra ? "ultra" : seen.plan;
  }
  return null;
}

function lentOf(world: WorldNow | null, machine: Pickable, agent: string, one: WorldLogin): Login["lent"] {
  const home = one.default ? null : one.dir;
  const mine = loginsOf(world?.accounts);
  for (const hub of world?.accounts.hubs ?? []) {
    const assist = hub.assists.find((given) => given.machine === machine.name && given.agent === agent && sameHome(given.home, home) && mine.some((login) => login.hub === hub.hub && login.account === given.account));
    if (assist) return { by: machine.owner ?? "", limit: limitOf(assist, PLATE), used: 0, fable: 0, until: assist.until ? momentSaid(new Date(assist.until)) : null };
  }
  return { by: machine.owner ?? "", limit: null, used: 0, fable: 0, until: null };
}

/** Whose a row of a report or the ledger is, as the pages name them: the
 *  login's own, or the account it worked for. */
function personOf(account: HubAccount | null, world: WorldNow | null): string | null {
  if (!account) return null;
  return loginsOf(world?.accounts).some((login) => login.hub === account.hub && login.account === account.account) ? null : account.account;
}

/** A login's usage report, as its usage page draws it. */
export function daysFrom(report: UsageReport, login: Login, plate: number, world: WorldNow | null, now: Date): Days {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const count = 371;
  const first = new Date(today.getTime() - (count - 1) * DAY);
  const dates = Array.from({ length: count }, (_, index) => new Date(first.getFullYear(), first.getMonth(), first.getDate() + index));
  const dayOf = (at: Date) => Math.round((new Date(at.getFullYear(), at.getMonth(), at.getDate()).getTime() - first.getTime()) / DAY);
  const people = report.people.map((person) => personOf(person.account, world));
  const names = [...new Set(people.map((name) => name ?? "我"))];
  const zero = () => new Array<number>(count).fill(0);
  const total: Record<Unit, number[]> = { quota: zero(), tokens: zero() };
  const byPerson: Record<Unit, number[][]> = { quota: names.map(zero), tokens: names.map(zero) };
  const byModel = [zero(), zero()];
  const fableSaid = login.family === "claude" && report.hours.some((hour) => hour.fable !== undefined);
  const hours = Array.from({ length: 7 }, () => new Array<number>(24).fill(0));
  const since = now.getTime() - 90 * DAY;
  for (const hour of report.hours) {
    const at = new Date(hour.at);
    const index = dayOf(at);
    if (index < 0 || index >= count) continue;
    // Quota dollars: API dollars, Fable's counted at its multiplier.
    const fable = (hour.fable ?? 0) / 1_000_000;
    const dollars = (hour.usd ?? 0) / 1_000_000 + fable * (FABLE_TIMES - 1);
    total.quota[index]! += dollars;
    byModel[0]![index]! += fable * FABLE_TIMES;
    byModel[1]![index]! += dollars - fable * FABLE_TIMES;
    total.tokens[index]! += hour.tokens;
    const who = names.indexOf(people[hour.person] ?? "我");
    if (who >= 0) {
      byPerson.quota[who]![index]! += dollars;
      byPerson.tokens[who]![index]! += hour.tokens;
    }
    if (at.getTime() >= since) hours[(at.getDay() + 6) % 7]![at.getHours()]! += login.family === "codex" ? hour.tokens : dollars;
  }
  // Days from older nodes carry API dollars alone; the week's Fable is
  // known apart, so the week's figures carry its multiplier either way.
  const parts: Record<Unit, Partial<Record<Split, Series>>> = { quota: {}, tokens: {} };
  if (fableSaid) parts.quota.model = { names: ["Fable", "其他型号"], values: byModel };
  if (names.length > 1) {
    parts.quota.person = { names, values: byPerson.quota };
    parts.tokens.person = { names, values: byPerson.tokens };
  }
  const week = report.people.map((person, index) => {
    const fable = (person.fable?.usd ?? 0) / 1_000_000;
    return { name: people[index] ?? null, week: (person.week.usd ?? 0) / 1_000_000 + fable * (FABLE_TIMES - 1), fable: fable * FABLE_TIMES };
  });
  const merged = new Map<string, { name: string | null; week: number; fable: number }>();
  for (const one of week) {
    const key = one.name ?? "";
    const had = merged.get(key) ?? { name: one.name, week: 0, fable: 0 };
    merged.set(key, { name: one.name, week: had.week + one.week, fable: had.fable + one.fable });
  }
  const ledgered = [...merged.values()].reduce((sum, one) => sum + one.week, 0);
  const weeks = (login.family === "codex" ? [] : (report.weeks ?? []))
    .filter((span) => (span.used_tenths ?? 0) >= 50)
    .map((span) => ({ start: new Date(span.start), value: span.used.reduce((sum, one) => sum + (one.usd ?? 0), 0) / 1_000_000 / ((span.used_tenths ?? 0) / 1000) }))
    .slice(-8);
  const how: Record<string, string> = { automatic: "自动检测", scheduled: "周期重置", recorded: "已记录" };
  return {
    dates,
    total,
    parts,
    hours,
    hoursAre: login.family === "codex" ? "tokens" : "quota",
    people: [...merged.values()],
    weeks,
    resets: report.resets ? report.resets.map((reset) => ({ at: momentSaid(new Date(reset.confirmed_at)), how: how[reset.source] ?? reset.source, from: Math.round((reset.before_used_tenths ?? 0) / 10), to: Math.round(reset.used_tenths / 10) })) : null,
    gap: login.week && !login.week.reset && (!login.lent || login.lent.limit === null) ? login.week.used - (ledgered / plate) * 100 : 0,
  };
}

const outcomes: Record<CallOutcome, Outcome> = { completed: "done", failed: "failed", interrupted: "stopped", held: "held", unknown: "done" };

/** One of the ledger's rows, as the requests' page draws it. */
function callOf(row: UsageCall, codex: boolean, world: WorldNow | null, nameOf: (conversation: string) => string | null): Call {
  const model = row.model ?? "未知型号";
  const api = (row.usd ?? 0) / 1_000_000;
  const times = timesOf(model);
  const started = row.timing.started_at ? new Date(row.timing.started_at).getTime() : null;
  const ended = new Date(row.timing.ended_at).getTime();
  const firstAt = row.timing.first_at ? new Date(row.timing.first_at).getTime() : null;
  const tokens = row.tokens ?? {};
  return {
    at: new Date(row.timing.ended_at),
    who: personOf(row.who, world),
    model,
    effort: row.effort ?? "",
    source: row.source,
    agent: row.agent,
    times,
    fresh: tokens.input ?? 0,
    cacheRead: tokens.cache_read ?? 0,
    cacheWrite: codex ? (tokens.reasoning ?? 0) : (tokens.cache_write ?? 0),
    output: tokens.output ?? 0,
    api,
    quota: api * times,
    first: started !== null && firstAt !== null ? (firstAt - started) / 1000 : 0,
    took: started !== null ? (ended - started) / 1000 : 0,
    outcome: outcomes[row.outcome] ?? "done",
    talk: row.conversation ? (nameOf(row.conversation) ?? row.conversation) : null,
    opens: row.conversation && row.turn ? { conversation: row.conversation, turn: row.turn } : null,
  };
}

/** What the ledger is asked for a page: of the page's own size, or, of a
 *  node that knows no sizes (`legacy`), the page of 100 it falls in and
 *  where in it the page starts. */
export function askedOf(query: CallsQuery, person: UsageCallsQuery["person"], legacy: boolean): { ask: UsageCallsQuery; skip: number } {
  const offset = (query.page - 1) * query.size;
  return {
    ask: {
      ...(legacy ? { page: Math.floor(offset / 100) } : { page: query.page - 1, size: query.size }),
      ...(query.from ? { from: query.from.toISOString() } : {}),
      ...(query.until ? { until: query.until.toISOString() } : {}),
      ...(query.model ? { model: query.model } : {}),
      ...(query.source ? { source: query.source } : {}),
      ...(query.outcome ? { outcome: ({ done: "completed", failed: "failed", stopped: "interrupted", held: "held" } as const)[query.outcome] } : {}),
      ...(person !== undefined ? { person } : {}),
      ...(query.turn ? { conversation: query.turn.conversation, turn: query.turn.turn } : {}),
    },
    skip: legacy ? offset % 100 : 0,
  };
}

export function callsFrom(page: UsageCallsPage, query: CallsQuery, skip: number, login: Login, world: WorldNow | null, seen: { models: Set<string>; people: Set<string> }, nameOf: (conversation: string) => string | null): CallsPage {
  const rows = page.rows.slice(skip, skip + query.size).map((row) => callOf(row, login.family === "codex", world, nameOf));
  for (const row of page.rows) {
    if (row.model) seen.models.add(row.model);
    const who = personOf(row.who, world);
    if (who) seen.people.add(who);
  }
  const sum = page.sum ? { input: page.sum.input, output: page.sum.output, quota: (page.sum.usd + (page.sum.fable_usd ?? 0) * (FABLE_TIMES - 1)) / 1_000_000 } : null;
  return { rows, total: page.total ?? null, sum, models: [...seen.models].sort((a, b) => Number(isFable(b)) - Number(isFable(a)) || a.localeCompare(b)), people: [...seen.people].sort() };
}
