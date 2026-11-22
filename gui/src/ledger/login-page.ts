// One login's page: which agent it is a login of, on which machine, and
// under that its four views. 「用量」: the account's own windows as its
// vendor reads them, in a place of their own; under them what the ledger
// reckons of its week; then what it used drawn by day, by week, over a
// year and by hour. 「调用」: its ledger, a request to a row. 「协助」: the
// people it works for and what share of its week each may use. 「设置」:
// what is set on the login itself. A login that assists me shows me all of
// that where I was given no limit, and lets me set the others' shares;
// where I was given one I see my own part alone. Everyone sees what the
// week is expected to hold and the share they were given of it.

import { glyph } from "../conversation/glyphs";
import { bars, calendar, INKS, punch, readout, rise, type Column } from "./charts";
import {
  admin,
  byId,
  callsOf,
  daysOf,
  deeds,
  expectOf,
  FABLE_TIMES,
  familyName,
  gapTold,
  grantsOf,
  lines,
  ME,
  NOW,
  pinned,
  plateOf,
  planNames,
  splitNames,
  usable,
  whenArrived,
  type Call,
  type CallsPage,
  type CallsQuery,
  type Days,
  type Login,
  type Outcome,
  type PlanLabel,
  type Series,
  type Source,
  type Split,
} from "./data";
import { clock, count, dated, dollars, seconds, tokens, whole } from "./format";
import { allotted, fableSaid, lentSpent, money, official, reckoned } from "./home";
import { doubleRule, rule } from "./marks";
import { draw, linkOf, on, say, state, type Tab } from "./state";
import { chevron, choices, escape, familyMark, figure, gauge, menu, menuUnder, toggle } from "./ui";

const current = (): Login | undefined => byId.get((state.place as { id: string }).id);
const shared = (login: Login) => grantsOf(login).length > 0 && !login.lent;

/** Said where what a page wants has not come, or could not be had. */
const waiting = (what: string | Error) =>
  what instanceof Error ? `<div class="lg-body lg-empty"><p>读取失败</p><p class="lg-soft" title="${escape(what.message)}">${escape(what.message.slice(0, 120))}</p></div>` : `<div class="lg-body lg-empty"><p class="lg-soft">${escape(what)}</p></div>`;

export function loginPage(): string {
  const login = current();
  if (!login) return `<div class="lg-page"><header class="lg-head lg-head-login"><a class="lg-back" href="${linkOf({ kind: "quota" })}" aria-label="返回" title="返回智能体">${glyph("back", 18)}</a></header>${waiting("该登录不在列表中")}</div>`;
  // One who manages it with its owner sets the shares; one it assists, as
  // anyone who uses it, sets compacting their own idle conversations, and
  // nothing else of it.
  const tabs: [Tab, string][] = login.lent
    ? login.lent.manages
      ? [["usage", "用量"], ["calls", "调用"], ["assist", "协助"], ["settings", "设置"]]
      : [["usage", "用量"], ["calls", "调用"], ["settings", "设置"]]
    : [["usage", "用量"], ["calls", "调用"], ["assist", "协助"], ["settings", "设置"]];
  const wanted = (state.place as { tab: Tab }).tab;
  const tab = tabs.some(([id]) => id === wanted) ? wanted : "usage";
  const body = tab === "usage" ? usage(login) : tab === "calls" ? callsView(login) : tab === "assist" ? assist(login) : settings(login);
  const plan = login.plan ? `　${escape(login.plan)}` : "";
  return `<div class="lg-page">
    <header class="lg-head lg-head-login">
      <a class="lg-back" href="${linkOf({ kind: "quota" })}" aria-label="返回" title="返回智能体">${glyph("back", 18)}</a>
      <span class="lg-head-mark">${familyMark(login, 28)}</span>
      <div class="lg-ident"><h1 class="lg-title">${escape(login.who)}</h1><p>${escape(familyName(login.family))}　${escape(login.machine)}${plan}${login.token ? "　长期令牌" : ""}${login.lent ? `　来自 ${escape(login.lent.by)}` : ""}</p></div>
      <div class="segmented">${tabs.map(([id, name]) => `<button type="button" class="segment${id === tab ? " chosen" : ""}" data-go="${linkOf({ kind: "login", id: login.id, tab: id })}">${name}</button>`).join("")}</div>
      <span class="lg-head-end"><button type="button" class="lg-go" data-act="new-talk" data-id="${escape(login.id)}">${glyph("plus", 14)}新对话</button></span>
    </header>
    ${body}
  </div>`;
}

// ---- 用量

/** The unit drawn: as the page is set, but tokens for Codex, which tells
 *  no prices. */
const unitNow = (): "quota" | "tokens" => (current()?.family === "codex" ? "tokens" : state.unit);
/** An amount in the unit drawn. */
const amount = (value: number) => (unitNow() === "tokens" ? tokens(value) : dollars(value));

/** The ways this login's usage can be split, as its ledger tells them. */
const splitsOf = (login: Login, days: Days) => (Object.keys(days.parts[unitNow()]) as Split[]).filter((way) => way !== "person" || shared(login));

/** What is drawn: the whole, or its parts by what it is split by. */
function series(login: Login, days: Days): Series {
  const parts = state.split === "none" || !splitsOf(login, days).includes(state.split) ? null : days.parts[unitNow()][state.split];
  return parts ?? { names: ["用量"], values: [days.total[unitNow()]] };
}

/** What the week is expected to hold and how far its figure may stand from
 *  it, as everyone a login works for is told. */
const expectSaid = (login: Login) => `预期周额度 ${whole(expectOf(login).base)}　±${expectOf(login).swing}%`;

/** A group of the page's large figures under its own name. */
const group = (name: string, said: string, cells: string[], more = "") => {
  const drawn = cells.filter(Boolean);
  return drawn.length ? `<section class="lg-group${more}"><header class="lg-group-head"><h2>${name}</h2><small>${said}</small></header><div class="lg-now">${drawn.join("")}</div></section>` : "";
};
const big = (name: string, cell: string, said = "") => (cell ? `<div class="lg-big"><h3${said ? ` title="${said}"` : ""}>${name}</h3>${cell}</div>` : "");

/** The top of 「用量」. One who sees the login whole: the account's own
 *  quota as its vendor reads it, in its own place, with whatever the vendor
 *  gives and nothing reckoned; then what the ledger reckons of the week.
 *  One given a limit: their own part, the share it is, and what the week
 *  is expected to hold. */
function nowOf(login: Login): string {
  const lent = login.lent;
  const days = usable(daysOf(login));
  if (lent && !lent.manages) {
    const [week, fable] = allotted(login, 20);
    return group("我的额度", `来自 ${escape(lent.by)}　${lent.until ? `至 ${escape(lent.until)}` : "不限期"}`, [
      big("份额", figure(`${lent.limit ?? 100}%`, expectSaid(login)), "每周开始时按上周读数与账本定出本周周额度，只在预期值的波动幅度内变动；本人额度为其份额"),
      big("本周", week!),
      big(`<span title="${fableSaid}">Fable</span>`, fable ?? ""),
    ]);
  }
  const [five, week] = official(login, 20);
  const read = login.readAt ? `官方读数　${escape(login.readAt)}` : "官方读数";
  const real = group("真实额度", read, [big("5 小时", five), big("7 天", week), login.credits ? big("余额", figure(escape(login.credits), "")) : ""]);
  // Codex tells no prices: nothing of its week is reckoned in dollars.
  if (login.family !== "claude") return real;
  const [spent, fable] = reckoned(login, 20);
  const gap = days && login.week && gapTold(days.gap, login.week.used) ? days.gap : 0;
  const told = gap
    ? `　<span class="lg-caution" title="${gap > 0 ? "可能来自官方 app、网页或其他设备，也可能是本周用法更耗额度" : "可能是厂商重置了额度"}">官方读数${gap > 0 ? "高于" : "低于"}账本 ${Math.abs(Math.round(gap))} 点</span>`
    : "";
  const mine = lent ? `　本人已用 ${whole(lentSpent(login).week)}${lent.limit === null ? "" : ` / ${whole((plateOf(login) * lent.limit) / 100)}`}` : "";
  return (
    real +
    group("推算", `按账本${mine}${told}`, [big("本周周额度", figure(whole(plateOf(login)), expectSaid(login)), "每周开始时按上周读数与账本定出，只在预期值的波动幅度内变动"), big("本周已用", spent), big(`<span title="${fableSaid}">Fable</span>`, fable)], " lg-reckoned")
  );
}

/** Each reading of a login's days, by when it came: the page's chart says
 *  which it draws, so a new reading draws the page again though nothing of
 *  its words changed. */
const readings = new WeakMap<object, number>();
let read = 0;
function readingOf(days: object): number {
  let at = readings.get(days);
  if (at === undefined) {
    at = ++read;
    readings.set(days, at);
  }
  return at;
}

function usage(login: Login): string {
  const fetched = daysOf(login);
  const days = usable(fetched);
  const kinds: [typeof state.chart, string][] = [["days", "按日"], ["weeks", "按周"], ["year", "全年"], ["hours", "时段"]];
  const splits = days && (state.chart === "days" || state.chart === "weeks") ? splitsOf(login, days) : [];
  const split = splits.length > 0 && state.split !== "none" && splits.includes(state.split);
  const stats = days
    ? `<header class="lg-stats-head">
        <div class="ledger-switch">${kinds.map(([id, name]) => `<button type="button" class="${id === state.chart ? "current" : ""}" data-act="chart" data-value="${id}">${name}</button>`).join("")}</div>
        <span class="lg-head-end">
          ${splits.length ? `<button type="button" class="lg-field"${split ? " data-set" : ""} data-act="split">${split ? `按${splitNames[state.split as Split]}` : "分组"}${chevron()}</button>` : ""}
          ${state.chart === "hours" || login.family === "codex" ? "" : `<div class="ledger-switch small"><button type="button" class="${state.unit === "quota" ? "current" : ""}" data-act="unit" data-value="quota" title="额度美元：请求的 API 价，Fable 另乘倍率">额度</button><button type="button" class="${state.unit === "tokens" ? "current" : ""}" data-act="unit" data-value="tokens">token</button></div>`}
        </span>
      </header>
      <div class="lg-stats-body" data-kind="${split ? "split" : state.chart}"><div class="lg-chart" data-chart="${readingOf(days)}"></div>${split ? legend(login, days) : ""}</div>
      ${figures(days)}`
    : `<p class="lg-soft lg-wait">${fetched instanceof Error ? `用量读取失败：${escape(fetched.message.slice(0, 120))}` : "正在读取用量…"}</p>`;
  return `<div class="lg-body lg-sheet">
    ${nowOf(login)}
    ${rule(2)}
    <section class="lg-stats">${stats}</section>
  </div>`;
}

const SHOWN_DAYS = 30;
const SHOWN_WEEKS = 12;
const weekdays = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

/** The days of each of the last weeks, the oldest first. */
function weeksOf(days: Days): number[][] {
  const last = days.dates.length - 1;
  return Array.from({ length: SHOWN_WEEKS }, (_, back) => {
    const end = last - (SHOWN_WEEKS - 1 - back) * 7;
    return Array.from({ length: 7 }, (_, day) => end - 6 + day).filter((index) => index >= 0);
  });
}

function legend(login: Login, days: Days): string {
  const { names, values } = series(login, days);
  const span = state.chart === "days" ? SHOWN_DAYS : SHOWN_WEEKS * 7;
  const sums = values.map((of) => of.slice(-span).reduce((sum, value) => sum + value, 0));
  const all = sums.reduce((sum, value) => sum + value, 0) || 1;
  return `<ul class="lg-legend">${names.map((name, index) => `<li><i style="background:${INKS[Math.min(index, INKS.length - 1)]}"></i><span>${escape(name)}</span><b>${amount(sums[index]!)}</b><small>${Math.round((sums[index]! / all) * 100)}%</small></li>`).join("")}</ul>`;
}

function figures(days: Days): string {
  const { dates, hours } = days;
  const total = days.total[unitNow()];
  const figure = (name: string, value: string, more = "") => `<div><dt>${name}</dt><dd>${value}${more ? `<small>${more}</small>` : ""}</dd></div>`;
  let shown = "";
  if (state.chart === "days") {
    const of = total.slice(-SHOWN_DAYS);
    const most = of.indexOf(Math.max(...of));
    const sum = of.reduce((a, b) => a + b, 0);
    shown = figure("近 30 天", amount(sum)) + figure("日均", amount(sum / SHOWN_DAYS)) + (most >= 0 && of[most]! > 0 ? figure("单日最高", amount(of[most]!), dated(dates[dates.length - of.length + most]!, false)) : "");
  } else if (state.chart === "weeks") {
    const sums = weeksOf(days).map((week) => week.reduce((sum, day) => sum + total[day]!, 0));
    shown = figure("本周", amount(sums[sums.length - 1] ?? 0)) + figure("上周", amount(sums[sums.length - 2] ?? 0)) + figure("周均", amount(sums.reduce((a, b) => a + b, 0) / Math.max(sums.length, 1)));
  } else if (state.chart === "year") {
    let run = 0;
    let longest = 0;
    for (const value of total) {
      run = value > 0 ? run + 1 : 0;
      longest = Math.max(longest, run);
    }
    shown = figure("近一年", amount(total.reduce((a, b) => a + b, 0))) + figure("使用天数", `${total.filter((value) => value > 0).length} 天`) + figure("最长连续", `${longest} 天`);
  } else {
    const all = hours.flat().reduce((a, b) => a + b, 0) || 1;
    const part = (from: number, to: number) => `${Math.round((hours.reduce((sum, day) => sum + day.slice(from, to).reduce((a, b) => a + b, 0), 0) / all) * 100)}%`;
    const most = Math.max(...hours.flat());
    const weekday = hours.findIndex((day) => day.includes(most));
    shown = (most > 0 ? figure("高峰时段", `${weekdays[weekday]} ${hours[weekday]!.indexOf(most)} 点`) : "") + figure("0–6 点占比", part(0, 6)) + figure("9–18 点占比", part(9, 18), "近 90 天");
  }
  return `<dl class="lg-figures">${shown}</dl>`;
}

/** Whether the chart drawn next comes onto the page, as one just changed
 *  does, and not only stands there again. */
let coming = false;

/** Draws the page's chart at the width it has; `arriving`, its marks come
 *  onto the page. */
export function fillCharts(root: HTMLElement, arriving = false): void {
  const holder = root.querySelector<HTMLElement>("[data-chart]");
  const login = current();
  if (!holder || state.place.kind !== "login" || !login) return;
  const days = usable(daysOf(login));
  if (!days) return;
  const comes = arriving || coming;
  coming = false;
  const width = Math.max(240, Math.floor(holder.clientWidth));
  const { dates, hours } = days;
  const total = days.total[unitNow()];
  const { values } = series(login, days);
  /** What a column over these days says, and what each of its parts is. */
  const said = (title: string, at: number[]) => {
    const each = values.map((of) => at.reduce((sum, day) => sum + of[day]!, 0));
    return { said: `${title}　${amount(each.reduce((a, b) => a + b, 0))}`, ...(each.length > 1 ? { parts: each.map(amount) } : {}) };
  };
  const axis = (value: number) => (unitNow() === "tokens" ? tokens(value) : `$${count(value)}`);
  if (state.chart === "days") {
    const from = Math.max(0, dates.length - SHOWN_DAYS);
    const columns: Column[] = Array.from({ length: dates.length - from }, (_, index) => {
      const date = dates[from + index]!;
      return { values: values.map((of) => of[from + index]!), ...said(dated(date), [from + index]), ...(date.getDay() === 1 ? { tick: `${date.getMonth() + 1}/${date.getDate()}` } : {}) };
    });
    holder.innerHTML = bars(`${login.id}-days-${state.split}-${unitNow()}`, columns, width, 280, axis);
  } else if (state.chart === "weeks") {
    const columns: Column[] = weeksOf(days)
      .filter((week) => week.length)
      .map((week) => {
        const start = dates[week[0]!]!;
        return { values: values.map((of) => week.reduce((sum, day) => sum + of[day]!, 0)), ...said(`${dated(start, false)} 起一周`, week), tick: `${start.getMonth() + 1}/${start.getDate()}` };
      });
    holder.innerHTML = bars(`${login.id}-weeks-${state.split}-${unitNow()}`, columns, width, 280, axis);
  } else if (state.chart === "year") {
    holder.innerHTML = calendar(login.id, dates, total, width, (index) => `${dated(dates[index]!)}　${total[index]! > 0 ? amount(total[index]!) : "无用量"}`);
  } else {
    holder.innerHTML = punch(login.id, hours, width, (weekday, hour) => `${weekdays[weekday]} ${hour} 点　${days.hoursAre === "requests" ? `${count(hours[weekday]![hour]!)} 次请求` : days.hoursAre === "tokens" ? tokens(hours[weekday]![hour]!) : dollars(hours[weekday]![hour]!)}`);
  }
  readout(holder, root.querySelector<HTMLElement>(".lg-legend"));
  if (comes) rise(holder);
}

/** Draws the page again with its chart changed: the new chart's marks come
 *  onto the page, and nothing else moves. */
function turned(): void {
  coming = true;
  draw();
}
// What was fetched comes in: a usage drawn for the first time rises.
whenArrived((key) => {
  const login = current();
  if (login && key === `days:${login.id}` && state.place.kind === "login" && state.place.tab === "usage") coming = true;
  draw();
});
on("chart", (button) => {
  state.chart = button.dataset.value as typeof state.chart;
  turned();
});
on("unit", (button) => {
  state.unit = button.dataset.value as typeof state.unit;
  turned();
});
on("split", (anchor) => {
  const login = current();
  const days = login && usable(daysOf(login));
  if (!login || !days) return;
  const ways = splitsOf(login, days);
  menu.open(anchor, "分组", [{ act: "none", label: "不分组", checked: state.split === "none" }, { rule: true as const }, ...ways.map((way) => ({ act: way, label: `按${splitNames[way]}`, checked: way === state.split }))], (way) => {
    state.split = way as Split | "none";
    turned();
  });
});

// ---- 调用

const sourceNames: Record<Source, string> = { main: "主循环", agent: "子代理", background: "后台" };
const outcomeNames: Record<Outcome, string> = { done: "完成", failed: "出错", stopped: "中断", held: "拦截" };
/** The ranges ready to choose; the ledger keeps a request's row 30 days. */
const presets = ["全部", "今天", "昨天", "最近 7 天", "最近 30 天"];
const DAY = 86400000;
const HOUR = 3600000;

/** The time the list is set to, as a range of instants. */
function rangeOf(): { from: Date | null; until: Date | null } {
  if (state.range) return { from: state.range.from, until: new Date(state.range.until.getTime() + DAY) };
  const midnight = new Date(NOW.getFullYear(), NOW.getMonth(), NOW.getDate());
  if (state.time === "今天") return { from: midnight, until: null };
  if (state.time === "昨天") return { from: new Date(midnight.getTime() - DAY), until: midnight };
  // A span that rolls with the clock begins on the hour: asked for by the
  // minute it would be another list each time the page is drawn, never the
  // one already read.
  const hour = NOW.getTime() - (NOW.getTime() % HOUR);
  if (state.time === "最近 7 天") return { from: new Date(hour - 7 * DAY), until: null };
  if (state.time === "最近 30 天") return { from: new Date(hour - 30 * DAY), until: null };
  return { from: null, until: null };
}

const queryOf = (): CallsQuery => ({ page: state.page, size: state.size, ...(state.turn ? { from: null, until: null } : rangeOf()), who: state.who, model: state.model, source: state.source, outcome: state.outcome, turn: state.turn });

/** The rows last drawn, for a press on one of them. */
let drawnRows: Call[] = [];
/** The list last read, and what it was the list of: while the same list is
 *  asked for again (its span moved on with the clock), it stays drawn. */
let lastCalls: { of: string; page: CallsPage } | null = null;
/** The models and people last seen in the login's ledger, to filter by. */
let seen: { models: string[]; people: string[] } = { models: [], people: [] };

function field(act: string, name: string, value: string | null): string {
  return `<button type="button" class="lg-field"${value ? " data-set" : ""} data-act="${act}">${escape(value ?? name)}${chevron()}</button>`;
}

function callsView(login: Login): string {
  const many = shared(login);
  // Codex tells no prices: its requests are counted in tokens.
  const priced = login.family !== "codex";
  const fetched = callsOf(login, queryOf());
  const of = JSON.stringify({ login: login.id, ...queryOf(), from: null, until: null, time: state.time, range: state.range });
  const read: CallsPage | null = usable(fetched);
  if (read) lastCalls = { of, page: read };
  const page: CallsPage | null = read ?? (fetched === null && lastCalls?.of === of ? lastCalls.page : null);
  if (page) seen = { models: page.models, people: page.people };
  const rows = page?.rows ?? [];
  drawnRows = rows;
  const cols = `${many ? "4.4em 4.6em" : "4.4em"} 8.2em minmax(0, 1fr) 5.2em 5em 5.6em 4.6em 1.2em`;
  let last = "";
  const list = rows
    .map((row, index) => {
      const none = row.outcome === "held";
      const shownIf = (value: string) => (none ? "" : value);
      const day = dated(row.at);
      const group = day === last ? "" : `<div class="lg-day">${day}</div>`;
      last = day;
      const talk = row.who !== null ? "一段对话" : row.source === "background" ? "后台" : row.source === "agent" ? `${row.talk ?? ""}　子代理 ${row.agent ?? ""}` : (row.talk ?? "");
      const mark =
        row.outcome === "failed"
          ? `<span class="lg-wrong" title="出错，未开始回答">${glyph("cross", 14)}</span>`
          : row.outcome === "stopped"
            ? `<span title="中断，回答未完成">${glyph("stop", 14)}</span>`
            : row.outcome === "held"
              ? `<span class="lg-caution" title="拦截：已达上限，未发出">${glyph("lock", 14)}</span>`
              : "";
      return `${group}<div class="lg-item${state.opened === index ? " open" : ""}">
        <div class="lg-row lg-cols" role="button" tabindex="0" data-act="open-call" data-index="${index}">
          <span class="lg-soft">${clock(row.at).slice(0, 5)}</span>
          ${many ? `<span>${escape(row.who ?? "我")}</span>` : ""}
          <span>${escape(row.model)}</span>
          <span class="lg-soft lg-clip">${escape(talk)}</span>
          <span class="lg-num lg-soft">${shownIf(tokens(row.fresh + row.cacheRead + row.cacheWrite))}</span>
          <span class="lg-num lg-soft">${shownIf(tokens(row.output))}</span>
          <span class="lg-num">${shownIf(priced ? dollars(row.quota) : tokens(row.fresh + row.cacheRead + row.cacheWrite + row.output))}</span>
          <span class="lg-num lg-soft">${shownIf(seconds(row.took))}</span>
          <span class="lg-mark-end">${mark}</span>
        </div>
        <div class="lg-fold"><div class="lg-fold-inner">${detail(login, row, index)}</div></div>
      </div>`;
    })
    .join("");
  const total = page ? (page.total ?? rows.length) : 0;
  const pageSum = (of: (row: Call) => number) => rows.reduce((all, row) => all + of(row), 0);
  const sum = page?.sum ?? { input: pageSum((row) => row.fresh + row.cacheRead + row.cacheWrite), output: pageSum((row) => row.output), quota: pageSum((row) => row.quota) };
  const pages = Math.max(1, Math.ceil(total / state.size));
  const near = [...new Set([1, 2, state.page - 1, state.page, state.page + 1, pages])].filter((one) => one >= 1 && one <= pages).sort((a, b) => a - b);
  const pager = near
    .map((one, index) => `${index && one - near[index - 1]! > 1 ? `<button type="button" class="lg-gap" data-act="jump" data-pages="${pages}" title="跳转到指定页">…</button>` : ""}<button type="button"${one === state.page ? ` class="current"` : ""} data-act="page" data-value="${one}">${count(one)}</button>`)
    .join("");
  const none = fetched instanceof Error ? `<p class="lg-none" title="${escape(fetched.message)}">读取失败</p>` : page ? `<p class="lg-none">无匹配记录</p>` : `<p class="lg-none lg-soft">正在读取…</p>`;
  return `<div class="lg-body lg-calls-view">
    <div class="lg-bar">
      ${state.turn ? `<button type="button" class="lg-field" data-set data-act="f-turn" title="仅显示对话中这一轮的请求；点按显示全部">对话中的一轮${glyph("close", 12)}</button>` : field("f-time", "时间", state.time === "全部" ? null : state.time)}${many ? field("f-who", "用户", state.who) : ""}${field("f-model", "型号", state.model)}${field("f-source", "来源", state.source && sourceNames[state.source])}${field("f-outcome", "结果", state.outcome && outcomeNames[state.outcome])}
    </div>
    <div class="lg-sheet"><div class="lg-list lg-calls" style="--cols:${cols}">
      <div class="lg-heads lg-cols"><span>时间</span>${many ? "<span>用户</span>" : ""}<span>型号</span><span>对话</span><span class="lg-num">输入</span><span class="lg-num">输出</span>${priced ? `<span class="lg-num" title="额度美元：请求的 API 价，Fable 另乘倍率">额度</span>` : `<span class="lg-num">合计</span>`}<span class="lg-num">耗时</span><span></span></div>${rule(1)}
      ${list || none}
    </div></div>
    <footer class="lg-foot" style="--cols:${cols}">
      ${doubleRule()}
      <div class="lg-sum lg-cols"><span class="lg-sum-name">${page?.sum ? "合计" : "本页合计"}<small>${count(page?.sum ? total : rows.length)} 次</small></span><span class="lg-num">${tokens(sum.input)}</span><span class="lg-num">${tokens(sum.output)}</span><span class="lg-num">${priced ? dollars(sum.quota) : tokens(sum.input + sum.output)}</span><span></span><span></span></div>
      <div class="lg-pages"><button type="button" class="lg-field lg-size" data-act="size">每页 ${state.size} 行${chevron()}</button><button type="button" data-act="page" data-value="${Math.max(1, state.page - 1)}" aria-label="上一页"${state.page === 1 ? " disabled" : ""}>${glyph("back", 14)}</button>${pager}<button type="button" class="lg-next" data-act="page" data-value="${Math.min(pages, state.page + 1)}" aria-label="下一页"${state.page === pages ? " disabled" : ""}>${glyph("back", 14)}</button></div>
    </footer>
  </div>`;
}

/** The rest of what one request was, under its row. */
function detail(login: Login, row: Call, index: number): string {
  const fact = (name: string, value: string) => `<dt>${name}</dt><dd>${value}</dd>`;
  const part = (row.quota / plateOf(login)) * 100;
  const codex = login.family === "codex";
  const speed = row.took > row.first ? `${Math.round(row.output / Math.max(row.took - row.first, 0.1))} token/s` : "";
  return `<div class="lg-detail">
    <dl>${fact("来源", row.source === "agent" ? `子代理 ${escape(row.agent ?? "")}` : sourceNames[row.source])}${row.effort ? fact("推理强度", escape(row.effort)) : ""}${fact("结果", outcomeNames[row.outcome])}</dl>
    <dl>${fact("新输入", count(row.fresh))}${fact(codex ? "缓存输入" : "缓存读", count(row.cacheRead))}${fact(codex ? "推理" : "缓存写", count(row.cacheWrite))}${fact("输出", count(row.output))}</dl>
    ${codex ? "" : `<dl>${fact("API 价", dollars(row.api))}${row.times === 1 ? "" : fact("Fable 倍率", `× ${row.times}`)}${fact("占周额度", part < 0.001 ? "< 0.001%" : `${part.toFixed(3)}%`)}</dl>`}
    <dl>${row.first ? fact("首字耗时", seconds(row.first)) : ""}${fact("总耗时", seconds(row.took))}${speed ? fact("输出速度", speed) : ""}</dl>
  </div>${row.opens ? `<div class="lg-detail-end"><button type="button" class="lg-word" data-act="open-turn" data-index="${index}">查看对话</button></div>` : ""}`;
}

/** The list drawn again from its first page, its rows coming in. */
function refiltered(): void {
  state.page = 1;
  state.opened = null;
  draw();
  document.querySelector(".lg-calls")?.classList.add("turned");
}

/** Sets one of the list's filters from a menu under its field. */
function filter<K extends "who" | "model" | "source" | "outcome">(key: K, title: string, options: [string | null, string][]): (anchor: HTMLElement) => void {
  return (anchor) =>
    menuUnder(anchor, title, options.map(([value, label]) => ({ act: value ?? "", label, checked: (state[key] ?? "") === (value ?? "") })), (value) => {
      (state[key] as string | null) = value || null;
      refiltered();
    });
}
on("f-who", (anchor) => {
  const login = current();
  if (login) filter("who", "用户", [[null, "全部用户"], ["我", "我"], ...[...new Set([...grantsOf(login).map((grant) => grant.person), ...seen.people])].map((person): [string, string] => [person, person])])(anchor);
});
on("f-turn", () => {
  state.turn = null;
  refiltered();
});
on("f-model", (anchor) => filter("model", "型号", [[null, "全部型号"], ...seen.models.map((model): [string, string] => [model, model])])(anchor));
on("f-source", (anchor) => filter("source", "来源", [[null, "全部来源"], ["main", "主循环"], ["agent", "子代理"], ...(current()?.family === "claude" ? [["background", "后台"] as [string, string]] : [])])(anchor));
on("f-outcome", (anchor) => filter("outcome", "结果", [[null, "全部结果"], ["done", "完成"], ["failed", "出错"], ["stopped", "中断"], ["held", "拦截"]])(anchor));

/** The time filter: ranges ready to choose beside a month to pick a first
 *  and a last day in. */
on("f-time", (anchor) => {
  document.querySelector(".lg-pop")?.remove();
  const pop = document.createElement("div");
  pop.className = "lg-pop";
  let month = new Date(NOW.getFullYear(), NOW.getMonth(), 1);
  let first: Date | null = null;
  const oldest = new Date(NOW.getFullYear(), NOW.getMonth(), NOW.getDate() - 30);
  const said = (day: Date) => `${day.getMonth() + 1} 月 ${day.getDate()} 日`;
  const paint = () => {
    const lead = (month.getDay() + 6) % 7;
    const length = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate();
    const cells = Array.from({ length }, (_, index) => {
      const day = new Date(month.getFullYear(), month.getMonth(), index + 1);
      return `<button type="button" data-day="${index + 1}" class="${first && day.getTime() === first.getTime() ? "chosen" : ""}"${day > NOW || day < oldest ? " disabled" : ""}>${index + 1}</button>`;
    });
    pop.innerHTML = `<div class="lg-pop-list">${presets.map((name) => `<button type="button" class="${state.time === name ? "chosen" : ""}" data-preset="${name}">${name}</button>`).join("")}</div>
      <div class="lg-cal">
        <header><button type="button" data-month="-1" aria-label="上个月">${glyph("back", 14)}</button><b>${month.getFullYear()} 年 ${month.getMonth() + 1} 月</b><button type="button" class="lg-next" data-month="1" aria-label="下个月">${glyph("back", 14)}</button></header>
        <div class="lg-cal-grid">${["一", "二", "三", "四", "五", "六", "日"].map((name) => `<i>${name}</i>`).join("")}${"<span></span>".repeat(lead)}${cells.join("")}</div>
        <p>${first ? `${said(first)} 起，选择结束日期` : "选择起始日期（保留 30 天）"}</p>
      </div>`;
  };
  const close = () => {
    pop.remove();
    document.removeEventListener("pointerdown", outside, true);
    document.removeEventListener("keydown", escaped, true);
  };
  const outside = (event: Event) => {
    if (!pop.contains(event.target as Node)) close();
  };
  const escaped = (event: KeyboardEvent) => {
    if (event.key === "Escape") close();
  };
  pop.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement).closest<HTMLElement>("button");
    if (!target) return;
    if (target.dataset.preset) {
      state.time = target.dataset.preset;
      state.range = null;
      close();
      refiltered();
    } else if (target.dataset.month) {
      month = new Date(month.getFullYear(), month.getMonth() + Number(target.dataset.month), 1);
      paint();
    } else if (target.dataset.day) {
      const day = new Date(month.getFullYear(), month.getMonth(), Number(target.dataset.day));
      if (!first) {
        first = day;
        paint();
        return;
      }
      const [from, to] = day < first ? [day, first] : [first, day];
      state.time = from.getTime() === to.getTime() ? said(from) : `${said(from)} – ${said(to)}`;
      state.range = { from, until: to };
      close();
      refiltered();
    }
  });
  paint();
  const box = anchor.getBoundingClientRect();
  pop.style.left = `${Math.round(box.left)}px`;
  pop.style.top = `${Math.round(box.bottom + 6)}px`;
  document.body.append(pop);
  window.setTimeout(() => {
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escaped, true);
  });
});

on("size", (anchor) =>
  menu.open(anchor, "每页行数", [20, 50, 100].map((size) => ({ act: String(size), label: `${size} 行`, checked: size === state.size })), (size) => {
    state.size = Number(size);
    refiltered();
  }),
);
function toPage(page: number): void {
  state.page = page;
  state.opened = null;
  draw();
  document.querySelector(".lg-calls")?.classList.add("turned");
  document.querySelector(".lg-calls-view .lg-sheet")?.scrollTo({ top: 0 });
}
on("page", (button) => toPage(Number(button.dataset.value)));
/** The gap between page numbers becomes a field for the page to go to. */
on("jump", (gap) => {
  const pages = Number(gap.dataset.pages);
  const input = document.createElement("input");
  input.className = "lg-jump";
  input.inputMode = "numeric";
  input.placeholder = "页码";
  input.setAttribute("aria-label", "跳转到指定页");
  gap.replaceWith(input);
  input.focus();
  input.addEventListener("keydown", (event) => {
    if (event.key === "Escape") draw();
    if (event.key !== "Enter") return;
    const page = Math.round(Number(input.value));
    if (page >= 1) toPage(Math.min(pages, page));
    else draw();
  });
  input.addEventListener("blur", () => window.setTimeout(() => input.isConnected && draw(), 0));
});
on("open-call", (row) => {
  const item = row.closest<HTMLElement>(".lg-item")!;
  const open = !item.classList.contains("open");
  item.parentElement?.querySelector(".lg-item.open")?.classList.remove("open");
  item.classList.toggle("open", open);
  state.opened = open ? Number(row.dataset.index) : null;
});
on("open-turn", (button) => {
  const login = current();
  const row = drawnRows[Number(button.dataset.index)];
  if (login && row && deeds) deeds.openTurn(login, row);
  else say("查看对话（样品未实现）");
});

// ---- 协助

/** What a person is given comes to, in a line: their share's dollars this
 *  week, and what they can see and do. */
export function limitSaid(login: Login, limit: number | null, manages = false): string {
  const comes = limit === null ? "" : `本周 ${whole((plateOf(login) * limit) / 100)}${login.family === "claude" ? `，其中 Fable 至多 ${whole((plateOf(login) * limit) / 200)}` : ""}，随周额度变动。`;
  return manages ? `${limit === null ? "与号主共用全部额度。" : comes}可见整个号的真实额度与推算，可调整各人份额。` : `${comes}对方可见本人额度、份额与预期周额度。`;
}

/** The ways a share is set: a few shares, or any share from 1 to 100 typed
 *  in its field, which takes effect as it is left or Enter is pressed (the
 *  user, 2026-11-17: "应该能输入一个特定的百分比"); and, where `none`, no
 *  limit at all, which one who manages the login with its owner may have. */
const LIMITS = ["10", "25", "50"];
function limitRow(limit: number | null, who: string, none: boolean): string {
  const typed = limit !== null && !LIMITS.includes(String(limit)) ? String(limit) : "";
  const unlimited: [string, string][] = none ? [["none", "不限"]] : [];
  const options = choices("grant-limit", [...unlimited, ...LIMITS.map((value): [string, string] => [value, `${value}%`])], typed ? "" : String(limit ?? "none"), `data-person="${who}"`);
  return `<div class="as-limit">${options}<label class="as-custom${typed ? " chosen" : ""}"><input type="text" inputmode="numeric" placeholder="自定" value="${typed}" data-limit-of="${who}" aria-label="自定比例"><span>%</span></label></div>`;
}

/** This week's quota dollars by person, as the login's usage says: the
 *  owner's, the reader's where the login is another's, and each other's. */
function spent(login: Login): { owner: { week: number; fable: number }; me: { week: number; fable: number }; of: (person: string) => { week: number; fable: number } } | null {
  const days = usable(daysOf(login));
  if (!days) return null;
  const none = { week: 0, fable: 0 };
  return {
    owner: days.people.find((one) => one.name === null) ?? none,
    me: days.people.find((one) => one.me) ?? none,
    of: (person) => days.people.find((one) => one.name === person && !one.me) ?? none,
  };
}

/** The reader, where they are one of those a login assists: named so in
 *  what a press on their own row asks. */

function assist(login: Login): string {
  const others = grantsOf(login);
  const line = lines[login.id] ?? { fiveStop: null, turns: null };
  // Its owner gives, ends and limits, and says who manages it with them;
  // one who does sets the shares of those it assists, their own among them.
  const owner = !login.lent;
  // One who manages it sets their own share, whoever else it assists.
  if (!others.length && owner)
    return `<div class="lg-body lg-empty"><span class="lg-empty-marks">${familyMark(login, 34)}${glyph("colleague", 34)}</span><p>此登录未协助任何人</p>${owner ? `<button type="button" class="lg-go" data-act="add-grant">添加协助</button>` : ""}</div>`;
  const plate = plateOf(login);
  const hasFable = login.family === "claude";
  const cols = "7em 6.4em 15em 15em minmax(7em, 1fr) 2em";
  const used = spent(login);
  const pct = (dollars: number) => (dollars / plate) * 100;
  const none = { week: 0, fable: 0 };
  /** The login's owner, measured against the whole week. */
  const ownerRow = (who: string) => {
    const of = used?.owner ?? none;
    const week = pct(of.week);
    const fable = (pct(of.fable) / 50) * 100;
    return `<div class="lg-item"><div class="lg-row lg-cols lg-still"><span>${who}</span><span class="lg-soft">号主</span>${gauge(`${login.id}-owner-week`, Math.round(week), { under: money(week, plate) })}${hasFable ? gauge(`${login.id}-owner-fable`, Math.round(fable), { under: money(fable, plate / 2) }) : "<span></span>"}<span></span><span></span></div></div>`;
  };
  /** One the login assists: against their share where they have one, the
   *  whole week where they have none; opened, what the reader may set. */
  const row = (key: string, who: string, given: { limit: number | null; manages?: boolean; until: string | null; paused?: boolean; direct?: boolean }, person: { week: number; fable: number }) => {
    const open = state.editing === key;
    const weekPct = pct(person.week);
    const fablePct = pct(person.fable);
    const of = given.limit === null ? plate : (plate * given.limit) / 100;
    const usedOf = given.limit === null ? weekPct : (weekPct / given.limit) * 100;
    const fable = given.limit === null ? (fablePct / 50) * 100 : (fablePct / (given.limit / 2)) * 100;
    const until = given.until === null ? "none" : "pick";
    const share = given.manages ? (given.limit === null ? "共管" : `共管　${given.limit}%`) : `${given.limit ?? 100}%`;
    // The owner alone gives no limit to another; one who manages may take
    // their own away again.
    const unlimited = !!given.manages && (owner || key === ME);
    return `<div class="lg-item${open ? " open" : ""}${state.fresh === key ? " fresh" : ""}">
        <div class="lg-row lg-cols" role="button" tabindex="0" data-act="edit-grant" data-person="${key}">
          <span>${who}</span>
          <span class="lg-soft">${share}</span>
          ${gauge(`${login.id}-${key}-week`, Math.round(usedOf), { near: given.limit !== null && usedOf >= 85, under: money(usedOf, of) })}
          ${hasFable ? gauge(`${login.id}-${key}-fable`, Math.round(fable), { near: given.limit !== null && fable >= 85, under: money(fable, of / 2) }) : "<span></span>"}
          <span class="lg-soft">${given.paused ? `<span class="lg-caution">已暂停</span>` : escape(given.until ?? "不限期")}</span>
          <span class="lg-mark-end">${chevron()}</span>
        </div>
        <div class="lg-fold"><div class="lg-fold-inner"><div class="lg-edit">
          ${owner && hasFable ? `<div class="choice-row"><span class="choice-name">身份</span>${choices("grant-role", [["plain", "普通"], ["manage", "共管"]], given.manages ? "manage" : "plain", `data-person="${key}"`)}</div>` : ""}
          <div class="choice-row"><span class="choice-name">份额</span><div>${hasFable ? `${limitRow(given.limit, key, unlimited)}<p class="lg-edit-note">${limitSaid(login, given.limit, given.manages)}</p>` : `<p class="lg-edit-note">Codex 不按人计额度；可用下方通用限制里的 5 小时额度暂停协助。</p>`}</div></div>
          ${owner ? `<div class="choice-row"><span class="choice-name">期限</span>${choices("grant-until", [["none", "不限期"], ["pick", until === "pick" ? escape(given.until!) : "指定…"]], until, `data-person="${key}"`)}</div>
          ${login.ownMachine ? `<div class="choice-row"><span class="choice-name">直连</span><div>${choices("grant-direct", [["on", "允许"], ["off", "不允许"]], login.allowsDirect && given.direct !== false ? "on" : "off", `data-person="${key}"${login.allowsDirect ? "" : " disabled"}`)}<p class="lg-edit-note">${login.allowsDirect ? "不允许时，对方的机器与这台机器之间只经中枢往来，对方不会得知这台机器的地址。" : "这台机器未开启「允许直连」，与所有人都只经中枢往来。"}</p></div></div>` : ""}
          <div class="lg-edit-end"><button type="button" class="lg-word lg-danger" data-act="grant-end" data-person="${key}">收回</button></div>` : ""}
        </div></div></div>
      </div>`;
  };
  const heads = owner ? ownerRow("我") : ownerRow(escape(login.lent!.by)) + row(ME, "我", login.lent!, used?.me ?? none);
  const rows = others.map((grant) => row(escape(grant.person), escape(grant.person), grant, used ? used.of(grant.person) : { week: (grant.used / 100) * plate, fable: (grant.fable / 100) * plate })).join("");
  return `<div class="lg-body lg-sheet"><div class="lg-list lg-people" style="--cols:${cols}">
      <div class="lg-heads lg-cols"><span>用户</span><span>份额</span><span>本周</span><span${hasFable ? ` title="${fableSaid}"` : ""}>${hasFable ? "Fable" : ""}</span><span>期限</span><span></span></div>${rule(1)}
      ${heads}${rows}
      ${owner ? `<div class="lg-add"><button type="button" class="lg-word lg-accent" data-act="add-grant">${glyph("plus", 14)}添加协助</button></div>` : ""}
    </div>
    ${owner ? `<section class="lg-lines">
      <h3>通用限制</h3>
      <div class="choice-row"><span class="choice-name">5 小时额度</span><div>${choices("line-five", [["none", "不限"], ["60", "60%"], ["70", "70%"], ["80", "80%"], ["90", "90%"]], String(line.fiveStop ?? "none"))}<p class="lg-edit-note">此登录的 5 小时额度达到该比例时，暂停全部协助。</p></div></div>
      <div class="choice-row"><span class="choice-name">并发轮数</span>${choices("line-turns", [["none", "自动"], ["1", "1"], ["2", "2"], ["3", "3"], ["4", "4"]], String(line.turns ?? "none"))}</div>
      ${hasFable ? `<div class="choice-row"><span class="choice-name">Fable</span><p>全号 Fable 用量达到本周周额度的 50% 时暂停 Fable（含号主），其他型号不受影响。</p></div>` : ""}
    </section>` : ""}
  </div>`;
}

const grantOf = (element: HTMLElement) => {
  const login = current();
  return login ? grantsOf(login).find((grant) => grant.person === element.dataset.person) : undefined;
};

on("edit-grant", (row) => {
  const item = row.closest<HTMLElement>(".lg-item")!;
  const open = !item.classList.contains("open");
  item.parentElement?.querySelector(".lg-item.open")?.classList.remove("open");
  item.classList.toggle("open", open);
  state.editing = open ? row.dataset.person! : null;
});
on("grant-limit", (button) => setLimit(button, button.dataset.value === "none" ? null : Number(button.dataset.value)));
/** Sets the share of the person `element` names, the reader's own where
 *  it names them. */
function setLimit(element: HTMLElement, limit: number | null): void {
  const login = current();
  if (!login) return;
  if (element.dataset.person === ME) {
    const lent = login.lent;
    if (!lent || lent.limit === limit) return;
    if (deeds) void deeds.assist(login, ME, limit, null).then(() => draw(), (error: unknown) => say(error instanceof Error ? error.message : "更改失败"));
    else {
      lent.limit = limit;
      draw();
    }
    return;
  }
  const grant = grantOf(element);
  if (!grant || grant.limit === limit) return;
  if (deeds) void deeds.assist(login, grant.person, limit, grant.until === null ? null : untilOf(login, grant.person), grant.manages).then(() => draw(), (error: unknown) => say(error instanceof Error ? error.message : "更改失败"));
  else {
    grant.limit = limit;
    draw();
  }
}
/** Whether a person manages the login with its owner, as its owner says:
 *  one who no longer does is left a share, having had none. */
on("grant-role", (button) => {
  const login = current();
  const grant = grantOf(button);
  const manages = button.dataset.value === "manage";
  if (!login || !grant || !!grant.manages === manages) return;
  const limit = manages ? grant.limit : (grant.limit ?? 25);
  if (deeds) void deeds.assist(login, grant.person, limit, grant.until === null ? null : untilOf(login, grant.person), manages).then(() => draw(), (error: unknown) => say(error instanceof Error ? error.message : "更改失败"));
  else {
    if (manages) grant.manages = true;
    else delete grant.manages;
    grant.limit = limit;
    draw();
  }
});
/** Whether a person's machines link with the login's machine directly, as
 *  its owner says; where the machine takes no direct connections there is
 *  nothing to say. */
on("grant-direct", (button) => {
  const login = current();
  const grant = grantOf(button);
  const direct = button.dataset.value === "on";
  if (!login || !grant || !login.allowsDirect || (grant.direct !== false) === direct) return;
  if (deeds) void deeds.assist(login, grant.person, grant.limit, grant.until === null ? null : untilOf(login, grant.person), grant.manages, direct).then(() => draw(), (error: unknown) => say(error instanceof Error ? error.message : "更改失败"));
  else {
    if (direct) delete grant.direct;
    else grant.direct = false;
    draw();
  }
});
// A share typed in its field takes effect as the field is left, or Enter
// is pressed in it; what is not a share from 1 to 100 is put back.
document.addEventListener("change", (event) => {
  const field = (event.target as HTMLElement | null)?.closest?.<HTMLInputElement>("input[data-limit-of]");
  if (!field) return;
  // A whole number, perhaps with its percent sign: `1.5`, `-20` or `1e2`
  // are no share, never read as another number with their marks dropped.
  const text = field.value.trim().replace(/%$/, "").trim();
  const typed = /^\d{1,3}$/.test(text) ? Number(text) : Number.NaN;
  if (!(typed >= 1 && typed <= 100)) {
    draw();
    return;
  }
  field.dataset.person = field.dataset.limitOf;
  setLimit(field, typed);
});
document.addEventListener("keydown", (event) => {
  const field = (event.target as HTMLElement | null)?.closest?.<HTMLInputElement>("input[data-limit-of]");
  if (field && event.key === "Enter") field.blur();
});
/** When an assistance ends, as the World said it; the specimen keeps none. */
let untilOf: (login: Login, person: string) => Date | null = () => null;
export function setUntils(how: (login: Login, person: string) => Date | null): void {
  untilOf = how;
}
function setLine(change: (line: { fiveStop: number | null; turns: number | null }) => void): void {
  const login = current();
  if (!login) return;
  const line = { ...(lines[login.id] ?? { fiveStop: null, turns: null }) };
  change(line);
  if (deeds) void deeds.line(login, line).then(() => draw());
  else {
    lines[login.id] = line;
    draw();
  }
}
on("line-five", (button) => setLine((line) => (line.fiveStop = button.dataset.value === "none" ? null : Number(button.dataset.value))));
on("line-turns", (button) => setLine((line) => (line.turns = button.dataset.value === "none" ? null : Number(button.dataset.value))));

// ---- 设置

/** What a login's week is reckoned by, in a line: this week's figure and
 *  what its owner expects; and, where the last weeks all came to one side
 *  of its reach, that what is expected wants a look. */
function expectLine(login: Login, weeks: number[]): string {
  const expect = expectOf(login);
  const side = pinned(weeks, expect);
  return `本周 ${whole(plateOf(login))}　预期 ${whole(expect.base)} ±${expect.swing}%${side ? `　<span class="lg-caution">近两周${side === "low" ? "低于下沿" : "高于上沿"}</span>` : ""}`;
}

const planChoices: [PlanLabel | "none", string][] = [["none", "未设置"], ["pro", planNames.pro], ["max", planNames.max], ["ultra", planNames.ultra], ["free", planNames.free]];

function settings(login: Login): string {
  const set = (name: string, said: string, control: string, more = "") => `<div class="lg-set"><div class="lg-set-what"><b>${name}</b><small>${said}</small></div><span class="lg-set-end">${control}</span>${more}</div>`;
  const id = escape(login.id);
  const word = (act: string, text: string, danger = false) => `<button type="button" class="lg-word${danger ? " lg-danger" : ""}" data-act="${act}" data-id="${id}">${text}</button>`;
  const days = usable(daysOf(login));
  const resets = days?.resets ?? null;
  const history = resets?.length
    ? `<div class="lg-item${state.editing === "resets" ? " open" : ""}"><div class="lg-fold"><div class="lg-fold-inner"><ul class="lg-resets">${resets.map((reset) => `<li><span>${escape(reset.at)}</span><span class="lg-soft">${escape(reset.how)}</span><span class="lg-soft">${reset.from}% → ${reset.to}%</span><span>${reset.undo ? `<button type="button" class="lg-word" data-act="undo-reset" data-id="${id}">撤回</button>` : ""}</span></li>`).join("")}</ul></div></div></div>`
    : "";
  const claude = login.family === "claude";
  const models = login.models?.length ? login.models.join("、") : "尚未读取";
  const plan = login.token
    ? admin
      ? set("套餐", "长期令牌不提供套餐，由管理员设置显示", choices("plan", planChoices, login.planSet ?? "none", `data-id="${id}"`))
      : set("套餐", login.planSet ? `${planNames[login.planSet]}　由管理员设置` : "长期令牌不提供套餐", "")
    : login.plan
      ? set("套餐", `${escape(login.plan)}　由 ${escape(familyName(login.family))} 报告`, "")
      : "";
  const compacts = login.standing === "in" && (login.family === "claude" || login.family === "codex");
  const idle = `${compacts ? set("闲置自动压缩", "对话闲置够久后自动执行一次 /compact，只对你自己的对话生效", toggle("idle-on", !!login.idle?.on, "", `data-id="${id}"`)) : ""}
    ${compacts && login.idle?.on ? set("压缩条件", `闲置 ${login.idle.after_minutes} 分钟后，上下文至少 ${tenThousands(login.idle.min_tokens)}`, word("idle-edit", "编辑")) : ""}`;
  if (login.lent) return `<div class="lg-body lg-sheet"><div class="lg-sets">${idle}</div></div>`;
  return `<div class="lg-body lg-sheet"><div class="lg-sets">
    ${set("工作台", "在 Fastctx 中对话，记录用量与额度", toggle("bench", login.workbench, "", `data-id="${id}"`))}
    ${set("接入", login.drift ? `已接入　<span class="lg-caution">${escape(login.drift)}</span>` : "已接入 Fastctx", `${login.drift ? word("attach", "更新接入") : ""}${word("detach", "移除", true)}`)}
    ${plan}
    ${claude ? set("模型", `${escape(models)}${login.modelsChanged ? "　已修改" : ""}`, word("models", "编辑")) : set("自动命名对话", "为新对话生成标题，计一次请求", toggle("auto-title", login.autoTitle ?? true, "", `data-id="${id}"`))}
    ${idle}
    ${claude ? set("周额度", expectLine(login, (days?.weeks ?? []).map((week) => week.value)), word("expect", "编辑")) : ""}
    ${claude ? set("Fable 倍率", `× ${FABLE_TIMES}　其他型号按 API 价计`, "") : set("计量", "按 API 价计", "")}
    ${claude ? set("重置用量", "厂商重置或部分重置额度后，将本周用量调至相应比例", word("reset-usage", "重置")) : ""}
    ${resets ? `<div class="lg-set lg-set-fold"><div class="lg-set-what"><b>重置记录</b><small>${resets[0] ? `最近：${escape(resets[0].at)}　${escape(resets[0].how)}` : "暂无记录"}</small></div><span class="lg-set-end">${resets.length ? `<button type="button" class="lg-word" data-act="show-resets">${state.editing === "resets" ? "收起" : "查看"}</button>` : ""}</span>${history}</div>` : ""}
    ${set("额度读数", login.readAt ? `最近读取：${escape(login.readAt)}` : "尚未读取", word("reread", "重新读取"))}
    ${set("目录", escape(login.dir), word("copy-dir", "复制"))}
    ${login.program ? set("程序", escape(login.program), "") : ""}
  </div></div>`;
}

on("show-resets", (button) => {
  const item = button.closest(".lg-set")!.querySelector(".lg-item");
  if (!item) return;
  const open = item.classList.toggle("open");
  state.editing = open ? "resets" : null;
  button.textContent = open ? "收起" : "查看";
});
on("copy-dir", (button) => {
  const login = current();
  if (!login) return;
  void navigator.clipboard?.writeText(login.dir).catch(() => {});
  button.textContent = "已复制";
  window.setTimeout(() => (button.textContent = "复制"), 1600);
});
on("reread", () => {
  const login = current();
  if (login && deeds) deeds.reread(login);
  else say("已请求重新读取额度");
});
/** A switch on the login that the World keeps: flipped at once, put back
 *  where the World did not take it. */
function flip(button: HTMLElement, keep: (login: Login, on: boolean) => Promise<void>): void {
  const login = current();
  const on = button.getAttribute("aria-checked") !== "true";
  button.setAttribute("aria-checked", String(on));
  if (!login || !deeds) return;
  keep(login, on).catch((error: unknown) => {
    button.setAttribute("aria-checked", String(!on));
    say(error instanceof Error ? error.message : "更改失败");
  });
}
on("auto-title", (button) => flip(button, (login, on) => deeds!.autoTitle(login, on)));
on("idle-on", (button) => flip(button, (login, on) => deeds!.idleOn(login, on)));
on("idle-edit", () => {
  const login = current();
  if (login && deeds) deeds.idle(login);
  else say("压缩条件（样品未实现）");
});

/** Tokens in tens of thousands, as the page writes context. */
function tenThousands(tokens: number): string {
  return `${Number((tokens / 10_000).toFixed(1))} 万`;
}
on("plan", (button) => {
  const login = current();
  if (!login) return;
  const plan = button.dataset.value === "none" ? null : (button.dataset.value as PlanLabel);
  if (deeds) void deeds.plan(login, plan).then(() => draw());
  else {
    login.planSet = plan;
    login.plan = plan ? planNames[plan] : null;
    draw();
  }
});
