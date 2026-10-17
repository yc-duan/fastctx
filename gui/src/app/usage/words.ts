// What the usage of an agent's login says (`world-agent-logins.md`, 用量):
// amounts as short as a chart's labels, Claude Code's dollars always marked
// as an estimate, and the hovers that carry the rest.

import { dollars, underCent } from "../../world/money";

export interface UsageWords {
  me: string;
  /** Whoever uses the machine itself, where no owner is known. */
  machineItself: string;
  /** "约": an amount Claude Code reckoned at list prices. */
  about: string;
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
  /** The week's span, and what it follows. */
  weekSpan(start: Date, end: Date): string;
  weekWhy(window: boolean): string;
  vsLastWeek(change: number): string;
  noLastWeek: string;
  today(amount: string | null): string;
  who: string;
  all: string;
  assistedBy(owner: string): string;
  assistUntil(until: Date | null, now: Date): string;
  weekly(amount: string): string;
  weeklyUsed(spent: string, of: string): string;
  weeklyNone: string;
  onlyYours: string;
  /** The tabs and the unit. */
  tabs: { calendar: string; quota: string; daily: string; punch: string };
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
  /** The days. */
  ranges: { week: string; days30: string; days90: string };
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
  about: "约",
  aboutWhy: "按 API 标价折算。订阅登录并没有花这笔钱；按 Esc 打断的那次请求 Claude Code 不记，所以只会偏少。",
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
  weekSpan: (start, end) => `${start.getMonth() + 1}月${start.getDate()}日 ${clock(start)} – ${end.getMonth() + 1}月${end.getDate()}日 ${clock(end)}`,
  weekWhy: (window) => (window ? "和这个登录的 7 天额度同一周" : "最近 7 天：还没读到这个登录的 7 天额度"),
  vsLastWeek: (change) => `比上一周 ${change > 0 ? "+" : ""}${change}%`,
  noLastWeek: "上一周没有用",
  today: (amount) => (amount ? `今天 ${amount}` : "今天还没用"),
  who: "这一周谁在用",
  all: "看全部",
  assistedBy: (owner) => `来自 ${owner} 的协助`,
  assistUntil: (until, now) => (until ? `到 ${until.getMonth() + 1}月${until.getDate()}日 ${clock(until)}${until.getFullYear() !== now.getFullYear() ? ` ${until.getFullYear()}` : ""}` : "不限期"),
  weekly: (amount) => `每周 约 ${amount}`,
  weeklyUsed: (spent, of) => `这一周用了 约 ${spent} / ${of}`,
  weeklyNone: "每周不限",
  onlyYours: "你只看得到自己用了多少",
  tabs: { calendar: "日历", quota: "额度", daily: "每天", punch: "时段" },
  usd: "约 $",
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
  ranges: { week: "这一周", days30: "30 天", days90: "90 天" },
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
  about: "≈",
  aboutWhy: "Reckoned at the API's list prices. A subscription login is not charged this; a request stopped with Esc is not counted, so it can only read low.",
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
  weekSpan: (start, end) => `${enMonth[start.getMonth()]} ${start.getDate()} ${clock(start)} – ${enMonth[end.getMonth()]} ${end.getDate()} ${clock(end)}`,
  weekWhy: (window) => (window ? "The same week as this login's 7-day limit" : "The last 7 days: this login's 7-day limit is not read yet"),
  vsLastWeek: (change) => `${change > 0 ? "+" : ""}${change}% on last week`,
  noLastWeek: "Unused last week",
  today: (amount) => (amount ? `Today ${amount}` : "Unused today"),
  who: "Who uses it this week",
  all: "Show all",
  assistedBy: (owner) => `Assisting you, from ${owner}`,
  assistUntil: (until, now) => (until ? `Until ${enMonth[until.getMonth()]} ${until.getDate()} ${clock(until)}${until.getFullYear() !== now.getFullYear() ? ` ${until.getFullYear()}` : ""}` : "No end"),
  weekly: (amount) => `≈ ${amount} a week`,
  weeklyUsed: (spent, of) => `≈ ${spent} of ${of} used this week`,
  weeklyNone: "No weekly limit",
  onlyYours: "You see only your own use",
  tabs: { calendar: "Calendar", quota: "Limit", daily: "Days", punch: "Hours" },
  usd: "≈ $",
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
  ranges: { week: "This week", days30: "30 days", days90: "90 days" },
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
