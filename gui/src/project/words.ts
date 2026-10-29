// What a project's pages say, in the user's words (`world-gui.md`, 用词):
// a project's front is its overview; journals are written along the way
// and never changed; decisions are the canon the user confirmed.

import { enProblems, failure, zhProblems } from "../app/problem";

export interface ProjectWords {
  overview: string;
  nobodyHere: string;
  journals: string;
  noJournals: string;
  decisions: string;
  noDecisions: string;
  decisionsUnreadable(why: string): string;
  places: string;
  noPlaces: string;
  /** What the user wrote for every agent working in the project. */
  instructions: string;
  noInstructions: string;
  /** What each mark of what waits for the user means, where the pointer
   *  rests on it. */
  needAsked: string;
  needUrgent: string;
  needInfo: string;
  /** A conversation's way, on its drawing: its stops in order. */
  way(stops: string[]): string;
  /** A sense on a conversation's line: what it watches, where, how it
   *  stands and what it saw last. */
  sense(condition: string, machine: string, phase: string, seen: string): string;
  /** A colleague on a conversation's line. */
  colleague(label: string, who: string, doing: string): string;
  /** A machine on the ground: what stands on it. */
  ground(agents: number, senses: number, jobs: number): string;
  /** The page's two views: the work now, and what the project keeps. */
  nowTab: string;
  referenceTab: string;
  opened(by: string, on: string, day: string): string;
  you: string;
  /** An open page, as who wrote an entry. */
  aPage(name: string): string;
  entries(n: number): string;
  writers(n: number): string;
  latest(by: string, ago: string): string;
  shown(first: number, last: number, total: number): string;
  earlier: string;
  selfReported: string;
  mentionedBy: string;
  cites: string;
  attachments: string;
  onMachine(machine: string): string;
  edited(by: string, day: string): string;
  day(at: Date): string;
  clock(at: Date): string;
  write: string;
  newJournal: string;
  writeTitle(journal: string): string;
  newJournalTitle: string;
  journalName: string;
  journalNameHint: string;
  description: string;
  descriptionHint: string;
  entry: string;
  entryHint: string;
  save: string;
  saving: string;
  close: string;
  failed(why: string): string;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** A name followed by Chinese: a space after a Latin name, none after a
 *  Chinese one. */
const then = (name: string) => (/[A-Za-z0-9)]$/.test(name) ? `${name} ` : name);

export const zhProject: ProjectWords = {
  overview: "概览",
  nobodyHere: "暂无对话",
  journals: "手记",
  noJournals: "暂无手记",
  decisions: "定论",
  noDecisions: "未设置定论位置",
  decisionsUnreadable: failure(zhProblems, "无法读取定论"),
  places: "位置",
  noPlaces: "暂无位置",
  instructions: "须知",
  noInstructions: "还没有写须知",
  nowTab: "此刻",
  referenceTab: "资料",
  needAsked: "在问你",
  needUrgent: "要紧",
  needInfo: "说了一句",
  way: (stops) => stops.join(" → "),
  sense: (condition, machine, phase, seen) => [condition, `${machine}　${phase}`, seen].filter(Boolean).join("\n"),
  colleague: (label, who, doing) => [label, `${who}　${doing}`].filter(Boolean).join("\n"),
  ground: (agents, senses, jobs) =>
    [agents ? `${agents} 段对话` : "", senses ? `${senses} 项观测` : "", jobs ? `${jobs} 件作业` : ""].filter(Boolean).join("　") || "这里暂时没有在做的事",
  opened: (by, on, day) => `${then(by)}创建于 ${on}，${day}`,
  you: "你",
  aPage: (name) => `页面「${name}」`,
  entries: (n) => `${n} 条`,
  writers: (n) => `${n} 位作者`,
  latest: (by, ago) => `最近 ${by}　${ago}`,
  shown: (first, last, total) => `第 ${first}–${last} 条，共 ${total} 条`,
  earlier: "更早",
  selfReported: "自报",
  mentionedBy: "被引用",
  cites: "回应",
  attachments: "附件",
  onMachine: (machine) => machine,
  edited: (by, day) => `${then(by)}编辑于 ${day}`,
  day: (at) => `${at.getFullYear()}年${at.getMonth() + 1}月${at.getDate()}日`,
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
  write: "添加条目",
  newJournal: "新建手记",
  writeTitle: (journal) => `添加到「${journal}」`,
  newJournalTitle: "新建手记",
  journalName: "主题名",
  journalNameHint: "小写英文词加连字符，例如 webrtc-weak-network",
  description: "范围",
  descriptionHint: "这份手记记录什么",
  entry: "内容",
  entryHint: "提交后不可修改，如有错误请追加更正",
  save: "提交",
  saving: "提交中…",
  close: "关闭",
  failed: failure(zhProblems, "提交失败"),
};

export const enProject: ProjectWords = {
  overview: "Overview",
  nobodyHere: "No conversations",
  journals: "Journals",
  noJournals: "No journals yet",
  decisions: "Decisions",
  noDecisions: "No decisions folder set",
  decisionsUnreadable: failure(enProblems, "Couldn't read decisions"),
  places: "Locations",
  noPlaces: "No locations yet",
  instructions: "Instructions",
  noInstructions: "No instructions yet",
  nowTab: "Now",
  referenceTab: "Reference",
  needAsked: "Asks you",
  needUrgent: "Urgent",
  needInfo: "Said something",
  way: (stops) => stops.join(" → "),
  sense: (condition, machine, phase, seen) => [condition, `${machine}, ${phase}`, seen].filter(Boolean).join("\n"),
  colleague: (label, who, doing) => [label, `${who}, ${doing}`].filter(Boolean).join("\n"),
  ground: (agents, senses, jobs) =>
    [agents ? `${agents} conversation${agents === 1 ? "" : "s"}` : "", senses ? `${senses} watcher${senses === 1 ? "" : "s"}` : "", jobs ? `${jobs} job${jobs === 1 ? "" : "s"}` : ""]
      .filter(Boolean)
      .join(", ") || "Nothing running here",
  opened: (by, on, day) => `Created by ${by} on ${on}, ${day}`,
  you: "you",
  aPage: (name) => `the page “${name}”`,
  entries: (n) => `${n} entr${n === 1 ? "y" : "ies"}`,
  writers: (n) => `${n} writer${n === 1 ? "" : "s"}`,
  latest: (by, ago) => `latest ${by}, ${ago}`,
  shown: (first, last, total) => `Entries ${first}–${last} of ${total}`,
  earlier: "Earlier",
  selfReported: "self-reported",
  mentionedBy: "Mentioned by",
  cites: "Answers",
  attachments: "Attachments",
  onMachine: (machine) => machine,
  edited: (by, day) => `edited by ${by}, ${day}`,
  day: (at) => at.toLocaleDateString("en", { year: "numeric", month: "long", day: "numeric" }),
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
  write: "Add entry",
  newJournal: "New journal",
  writeTitle: (journal) => `Add to “${journal}”`,
  newJournalTitle: "New journal",
  journalName: "Subject",
  journalNameHint: "Lowercase words joined by hyphens, e.g. webrtc-weak-network",
  description: "Scope",
  descriptionHint: "What this journal records",
  entry: "Entry",
  entryHint: "Entries can't be edited; add a correction if needed",
  save: "Submit",
  saving: "Submitting…",
  close: "Close",
  failed: failure(enProblems, "Couldn't submit"),
};
