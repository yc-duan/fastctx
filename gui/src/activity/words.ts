// What the activity page says (`world-gui.md`, 活动): the names of its
// views and spans, short states, and the hovers that carry the numbers.

import type { Outcome, TouchHow } from "../world/book";

export type View = "now" | "timeline" | "calendar" | "rhythm" | "spread" | "files" | "heed" | "sessions";
export type Span = "day" | "week" | "month" | "quarter";

export interface ActivityWords {
  title: string;
  views: Record<View, string>;
  spans: Record<Span, string>;
  outcome: Record<Outcome, string>;
  how: Record<TouchHow, string>;
  /** Nothing runs on any machine. */
  idle: string;
  /** What ended lately, under what runs now. */
  lately: string;
  /** Nothing happened in the span shown. */
  quiet: string;
  /** Machines that did not answer, in a hover. */
  unanswered(machines: string): string;
  /** A machine that did not answer, in its lane. */
  away: string;
  ran(duration: string): string;
  running(duration: string): string;
  duration(ms: number): string;
  count(n: number): string;
  files(n: number): string;
  failed(n: number): string;
  bytes(n: number): string;
  went(machines: string): string;
  /** The views' row labels. */
  byMachine: string;
  byAgent: string;
  byProject: string;
  byOutcome: string;
  noProject: string;
  agent: Record<"claude" | "codex" | "page" | "other", string>;
  /** A session the user opened on their own, and one of Fastctx's. */
  own: string;
  held: string;
  /** A file seen changed in its repository, in its hover. */
  seen: string;
  heed: { failed: string; stopped: string; lost: string; long: string; big: string; removed: string };
  /** The calendar and rhythm hovers. */
  day(date: string, runs: number, failed: number): string;
  hourOfWeek(weekday: string, hour: number, runs: number): string;
  weekday: string[];
  /** A day as the timeline and the hovers name it; a month's short name. */
  date(day: number): string;
  /** A moment in a list: the time today, yesterday's with its day, else
   *  the date. */
  stamp(at: number, now: number): string;
  month(month: number): string;
  /** The deed's card. */
  script: string;
  output: string;
  where: string;
  session: string;
  bringIn: string;
  broughtIn: string;
  bringInWhy: string;
  previousDay: string;
  nextDay: string;
  today: string;
}

const zhDuration = (ms: number) => {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  return minutes % 60 ? `${hours} 小时 ${minutes % 60} 分` : `${hours} 小时`;
};

const enDuration = (ms: number) => {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  return minutes % 60 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
};

const size = (n: number, units: string[]) => {
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 && unit ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
};

export const zhActivity: ActivityWords = {
  title: "活动",
  views: { now: "此刻", timeline: "时间线", calendar: "日历", rhythm: "节律", spread: "分布", files: "文件", heed: "留意", sessions: "会话" },
  spans: { day: "今天", week: "7 天", month: "30 天", quarter: "90 天" },
  outcome: { running: "在跑", done: "完成", failed: "失败", stopped: "停下", lost: "失去联系" },
  how: { added: "新建", changed: "改动", removed: "删除", copied: "复制到这里" },
  idle: "没有在跑的命令",
  lately: "刚才",
  quiet: "这段时间里没有活动",
  unanswered: (machines) => `没有回答：${machines}`,
  away: "不在线",
  ran: (duration) => `跑了 ${duration}`,
  running: (duration) => `已跑 ${duration}`,
  duration: zhDuration,
  count: (n) => `${n.toLocaleString("zh-CN")} 次`,
  files: (n) => `${n.toLocaleString("zh-CN")} 个文件`,
  failed: (n) => `${n.toLocaleString("zh-CN")} 次失败`,
  bytes: (n) => size(n, ["字节", "KB", "MB", "GB"]),
  went: (machines) => `去了 ${machines}`,
  byMachine: "机器",
  byAgent: "智能体",
  byProject: "项目",
  byOutcome: "结果",
  noProject: "不在项目里",
  agent: { claude: "Claude Code", codex: "Codex", page: "页面", other: "别处" },
  own: "你自己开的会话",
  held: "Fastctx 的对话",
  seen: "在仓库里看到它变了，行数相对上次提交",
  heed: { failed: "没跑成", stopped: "被停下", lost: "失去联系", long: "跑得久", big: "输出很大", removed: "删了文件" },
  day: (date, runs, failed) => (runs ? `${date}\n${runs} 次${failed ? `，${failed} 次失败` : ""}` : `${date}\n没有活动`),
  hourOfWeek: (weekday, hour, runs) => `${weekday} ${hour}:00–${hour + 1}:00\n近 90 天 ${runs ? `${runs} 次` : "没有活动"}`,
  weekday: ["一", "二", "三", "四", "五", "六", "日"],
  date: (day) => {
    const at = new Date(day);
    return `${at.getMonth() + 1}月${at.getDate()}日 周${"日一二三四五六"[at.getDay()]}`;
  },
  month: (month) => `${month + 1}月`,
  stamp: (at, now) => {
    const date = new Date(at);
    const clock = `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
    const days = Math.round((new Date(now).setHours(0, 0, 0, 0) - new Date(at).setHours(0, 0, 0, 0)) / 86_400_000);
    if (days <= 0) return clock;
    if (days === 1) return `昨天 ${clock}`;
    return new Date(now).getFullYear() === date.getFullYear() ? `${date.getMonth() + 1}月${date.getDate()}日` : `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
  },
  script: "命令",
  output: "输出",
  where: "在哪",
  session: "会话",
  bringIn: "接进来",
  broughtIn: "已接进来",
  bringInWhy: "从此作为 Fastctx 的对话，在侧栏里接着",
  previousDay: "前一天",
  nextDay: "后一天",
  today: "今天",
};

export const enActivity: ActivityWords = {
  title: "Activity",
  views: { now: "Now", timeline: "Timeline", calendar: "Calendar", rhythm: "Rhythm", spread: "Spread", files: "Files", heed: "Heed", sessions: "Sessions" },
  spans: { day: "Today", week: "7 days", month: "30 days", quarter: "90 days" },
  outcome: { running: "Running", done: "Done", failed: "Failed", stopped: "Stopped", lost: "Lost" },
  how: { added: "Added", changed: "Changed", removed: "Removed", copied: "Copied here" },
  idle: "Nothing running",
  lately: "Lately",
  quiet: "Nothing in this span",
  unanswered: (machines) => `No answer: ${machines}`,
  away: "Offline",
  ran: (duration) => `Ran ${duration}`,
  running: (duration) => `Running ${duration}`,
  duration: enDuration,
  count: (n) => (n === 1 ? "1 run" : `${n.toLocaleString("en")} runs`),
  files: (n) => (n === 1 ? "1 file" : `${n.toLocaleString("en")} files`),
  failed: (n) => `${n.toLocaleString("en")} failed`,
  bytes: (n) => size(n, ["B", "KB", "MB", "GB"]),
  went: (machines) => `Went to ${machines}`,
  byMachine: "Machines",
  byAgent: "Agents",
  byProject: "Projects",
  byOutcome: "Outcomes",
  noProject: "No project",
  agent: { claude: "Claude Code", codex: "Codex", page: "Pages", other: "Elsewhere" },
  own: "A session you opened",
  held: "A Fastctx conversation",
  seen: "Seen changed in its repository; lines against the last commit",
  heed: { failed: "Failed", stopped: "Stopped", lost: "Lost", long: "Ran long", big: "Large output", removed: "Removed files" },
  day: (date, runs, failed) => (runs ? `${date}\n${runs} runs${failed ? `, ${failed} failed` : ""}` : `${date}\nNo activity`),
  hourOfWeek: (weekday, hour, runs) => `${weekday} ${hour}:00–${hour + 1}:00\nLast 90 days: ${runs ? `${runs} runs` : "none"}`,
  weekday: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
  date: (day) => new Date(day).toLocaleDateString("en", { weekday: "short", month: "short", day: "numeric" }),
  month: (month) => ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][month]!,
  stamp: (at, now) => {
    const date = new Date(at);
    const clock = `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
    const days = Math.round((new Date(now).setHours(0, 0, 0, 0) - new Date(at).setHours(0, 0, 0, 0)) / 86_400_000);
    if (days <= 0) return clock;
    if (days === 1) return `Yesterday ${clock}`;
    return date.toLocaleDateString("en", { month: "short", day: "numeric", ...(new Date(now).getFullYear() === date.getFullYear() ? {} : { year: "numeric" }) });
  },
  script: "Command",
  output: "Output",
  where: "Where",
  session: "Session",
  bringIn: "Bring in",
  broughtIn: "Brought in",
  bringInWhy: "Carries on as a Fastctx conversation, in the sidebar",
  previousDay: "Previous day",
  nextDay: "Next day",
  today: "Today",
};
