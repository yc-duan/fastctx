// The words of a login's request list (`calls.ts`; `world-usage.md`, 调用记录):
// its filters, the day each group of rows is from, the short marks a row
// carries and the sentences behind them.

import type { CallOutcome, CallPhase, CallSource } from "../../world/usage-calls";

export interface CallsWords {
  /** The panel's title, and the chip of one turn's requests. */
  title: string;
  turn: string;
  turnSaid: string;
  allTurns: string;
  /** The filters: their names unset, and what each can be set to. */
  model: string;
  allModels: string;
  source: string;
  sources: Record<CallSource | "all", string>;
  outcome: string;
  outcomes: Record<CallOutcome | "all", string>;
  /** A row's short marks. */
  agentTag: string;
  background: string;
  /** The days rows are grouped by. */
  today: string;
  yesterday: string;
  /** Under the last row, where the list ends. */
  kept: string;
  none: string;
  noneFiltered: string;
  /** What a row says while the pointer rests on it. */
  at(at: Date): string;
  backgroundSaid: string;
  agentSaid: string;
  took(ms: number): string;
  first(ms: number): string;
  roughly: string;
  outcomeSaid: Record<Exclude<CallOutcome, "completed">, string>;
  phase: Record<Exclude<CallPhase, "finished" | "unknown">, string>;
  open: string;
  others: string;
  /** A short duration, as a row's figure. */
  short(ms: number): string;
  /** Between two things said on one line. */
  and: string;
}

const zhWeekday = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

const two = (value: number) => String(value).padStart(2, "0");

/** A short duration: tenths under ten seconds, whole seconds under a
 *  minute, minutes and seconds past it. */
function short(ms: number): string {
  const seconds = ms / 1000;
  if (seconds < 10) return `${Math.max(0.1, Math.round(seconds * 10) / 10).toFixed(1)}s`;
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const whole = Math.round(seconds);
  return `${Math.floor(whole / 60)}m ${two(whole % 60)}s`;
}

export const zhCalls: CallsWords = {
  title: "最近 30 天",
  turn: "这一轮",
  turnSaid: "只看对话里这一轮的请求",
  allTurns: "看全部",
  model: "型号",
  allModels: "所有型号",
  source: "来源",
  sources: { all: "全部", main: "主循环", agent: "子代理", background: "Claude Code 后台" },
  outcome: "结果",
  outcomes: { all: "全部", completed: "完成", failed: "出错", interrupted: "中断", held: "拦下", unknown: "不明" },
  agentTag: "子代理",
  background: "后台",
  today: "今天",
  yesterday: "昨天",
  kept: "逐个请求只留最近 30 天",
  none: "最近 30 天没有请求",
  noneFiltered: "没有这样的请求",
  at: (at) => `${at.getMonth() + 1}月${at.getDate()}日 ${zhWeekday[(at.getDay() + 6) % 7]} ${at.getHours()}:${two(at.getMinutes())}:${two(at.getSeconds())}`,
  backgroundSaid: "Claude Code 自己的后台请求（压缩、起标题、处理网页），看不到 token 与型号",
  agentSaid: "子代理的请求",
  took: (ms) => `用时 ${short(ms)}`,
  first: (ms) => `首个片段 ${short(ms)}`,
  roughly: "Codex 只给得出大概的时刻",
  outcomeSaid: { failed: "出错了", interrupted: "中断了", held: "到了上限，没有发出", unknown: "没有下文" },
  phase: { before_response: "停在等回应时", streaming: "停在回应途中" },
  open: "打开这一轮",
  others: "一段对话",
  short,
  and: "，",
};

const enWeekday = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const enMonth = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

export const enCalls: CallsWords = {
  title: "Last 30 days",
  turn: "This turn",
  turnSaid: "Only this turn's requests",
  allTurns: "Show all",
  model: "Model",
  allModels: "All models",
  source: "Source",
  sources: { all: "All", main: "Main loop", agent: "Subagents", background: "Claude Code background" },
  outcome: "Outcome",
  outcomes: { all: "All", completed: "Completed", failed: "Failed", interrupted: "Interrupted", held: "Held", unknown: "Unknown" },
  agentTag: "Subagent",
  background: "Background",
  today: "Today",
  yesterday: "Yesterday",
  kept: "Requests are kept one by one for 30 days",
  none: "No requests in the last 30 days",
  noneFiltered: "No such requests",
  at: (at) => `${enWeekday[(at.getDay() + 6) % 7]} ${enMonth[at.getMonth()]} ${at.getDate()}, ${at.getHours()}:${two(at.getMinutes())}:${two(at.getSeconds())}`,
  backgroundSaid: "Claude Code's own background request (compacting, titles, web pages); its tokens and model aren't visible",
  agentSaid: "A subagent's request",
  took: (ms) => `Took ${short(ms)}`,
  first: (ms) => `First chunk ${short(ms)}`,
  roughly: "Codex gives only rough times",
  outcomeSaid: { failed: "Failed", interrupted: "Interrupted", held: "Held at an allowance, never sent", unknown: "Nothing more was heard" },
  phase: { before_response: "Stopped waiting for a response", streaming: "Stopped mid-response" },
  open: "Open this turn",
  others: "A conversation",
  short,
  and: ", ",
};
