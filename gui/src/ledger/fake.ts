// A made-up network for the agents' pages to be looked at and pressed
// through in the specimen: machines, the agents found on each, their logins,
// the people some logins assist, and each login's own ledger of requests.
// Everything comes from one seed, so it is the same each time. It fills the
// pages' data the way the World does in the app (`data.ts`, `live.ts`).

import { seeded } from "../pen/shapes";
import { grantsOf, plateOf, timesOf, type Call, type CallsPage, type CallsQuery, type Days, type Feed, type Grant, type Line, type Login, type Machine, type Net, type Outcome, type Series, type Source, type Split, type Unit } from "./data";

export const NOW = new Date("2026-11-15T10:04:38+08:00");

const machines: Machine[] = [
  { id: "desktop", kind: "desktop", here: true, online: true, owner: null, scanned: true, folders: [{ dir: "D:\\CodexHomes", said: "找到 1 个登录", busy: false }, { dir: "D:\\", said: "已停止　已扫描 12,345 个文件夹", busy: false }], missing: [] },
  { id: "laptop", kind: "laptop", here: false, online: true, owner: null, scanned: true },
  { id: "lab", kind: "desktop", here: false, online: true, owner: null, scanned: true },
  { id: "server", kind: "server", here: false, online: true, owner: null, scanned: true },
  { id: "studio", kind: "desktop", here: false, online: false, owner: null, scanned: false },
  { id: "ada-mac", kind: "laptop", here: false, online: true, owner: "ada", scanned: true },
  { id: "bo-pc", kind: "desktop", here: false, online: true, owner: "bo", scanned: true },
];

const logins: Login[] = [
  { id: "max", family: "claude", machine: "desktop", dir: "C:\\Users\\me\\.claude", who: "me@example.com", plan: "Max", standing: "in", workbench: true, isDefault: true, five: { used: 38, resets: "2 小时 14 分后" }, week: { used: 72, resets: "周四 05:00" }, fable: 44, program: "claude 2.1.322　npm　在 PATH 中", models: ["Fable 5.2", "Opus 5.5", "Sonnet 5.5", "Haiku 5.5"], readAt: "4 分钟前" },
  { id: "review", family: "claude", machine: "desktop", dir: "C:\\Users\\me\\.claude-review", who: ".claude-review", plan: null, standing: "found", workbench: false },
  { id: "cx", family: "codex", machine: "desktop", dir: "C:\\Users\\me\\.codex", who: "me@example.com", plan: "Pro", standing: "in", workbench: true, isDefault: true, five: { used: 27, resets: "3 小时 40 分后" }, week: { used: 55, resets: "周五 09:00" }, autoTitle: true, program: "codex 0.98.0　官方安装　在 PATH 中", readAt: "1 分钟前" },
  { id: "cx-work", family: "codex", machine: "desktop", dir: "C:\\Users\\me\\.codex-work", who: "work@example.com", plan: "Plus", standing: "in", workbench: false },
  { id: "cx-team", family: "codex", machine: "desktop", dir: "D:\\CodexHomes\\team", who: "team", plan: null, standing: "found", workbench: false, under: "D:\\CodexHomes" },
  { id: "gemini", family: "antigravity", machine: "desktop", dir: "C:\\Users\\me\\.gemini", who: ".gemini", plan: null, standing: "in", workbench: false },
  { id: "cursor", family: "cursor", machine: "desktop", dir: "C:\\Users\\me\\.cursor", who: ".cursor", plan: null, standing: "found", workbench: false },
  { id: "opencode", family: "opencode", machine: "desktop", dir: "C:\\Users\\me\\.config\\opencode", who: "opencode", plan: null, standing: "renew", workbench: false },
  { id: "lap", family: "claude", machine: "laptop", dir: "/Users/me/.claude", who: "me@example.com", plan: "Max", standing: "in", workbench: false, isDefault: true },
  { id: "lap-cx", family: "codex", machine: "laptop", dir: "/Users/me/.codex", who: "me@example.com", plan: "Pro", standing: "found", workbench: false, isDefault: true },
  { id: "verges", family: "claude", machine: "lab", dir: "/home/me/.claude-verges", who: ".claude-verges", plan: "Ultra", planSet: "ultra", standing: "in", workbench: true, token: true, five: { used: 61, resets: "48 分后" }, week: { used: 92, resets: "周三 21:00" }, fable: 50, fableHeld: true, program: "claude 2.1.322　npm　在 PATH 中", models: ["Fable 5.2", "Opus 5.5", "Sonnet 5.5"], readAt: "12 分钟前" },
  { id: "cx-lab", family: "codex", machine: "lab", dir: "/home/me/.codex", who: "lab@example.com", plan: "Pro", standing: "in", workbench: true, isDefault: true, five: { used: 100, resets: "1 小时 05 分后" }, week: { used: 64, resets: "周六 14:00" }, credits: "$12.40", autoTitle: true },
  { id: "dy", family: "claude", machine: "server", dir: "/root/.claude-dy", who: ".claude-dy", plan: null, planSet: null, standing: "in", workbench: true, token: true, five: { used: 12, resets: "4 小时 02 分后" }, week: { used: 42, resets: "周五 21:00" }, fable: 18 },
  { id: "ada-work", family: "claude", machine: "ada-mac", dir: "/Users/ada/.claude", who: "work@example.com", plan: "Max", standing: "in", workbench: true, lent: { by: "ada", limit: 25, used: 15.5, fable: 5, until: "11 月 30 日" } },
  { id: "bo-team", family: "claude", machine: "bo-pc", dir: "C:\\Users\\bo\\.claude", who: "team@example.com", plan: "Ultra", standing: "in", workbench: true, five: { used: 44, resets: "1 小时 32 分后" }, week: { used: 63, resets: "周一 08:00" }, fable: 21, lent: { by: "bo", limit: null, used: 9, fable: 4, until: null } },
];

const grants: Grant[] = [
  { login: "max", person: "ada", limit: 25, used: 22, fable: 12, until: "11 月 30 日" },
  { login: "max", person: "bo", limit: 20, used: 11, fable: 6, until: null },
  { login: "max", person: "cy", limit: 10, used: 6, fable: 4, until: "11 月 21 日" },
  { login: "verges", person: "ada", limit: 50, used: 44, fable: 25, until: null },
  { login: "verges", person: "dee", limit: 25, used: 19, fable: 11, until: "12 月 05 日" },
  { login: "verges", person: "bo", limit: 15, used: 6, fable: 2, until: null },
];

const lines: Record<string, Line> = {
  max: { fiveStop: 80, turns: 3 },
  verges: { fiveStop: 70, turns: 2 },
};

/** What the specimen's logins' weeks are set to, and what their whole weeks
 *  came to by the readings, the newest last. */
const plates: Record<string, number> = { max: 3000, verges: 3000, dy: 2400, cx: 1200, "cx-lab": 1200, "ada-work": 1500, "bo-team": 3000 };
const weeksRead: Record<string, number[]> = {
  max: [3080, 2990, 3120, 3040, 2410, 3070, 3010, 3060],
  verges: [2960, 3050, 2980, 3020, 3090, 2940, 3010, 3030],
  dy: [3150, 3080, 3190, 2260, 3110, 3140, 3090, 3120],
  cx: [1180, 1230, 1150, 1210, 1190, 1240, 1170, 1200],
  "cx-lab": [1160, 1190, 1220, 1180, 1140, 1210, 1170, 1180],
};

export function fakeNet(): Net {
  return { now: NOW, machines, logins, grants, lines, people: ["ada", "bo", "cy", "dee", "eli", "fan", "gus"], plates, admin: true };
}

interface Model {
  name: string;
  effort: string;
  /** Its part of a login's requests, of a hundred. */
  share: number;
  /** Its API price beside Opus 5.5's. */
  price: number;
}

const modelsOf = (family: string): Model[] =>
  family === "codex"
    ? [
        { name: "GPT-6.1 Sol", effort: "高", share: 62, price: 1.6 },
        { name: "GPT-6.1", effort: "中", share: 28, price: 1 },
        { name: "GPT-6.1 mini", effort: "", share: 10, price: 0.2 },
      ]
    : [
        { name: "Fable 5.2", effort: "很高", share: 46, price: 2 },
        { name: "Opus 5.5", effort: "高", share: 28, price: 1 },
        { name: "Sonnet 5.5", effort: "中", share: 9, price: 0.5 },
        { name: "Haiku 5.5", effort: "", share: 17, price: 0.025 },
      ];

const talks = ["PID 环改定点", "批量提交", "真机验证", "日志轮转", "CAN 帧时间戳", "夜里的回归与训练", "图标导出", "ingest 迁移"];
const agents = ["Explore", "Plan", "general-purpose"];
const TOTALS: Record<string, number> = { max: 33007, verges: 41260, dy: 12480, cx: 9120, "cx-lab": 15830, "ada-work": 2140, "bo-team": 3310 };

/** A run of a login's requests, the newest first. */
function made(login: Login, page: number, size: number): Call[] {
  const random = seeded(`calls-${login.id}-${page}`);
  const pick = <T>(of: T[]): T => of[Math.floor(random() * of.length)]!;
  const models = modelsOf(login.family);
  const others = grantsOf(login).map((grant) => grant.person);
  const rows: Call[] = [];
  let at = NOW.getTime() - page * size * 95000;
  for (let index = 0; index < size; index += 1) {
    at -= Math.round(4000 + random() * random() * 380000);
    let roll = random() * 100;
    const model = models.find((one) => (roll -= one.share) < 0) ?? models[0]!;
    const small = model.price < 0.3;
    const who = login.lent ? null : others.length && random() < 0.42 ? pick(others) : null;
    const source: Source = small ? "agent" : random() < 0.06 ? "background" : random() < 0.12 ? "agent" : "main";
    const cacheRead = small ? Math.round(random() * 40000) : Math.round(90000 + random() * 190000);
    const fresh = Math.round(4 + random() * random() * (small ? 9000 : 6000));
    const cacheWrite = random() < 0.35 ? Math.round(1200 + random() * 14000) : 0;
    const output = Math.round(120 + random() * random() * (small ? 900 : 5200));
    // Opus 5.5's prices a million tokens: 4 in, 5 written to the cache, 20
    // out, 0.2 read from it; a dearer model's reading costs no more.
    const api = (model.price * (fresh * 4 + cacheWrite * 5 + output * 20) + Math.min(model.price, 1) * 0.2 * cacheRead) / 1e6;
    const times = timesOf(model.name);
    const fate = random();
    const outcome: Outcome = fate < 0.03 ? "failed" : fate < 0.05 ? "stopped" : fate < 0.06 ? "held" : "done";
    const none = outcome === "held";
    const first = none ? 0 : 0.3 + random() * random() * 4;
    rows.push({
      at: new Date(at),
      who,
      model: model.name,
      effort: model.effort,
      source,
      agent: source === "agent" ? pick(agents) : null,
      times,
      fresh: none ? 0 : fresh,
      cacheRead: none ? 0 : cacheRead,
      cacheWrite: none ? 0 : cacheWrite,
      output: none ? 0 : output,
      quota: none ? 0 : api * times,
      api: none ? 0 : api,
      first,
      took: none ? 0 : first + output / (38 + random() * 60),
      outcome,
      talk: who === null && source !== "background" ? pick(talks) : null,
      opens: who === null && source !== "background" ? { conversation: "claude-2b1c", turn: "t1" } : null,
    });
  }
  return rows;
}

const presetPart = (query: CallsQuery) => (query.from === null ? 1 : Math.min(1, (NOW.getTime() - query.from.getTime()) / (30 * 86400000)));

function calls(login: Login, query: CallsQuery): CallsPage {
  const matches = (row: Call) =>
    (query.who === null || (query.who === "我" ? row.who === null : row.who === query.who)) &&
    (!query.model || row.model === query.model) &&
    (!query.source || row.source === query.source) &&
    (!query.outcome || row.outcome === query.outcome);
  const rows: Call[] = [];
  let seen = 0;
  const skip = Math.floor(((query.page - 1) * query.size) / 60);
  for (let tries = 0; rows.length < query.size && tries < 40; tries += 1) {
    for (const row of made(login, skip + tries, 60)) {
      seen += 1;
      if (matches(row) && rows.length < query.size) rows.push(row);
    }
  }
  const total = Math.max(rows.length, Math.round((TOTALS[login.id] ?? 0) * presetPart(query) * Math.min(1, (rows.length / Math.max(seen, 1)) * 1.15)));
  const scale = total / Math.max(rows.length, 1);
  const sum = (of: (row: Call) => number) => rows.reduce((all, row) => all + of(row), 0) * scale;
  return {
    rows,
    total,
    sum: { input: sum((row) => row.fresh + row.cacheRead + row.cacheWrite), output: sum((row) => row.output), quota: sum((row) => row.quota) },
    models: modelsOf(login.family).map((model) => model.name),
    people: grantsOf(login).map((grant) => grant.person),
  };
}

const DAY = 86400000;

/** A year of a login's days, the newest last. */
function days(login: Login): Days {
  const random = seeded(`days-${login.id}`);
  const weekly = ((login.week?.used ?? 30) / 100) * plateOf(login) + 300;
  const dates: Date[] = [];
  const total: number[] = [];
  for (let back = 370; back >= 0; back -= 1) {
    const at = new Date(NOW.getTime() - back * DAY);
    const weekend = at.getDay() === 0 || at.getDay() === 6;
    const young = Math.min(1, (371 - back) / 150);
    const idle = random() < (weekend ? 0.35 : 0.08) || back > 330;
    dates.push(at);
    total.push(idle ? 0 : (weekly / 7) * young * (weekend ? 0.45 : 1) * (0.35 + random() * 1.5));
  }
  const cut = (names: string[], weights: number[]): Series => {
    const values = names.map(() => [] as number[]);
    for (const amount of total) {
      const drawn = weights.map((weight) => weight * (0.5 + random()));
      const sum = drawn.reduce((a, b) => a + b, 0);
      drawn.forEach((one, index) => values[index]!.push((amount * one) / sum));
    }
    return { names, values };
  };
  const models = modelsOf(login.family);
  const others = grantsOf(login);
  const hours = Array.from({ length: 7 }, (_, weekday) =>
    Array.from({ length: 24 }, (_, hour) => {
      const awake = hour >= 9 && hour <= 23 ? 1 : hour <= 1 ? 0.5 : 0.08;
      const peak = hour >= 14 && hour <= 17 ? 1.5 : hour >= 20 && hour <= 22 ? 1.3 : 1;
      return Math.round(awake * peak * (weekday > 4 ? 0.4 : 1) * random() * 420);
    }),
  );
  const plate = plateOf(login);
  const used = login.week?.used ?? 0;
  const elsewhere = others.length ? 2 : 0;
  const mine = Math.max(used - others.reduce((sum, grant) => sum + grant.used, 0) - elsewhere, 0);
  const mineFable = Math.max((login.fable ?? 0) - others.reduce((sum, grant) => sum + grant.fable, 0), 0);
  const quota: Partial<Record<Split, Series>> = {
    model: cut(models.map((one) => one.name), models.map((one) => one.share * Math.max(one.price * timesOf(one.name), 0.3))),
    ...(others.length ? { person: cut(["我", ...others.map((grant) => grant.person)], [Math.max(mine, 20), ...others.map((grant) => grant.used)]) } : {}),
    kind: cut(login.family === "codex" ? ["缓存输入", "新输入", "输出", "推理"] : ["缓存读", "缓存写", "新输入", "输出"], login.family === "codex" ? [30, 8, 34, 28] : [34, 12, 6, 48]),
    source: cut(login.family === "codex" ? ["主循环", "子代理"] : ["主循环", "子代理", "后台"], login.family === "codex" ? [82, 18] : [78, 18, 4]),
  };
  const tokensOf = (series: Series): Series => ({ names: series.names, values: series.values.map((of) => of.map((value) => value * 290000)) });
  const parts: Record<Unit, Partial<Record<Split, Series>>> = { quota, tokens: Object.fromEntries(Object.entries(quota).map(([split, series]) => [split, tokensOf(series!)])) };
  return {
    dates,
    total: { quota: total, tokens: total.map((value) => value * 290000) },
    parts,
    hours,
    hoursAre: "requests",
    people: [
      { name: null, week: (mine / 100) * plate, fable: (mineFable / 100) * plate },
      ...others.map((grant) => ({ name: grant.person, week: (grant.used / 100) * plate, fable: (grant.fable / 100) * plate })),
    ],
    weeks: (weeksRead[login.id] ?? []).map((value, index, all) => ({ start: new Date(NOW.getTime() - (all.length - index + 1) * 7 * DAY), value })),
    resets: [
      { at: "11 月 12 日 05:00", how: "周期重置", from: 96, to: 0 },
      { at: "11 月 5 日 05:00", how: "周期重置", from: 81, to: 0 },
      { at: "10 月 31 日 16:20", how: "自动检测", from: 64, to: 1 },
      { at: "10 月 29 日 05:00", how: "周期重置", from: 100, to: 0 },
    ],
    elsewhere,
    // A made-up size for its five-hour window, only to give the bars their dollars.
    fiveSpent: login.five ? (login.five.used / 100) * (login.family === "codex" ? 300 : 780) : null,
  };
}

export const fakeFeed: Feed = {
  days: (login) => Promise.resolve(days(login)),
  calls: (login, query) => Promise.resolve(calls(login, query)),
};
