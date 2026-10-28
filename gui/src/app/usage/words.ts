// What the usage of an agent's login says (`world-usage.md`, 用量): amounts
// as short as a chart's labels, and the hovers that carry the rest, how
// Claude Code reckons its dollars among them.

import { dollars, underCent } from "../../world/money";

export interface UsageWords {
  me: string;
  /** Whoever uses the machine itself, where no owner is known. */
  machineItself: string;
  aboutWhy: string;
  money(usd: number): string;
  tokens(count: number): string;
  tokenUnit: string;
  /** The page's head. */
  newTalk: string;
  more: string;
  /** The strip across the top. */
  fiveHours: string;
  sevenDays: string;
  window(minutes: number | null, name: string): string;
  resetsIn(left: string): string;
  notRead: string;
  thisWeek: string;
  myWeek: string;
  whoseWeek(name: string): string;
  /** The week's span, and what it follows. */
  weekSpan(start: Date, end: Date): string;
  weekWhy(window: boolean): string;
  vsLastWeek(change: number): string;
  noLastWeek: string;
  today(amount: string | null): string;
  who: string;
  assistUntil(until: Date | null, now: Date): string;
  untilShort(until: Date, now: Date): string;
  weekly(amount: string): string;
  daily(amount: string): string;
  fiveHourCap(percent: number): string;
  turnsCap(turns: number): string;
  noCap: string;
  /** The rings of an assistance's allowances, to the account it assists. */
  aDay: string;
  aWeek: string;
  lastDay: string;
  dayCapSaid(cap: string, spent: string): string;
  weekCapSaid(cap: string, spent: string): string;
  /** A person's own weekly allowance, beside the windows. */
  capRing(name: string): string;
  /** The Fable limit's ring, half the weekly one, and its hover. */
  fable: string;
  fableCapSaid(cap: string, spent: string): string;
  /** The allowances' bars, by name. */
  dayCap: string;
  weekCap: string;
  /** What one used this week, for their row's hover. */
  spentWeek(name: string, amount: string): string;
  /** The row the people past the first few fold into, and its undoing. */
  others(count: number): string;
  fewer: string;
  capOf(spent: string, cap: string): string;
  /** Each person's part of the account's seven-day window. */
  partOf(name: string, percent: number): string;
  partSaid(name: string, percent: number): string;
  partHead: string;
  partsWhy: string;
  weekReached(percent: number): string;
  /** The tabs and the unit. */
  tabs: { calendar: string; quota: string; weeks: string; punch: string; calls: string };
  usd: string;
  tokenTab: string;
  /** The calendar. */
  year: string;
  recentWeeks(weeks: number): string;
  less: string;
  most: string;
  weekday: string[];
  weekRow: string;
  month(month: number): string;
  day(at: Date): string;
  weekFrom(at: Date): string;
  unused: string;
  yearTotal(amount: string): string;
  daysUsed(days: number): string;
  longestRun(days: number): string;
  busiest(day: string, amount: string): string;
  /** The quota. */
  quotaTitle: string;
  now: string;
  reset: string;
  fullBy(at: Date): string;
  reachBy(percent: number): string;
  quotaUnread: string;
  quotaAt(at: Date, week: number | null, five: number | null): string;
  /** A weekly allowance through the week, in place of the quota. */
  capTitle(amount: string): string;
  capAt(at: Date, percent: number, amount: string): string;
  /** The reset weeks. */
  weeksTitle(weeks: number): string;
  weeksNone: string;
  cap(amount: string): string;
  /** The hours. */
  punchTitle: string;
  punchSaid(night: number): string;
  punchAt(weekday: string, hour: number, amount: string | null): string;
  /** Where it went, and the models. */
  where: string;
  byMoney: string;
  fromCache(percent: number): string;
  kinds: { input: string; cache_read: string; cache_write: string; output: string; reasoning: string; fresh: string; cached: string };
  models: string;
  thisWeekShort: string;
  noneThisWeek: string;
  /** The page as a whole. */
  reading: string;
  failed: string;
  away: string;
  notListed: string;
  workbenchOff: string;
}

const zhWeekday = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

/** Dollars as short as a label can be: to the cent below a hundred, whole
 *  above it. */
function money(usd: number): string {
  if (usd === 0) return "$0";
  if (underCent(usd)) return "<$0.01";
  if (usd >= 100) return `$${Math.round(usd).toLocaleString("en-US")}`;
  return dollars(usd);
}

function tokens(count: number): string {
  const short = (value: number, unit: string) => `${value >= 10 ? Math.round(value) : Math.round(value * 10) / 10}${unit}`;
  if (count >= 1e9) return short(count / 1e9, "B");
  if (count >= 1e6) return short(count / 1e6, "M");
  if (count >= 1e3) return short(count / 1e3, "k");
  return `${Math.round(count)}`;
}

function clock(at: Date): string {
  return `${at.getHours()}:${String(at.getMinutes()).padStart(2, "0")}`;
}

export const zhUsage: UsageWords = {
  me: "我",
  machineItself: "这台机器上",
  aboutWhy: "由 Claude Code 自己计算：它按内置的 Anthropic API 官方价目表，把每个请求的 token 折成美元，累加成会话总额；Fastctx 经官方 mod 接口的 session.measure 事件读取这个总额，按增量记账。订阅登录不实际扣这笔钱。",
  money,
  tokens,
  tokenUnit: "token",
  newTalk: "新对话",
  more: "更多",
  fiveHours: "5 小时",
  sevenDays: "7 天",
  window: (minutes, name) => (minutes === 300 || name.startsWith("five_hour") ? "5 小时" : minutes === 10080 || name.startsWith("seven_day") ? "7 天" : name),
  resetsIn: (left) => `${left} 后重置`,
  notRead: "还没读到",
  thisWeek: "这一周",
  myWeek: "我这一周",
  whoseWeek: (name) => `${name} 这一周`,
  weekSpan: (start, end) => `${start.getMonth() + 1}月${start.getDate()}日 ${clock(start)} – ${end.getMonth() + 1}月${end.getDate()}日 ${clock(end)}`,
  weekWhy: (window) => (window ? "和这个登录的 7 天额度同一周" : "最近 7 天：还没读到这个登录的 7 天额度"),
  vsLastWeek: (change) => `比上一周 ${change > 0 ? "+" : ""}${change}%`,
  noLastWeek: "上一周没有用",
  today: (amount) => (amount ? `今天 ${amount}` : "今天还没用"),
  who: "这一周谁在用",
  assistUntil: (until, now) => (until ? `到 ${until.getMonth() + 1}月${until.getDate()}日 ${clock(until)}${until.getFullYear() !== now.getFullYear() ? ` ${until.getFullYear()}` : ""}` : "不限期"),
  untilShort: (until, now) => `${until.getMonth() + 1}月${until.getDate()}日${until.getFullYear() !== now.getFullYear() ? ` ${until.getFullYear()}` : ""}`,
  weekly: (amount) => `每周 约 ${amount}`,
  daily: (amount) => `每天 约 ${amount}`,
  fiveHourCap: (percent) => `5 小时额度用到 ${percent}% 停`,
  turnsCap: (turns) => `同时至多 ${turns} 轮`,
  noCap: "不限额",
  aDay: "每天",
  aWeek: "每周",
  lastDay: "最近 24 小时",
  dayCapSaid: (cap, spent) => `每天上限 约 ${cap}\n最近 24 小时用了 约 ${spent}`,
  weekCapSaid: (cap, spent) => `每周上限 约 ${cap}\n这一周用了 约 ${spent}`,
  capRing: (name) => `${name} 的上限`,
  fable: "Fable",
  fableCapSaid: (cap, spent) => `Fable 上限是每周上限的一半，约 ${cap}\n这一周用了 约 ${spent}`,
  dayCap: "每天上限",
  weekCap: "每周上限",
  spentWeek: (name, amount) => `${name} 这一周用了 ${amount}`,
  others: (count) => `其余 ${count} 人`,
  fewer: "收起",
  capOf: (spent, cap) => `${spent} / ${cap}`,
  partOf: (name, percent) => `${name} 约 ${percent}%`,
  partSaid: (name, percent) => `${name} 用掉整个号这一周额度的约 ${percent}%`,
  partHead: "占周额度",
  partsWhy: "按这一周各人在 Fastctx 里记下的用量分摊整个号的 7 天额度：Claude 按 Claude Code 账本里的美元，Codex 按 token，缓存读算十分之一、输出算八倍（OpenAI 标价的比例）",
  weekReached: (percent) => `整个号 7 天额度用到 ${percent}%`,
  tabs: { calendar: "日历", quota: "额度", weeks: "每周", punch: "时段", calls: "调用" },
  usd: "$",
  tokenTab: "token",
  year: "过去一年",
  recentWeeks: (weeks) => `近 ${weeks} 周`,
  less: "少",
  most: "多",
  weekday: zhWeekday,
  weekRow: "周",
  month: (month) => `${month + 1}月`,
  day: (at) => `${at.getMonth() + 1}月${at.getDate()}日 ${zhWeekday[(at.getDay() + 6) % 7]}`,
  weekFrom: (at) => `${at.getMonth() + 1}月${at.getDate()}日 起的一周`,
  unused: "没有用",
  yearTotal: (amount) => `一年 ${amount}`,
  daysUsed: (days) => `用过 ${days} 天`,
  longestRun: (days) => `最长连着 ${days} 天`,
  busiest: (day, amount) => `最多的一天 ${day} ${amount}`,
  quotaTitle: "这一周的额度",
  now: "现在",
  reset: "重置",
  fullBy: (at) => `照最近一天的速度，${zhWeekday[(at.getDay() + 6) % 7]} ${clock(at)} 用满`,
  reachBy: (percent) => `照最近一天的速度，重置前用到 ${percent}%`,
  quotaUnread: "这一周还没读到额度：这个登录干过一轮之后才有",
  quotaAt: (at, week, five) => [`${zhWeekday[(at.getDay() + 6) % 7]} ${clock(at)}`, week !== null ? `7 天 ${week}%` : "", five !== null ? `5 小时 ${five}%` : ""].filter(Boolean).join("\n"),
  capTitle: (amount) => `这一周的上限 ${amount}`,
  capAt: (at, percent, amount) => `${zhWeekday[(at.getDay() + 6) % 7]} ${clock(at)}\n${amount}，上限的 ${percent}%`,
  weeksTitle: (weeks) => (weeks === 1 ? "这一周" : `近 ${weeks} 个重置周`),
  weeksNone: "还没有记下的重置周：这个登录读到 7 天额度之后才有",
  cap: (amount) => `上限 ${amount}`,
  punchTitle: "一天里的什么时候",
  punchSaid: (night) => `近 90 天，夜里 0–6 点占 ${night}%`,
  punchAt: (weekday, hour, amount) => `${weekday} ${hour}:00–${hour + 1}:00\n近 90 天 ${amount ?? "没有用"}`,
  where: "用在哪",
  byMoney: "按花的钱分",
  fromCache: (percent) => `输入里 ${percent}% 来自缓存`,
  kinds: { input: "输入", cache_read: "缓存读", cache_write: "缓存写", output: "输出", reasoning: "推理", fresh: "新输入", cached: "缓存输入" },
  models: "型号",
  thisWeekShort: "这一周",
  noneThisWeek: "这一周还没用",
  reading: "正在读",
  failed: "没读到",
  away: "这台机器现在不在线",
  notListed: "这个登录不在这台机器的已接入里了",
  workbenchOff: "打开工作台后才显示用量",
};

const enWeekday = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const enMonth = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

export const enUsage: UsageWords = {
  me: "Me",
  machineItself: "On the machine",
  aboutWhy: "Calculated by Claude Code itself: it prices each request's tokens with its built-in Anthropic API price list and adds them into the session's total; Fastctx reads that total from the official mod API's session.measure event and records each increase. A subscription login is not charged this amount.",
  money,
  tokens,
  tokenUnit: "tokens",
  newTalk: "New conversation",
  more: "More",
  fiveHours: "5 hours",
  sevenDays: "7 days",
  window: (minutes, name) => (minutes === 300 || name.startsWith("five_hour") ? "5 hours" : minutes === 10080 || name.startsWith("seven_day") ? "7 days" : name),
  resetsIn: (left) => `resets in ${left}`,
  notRead: "Not read yet",
  thisWeek: "This week",
  myWeek: "My week",
  whoseWeek: (name) => `${name}'s week`,
  weekSpan: (start, end) => `${enMonth[start.getMonth()]} ${start.getDate()} ${clock(start)} – ${enMonth[end.getMonth()]} ${end.getDate()} ${clock(end)}`,
  weekWhy: (window) => (window ? "The same week as this login's 7-day limit" : "The last 7 days: this login's 7-day limit is not read yet"),
  vsLastWeek: (change) => `${change > 0 ? "+" : ""}${change}% on last week`,
  noLastWeek: "Unused last week",
  today: (amount) => (amount ? `Today ${amount}` : "Unused today"),
  who: "Who uses it this week",
  assistUntil: (until, now) => (until ? `Until ${enMonth[until.getMonth()]} ${until.getDate()} ${clock(until)}${until.getFullYear() !== now.getFullYear() ? ` ${until.getFullYear()}` : ""}` : "No end"),
  untilShort: (until, now) => `${enMonth[until.getMonth()]} ${until.getDate()}${until.getFullYear() !== now.getFullYear() ? ` ${until.getFullYear()}` : ""}`,
  weekly: (amount) => `≈ ${amount} a week`,
  daily: (amount) => `≈ ${amount} a day`,
  fiveHourCap: (percent) => `Stops at ${percent}% of the 5-hour limit`,
  turnsCap: (turns) => `At most ${turns} turns at once`,
  noCap: "No allowance",
  aDay: "A day",
  aWeek: "A week",
  lastDay: "Last 24 hours",
  dayCapSaid: (cap, spent) => `≈ ${cap} a day at most\n≈ ${spent} in the last 24 hours`,
  weekCapSaid: (cap, spent) => `≈ ${cap} a week at most\n≈ ${spent} this week`,
  capRing: (name) => `${name}'s allowance`,
  fable: "Fable",
  fableCapSaid: (cap, spent) => `Fable gets half the weekly limit, ≈ ${cap}\n≈ ${spent} this week`,
  dayCap: "Daily limit",
  weekCap: "Weekly limit",
  spentWeek: (name, amount) => `${name}: ${amount} this week`,
  others: (count) => `${count} others`,
  fewer: "Show fewer",
  capOf: (spent, cap) => `${spent} / ${cap}`,
  partOf: (name, percent) => `${name} ≈ ${percent}%`,
  partSaid: (name, percent) => `${name} used ≈ ${percent}% of the account's limit this week`,
  partHead: "Of the limit",
  partsWhy: "The account's 7-day limit shared out by what each person used this week as Fastctx recorded it: Claude by Claude Code's own dollars, Codex by tokens, a cache read a tenth and output eight times (OpenAI's list-price ratios)",
  weekReached: (percent) => `The account's 7-day limit at ${percent}%`,
  tabs: { calendar: "Calendar", quota: "Limit", weeks: "Weeks", punch: "Hours", calls: "Requests" },
  usd: "$",
  tokenTab: "Tokens",
  year: "The past year",
  recentWeeks: (weeks) => `The last ${weeks} weeks`,
  less: "Less",
  most: "More",
  weekday: enWeekday,
  weekRow: "Wk",
  month: (month) => enMonth[month]!,
  day: (at) => `${enWeekday[(at.getDay() + 6) % 7]} ${enMonth[at.getMonth()]} ${at.getDate()}`,
  weekFrom: (at) => `Week of ${enMonth[at.getMonth()]} ${at.getDate()}`,
  unused: "Unused",
  yearTotal: (amount) => `A year ${amount}`,
  daysUsed: (days) => `Used on ${days} days`,
  longestRun: (days) => `Longest run ${days} days`,
  busiest: (day, amount) => `Busiest ${day} ${amount}`,
  quotaTitle: "This week's limit",
  now: "Now",
  reset: "Reset",
  fullBy: (at) => `At the last day's pace, full ${enWeekday[(at.getDay() + 6) % 7]} ${clock(at)}`,
  reachBy: (percent) => `At the last day's pace, ${percent}% by the reset`,
  quotaUnread: "No limit read this week yet: it comes once this login has done a turn",
  quotaAt: (at, week, five) => [`${enWeekday[(at.getDay() + 6) % 7]} ${clock(at)}`, week !== null ? `7 days ${week}%` : "", five !== null ? `5 hours ${five}%` : ""].filter(Boolean).join("\n"),
  capTitle: (amount) => `This week's allowance ${amount}`,
  capAt: (at, percent, amount) => `${enWeekday[(at.getDay() + 6) % 7]} ${clock(at)}\n${amount}, ${percent}% of the allowance`,
  weeksTitle: (weeks) => (weeks === 1 ? "This week" : `The last ${weeks} reset weeks`),
  weeksNone: "No reset week recorded yet: they come once this login's 7-day limit is read",
  cap: (amount) => `Allowance ${amount}`,
  punchTitle: "When in the day",
  punchSaid: (night) => `Last 90 days, ${night}% between midnight and 6`,
  punchAt: (weekday, hour, amount) => `${weekday} ${hour}:00–${hour + 1}:00\nLast 90 days ${amount ?? "unused"}`,
  where: "Where it went",
  byMoney: "By what it cost",
  fromCache: (percent) => `${percent}% of the input from the cache`,
  kinds: { input: "Input", cache_read: "Cache read", cache_write: "Cache write", output: "Output", reasoning: "Reasoning", fresh: "New input", cached: "Cached input" },
  models: "Models",
  thisWeekShort: "This week",
  noneThisWeek: "Unused this week",
  reading: "Reading",
  failed: "Not read",
  away: "This machine is offline",
  notListed: "This login is no longer hooked up on the machine",
  workbenchOff: "Usage shows once the workbench is on",
};
