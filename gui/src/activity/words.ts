// What the activity page says of itself (`gui-activity-page`); the words of
// the files it lists are the files' own (`files/words.ts`).

export type Span = "day" | "week" | "month" | "quarter";
/** What the files stand under: the conversation that changed them, or the
 *  machine holding them. */
export type Grouping = "talk" | "machine";

export interface ActivityWords {
  title: string;
  spans: Record<Span, string>;
  spanLabel: string;
  groupings: Record<Grouping, string>;
  groupingLabel: string;
  /** Nothing was changed in the span shown. */
  quiet: string;
  /** Groups not listed until asked for. */
  moreGroups(n: number): string;
  /** A machine's name leads to its folders. */
  folders: string;
  openFile: string;
  allChanges: string;
  /** Machines that did not answer. */
  unanswered(machines: string): string;
  /** More went on in the span than a machine's answer holds. */
  truncated(limit: number): string;
}

export const zhActivity: ActivityWords = {
  title: "活动",
  spans: { day: "今天", week: "7 天", month: "30 天", quarter: "90 天" },
  spanLabel: "时间范围",
  groupings: { talk: "按对话", machine: "按电脑" },
  groupingLabel: "分组方式",
  quiet: "此时段没有文件改动",
  moreGroups: (n) => `还有 ${n} 组`,
  folders: "查看这台电脑的目录",
  openFile: "打开文件",
  allChanges: "全部改动",
  unanswered: (machines) => `无应答：${machines}`,
  truncated: (limit) => `活动较多，每台电脑仅统计最近 ${limit.toLocaleString("en-US")} 条`,
};

export const enActivity: ActivityWords = {
  title: "Activity",
  spans: { day: "Today", week: "7 days", month: "30 days", quarter: "90 days" },
  spanLabel: "Span",
  groupings: { talk: "By conversation", machine: "By computer" },
  groupingLabel: "Grouped by",
  quiet: "No file was changed in this span",
  moreGroups: (n) => `${n} more`,
  folders: "This computer's folders",
  openFile: "Open the file",
  allChanges: "All its changes",
  unanswered: (machines) => `No answer from ${machines}`,
  truncated: (limit) => `Much went on: only the last ${limit.toLocaleString("en-US")} runs of each computer are counted`,
};
