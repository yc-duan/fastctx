// What the activity page says (`gui-activity-page`): the files changed, in
// the words a list of changed files is read in everywhere.

import type { TouchHow } from "../world/book";

export type Span = "day" | "week" | "month" | "quarter";

export interface ActivityWords {
  title: string;
  spans: Record<Span, string>;
  spanLabel: string;
  /** How many files changed in the span, over the list. */
  changed(files: number): string;
  /** A machine's files, beside its name. */
  files(n: number): string;
  /** Nothing was changed in the span shown. */
  quiet: string;
  how: Record<TouchHow, string>;
  /** A file seen changed in its repository, rather than written by
   *  Fastctx's own commands: its lines are counted against the last commit. */
  seen: string;
  /** Whose session changed it, where it has no name of its own. */
  own(agent: string): string;
  /** A session of another machine, beside who changed it. */
  from(machine: string): string;
  /** The changes of an opened file not listed, earlier than those that are. */
  earlier(n: number): string;
  /** Machines that did not answer. */
  unanswered(machines: string): string;
  /** More went on in the span than a machine's answer holds. */
  truncated(limit: number): string;
  /** A moment in a list: the time today, yesterday's with its day, else
   *  the date. */
  stamp(at: number, now: number): string;
}

const clock = (date: Date) => `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
const daysBefore = (at: number, now: number) => Math.round((new Date(now).setHours(0, 0, 0, 0) - new Date(at).setHours(0, 0, 0, 0)) / 86_400_000);

export const zhActivity: ActivityWords = {
  title: "活动",
  spans: { day: "今天", week: "7 天", month: "30 天", quarter: "90 天" },
  spanLabel: "时间范围",
  changed: (files) => `${files} 个文件有改动`,
  files: (n) => `${n} 个文件`,
  quiet: "此时段没有文件改动",
  how: { added: "新建", changed: "改动", removed: "删除", copied: "复制到这里" },
  seen: "仓库中有变更，行数相对上次提交",
  own: (agent) => `${agent} 会话`,
  from: (machine) => `来自 ${machine}`,
  earlier: (n) => `更早的 ${n} 次未列出`,
  unanswered: (machines) => `无应答：${machines}`,
  truncated: (limit) => `活动较多，每台电脑仅统计最近 ${limit.toLocaleString("en-US")} 条`,
  stamp: (at, now) => {
    const date = new Date(at);
    const days = daysBefore(at, now);
    if (days <= 0) return clock(date);
    if (days === 1) return `昨天 ${clock(date)}`;
    return `${new Date(now).getFullYear() === date.getFullYear() ? "" : `${date.getFullYear()}年`}${date.getMonth() + 1}月${date.getDate()}日 ${clock(date)}`;
  },
};

export const enActivity: ActivityWords = {
  title: "Activity",
  spans: { day: "Today", week: "7 days", month: "30 days", quarter: "90 days" },
  spanLabel: "Span",
  changed: (files) => `${files} changed ${files === 1 ? "file" : "files"}`,
  files: (n) => `${n} ${n === 1 ? "file" : "files"}`,
  quiet: "No file was changed in this span",
  how: { added: "Added", changed: "Changed", removed: "Removed", copied: "Copied here" },
  seen: "Seen changed in its repository; lines are counted against the last commit",
  own: (agent) => `A ${agent} session`,
  from: (machine) => `from ${machine}`,
  earlier: (n) => `${n} earlier ${n === 1 ? "change is" : "changes are"} not listed`,
  unanswered: (machines) => `No answer from ${machines}`,
  truncated: (limit) => `Much went on: only the last ${limit.toLocaleString("en-US")} runs of each computer are counted`,
  stamp: (at, now) => {
    const date = new Date(at);
    const days = daysBefore(at, now);
    if (days <= 0) return clock(date);
    if (days === 1) return `Yesterday ${clock(date)}`;
    return `${date.toLocaleDateString("en", { month: "short", day: "numeric", ...(new Date(now).getFullYear() === date.getFullYear() ? {} : { year: "numeric" }) })} ${clock(date)}`;
  },
};
