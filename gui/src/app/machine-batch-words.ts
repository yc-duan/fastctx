// The words of machines chosen together (`machine-framed.ts`,
// `machine-ways.ts`): what is done to how many of them, choosing them, what
// several run as the window of versions says it, and setting how they update.

import { enProblems, failure, zhProblems } from "./problem";

export interface BatchWords {
  /** An action that does not reach every machine chosen, with how many it reaches. */
  some(label: string, count: number): string;
  share: string;
  group: string;
  update: string;
  remove: string;
  clear: string;
  /** Choosing several on a touch screen: every machine, or none again;
   *  and the words of the bar along the bottom, one short word each. */
  selectAll: string;
  selectNone: string;
  bar: { share: string; group: string; update: string; remove: string };
  /** A row's box in the list, a heading's, and choosing a box's machines
   *  from its "⋯". */
  pickOne(name: string): string;
  pickAll(label: string): string;
  pickGroup: string;

  /** What several run, as the window of versions says it. */
  allRun(count: number, version: string): string;
  oldestRuns(count: number, version: string): string;

  /** Setting how several update. */
  waysTitle(count: number): string;
  /** Setting how they update did not take: a known reason in plain words,
   *  else that it did not. */
  waysFailed(why: string): string;
  auto: string;
  relays: string;
  on: string;
  off: string;
  keep: string;
  apply(count: number): string;
  set: string;
}

export const zhBatch: BatchWords = {
  some: (label, count) => `${label}（${count} 台）`,
  share: "共享…",
  group: "分组",
  update: "更新…",
  remove: "移除…",
  clear: "取消选择",
  selectAll: "全选",
  selectNone: "取消全选",
  bar: { share: "共享", group: "分组", update: "更新", remove: "移除" },
  pickOne: (name) => `选中 ${name}`,
  pickAll: (label) => `选中 ${label} 的全部机器`,
  pickGroup: "选中此组",

  allRun: (count, version) => `${count} 台均运行 ${version}`,
  oldestRuns: (count, version) => `${count} 台中最旧的运行 ${version}`,

  waysTitle: (count) => `${count} 台的更新方式`,
  waysFailed: failure(zhProblems, "更改失败"),
  auto: "自动更新",
  relays: "转发安装包",
  on: "开",
  off: "关",
  keep: "保持",
  apply: (count) => `应用到 ${count} 台`,
  set: "已设置",
};

export const enBatch: BatchWords = {
  some: (label, count) => `${label} (${count})`,
  share: "Share…",
  group: "Group",
  update: "Update…",
  remove: "Remove…",
  clear: "Clear",
  selectAll: "Select all",
  selectNone: "Select none",
  bar: { share: "Share", group: "Group", update: "Update", remove: "Remove" },
  pickOne: (name) => `Choose ${name}`,
  pickAll: (label) => `Choose every machine in ${label}`,
  pickGroup: "Choose this group",

  allRun: (count, version) => `All ${count} run ${version}`,
  oldestRuns: (count, version) => `The oldest of these ${count} runs ${version}`,

  waysTitle: (count) => `How these ${count} update`,
  waysFailed: failure(enProblems, "Not changed"),
  auto: "Automatic updates",
  relays: "Pass packages on",
  on: "On",
  off: "Off",
  keep: "Leave as is",
  apply: (count) => `Apply to ${count}`,
  set: "Set",
};
