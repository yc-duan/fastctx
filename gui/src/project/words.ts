// What a project's pages say, in the user's words (`world-gui.md`, 用词):
// a project's front is its overview; journals are written along the way
// and never changed; decisions are the canon the user confirmed.

export interface ProjectWords {
  overview: string;
  here: string;
  nobodyHere: string;
  journals: string;
  noJournals: string;
  decisions: string;
  noDecisions: string;
  decisionsUnreadable(why: string): string;
  places: string;
  what: string;
  machine: string;
  path: string;
  opened(by: string, on: string, day: string): string;
  you: string;
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
}

const pad = (n: number) => String(n).padStart(2, "0");

/** A name followed by Chinese: a space after a Latin name, none after a
 *  Chinese one. */
const then = (name: string) => (/[A-Za-z0-9)]$/.test(name) ? `${name} ` : name);

export const zhProject: ProjectWords = {
  overview: "概览",
  here: "此刻在这里",
  nobodyHere: "此刻没有谁在这个项目里。",
  journals: "手记",
  noJournals: "还没有手记。",
  decisions: "定论",
  noDecisions: "还没有写下定论的地方。",
  decisionsUnreadable: (why) => `定论此刻读不到：${why}`,
  places: "在哪里能找到什么",
  what: "是什么",
  machine: "机器",
  path: "路径",
  opened: (by, on, day) => `${then(by)}在 ${on} 上开启 · ${day}`,
  you: "你",
  entries: (n) => `${n} 条`,
  writers: (n) => `${n} 位写过`,
  latest: (by, ago) => `最近一条：${by} · ${ago}`,
  shown: (first, last, total) => `第 ${first}–${last} 条，共 ${total} 条`,
  earlier: "更早的条目",
  selfReported: "自报",
  mentionedBy: "后来提到它的",
  cites: "回应",
  attachments: "附件",
  onMachine: (machine) => `在 ${machine}`,
  edited: (by, day) => `${then(by)}改于 ${day}`,
  day: (at) => `${at.getFullYear()}年${at.getMonth() + 1}月${at.getDate()}日`,
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
};

export const enProject: ProjectWords = {
  overview: "Overview",
  here: "Here now",
  nobodyHere: "Nobody is in this project right now.",
  journals: "Journals",
  noJournals: "No journals yet.",
  decisions: "Decisions",
  noDecisions: "No place for decisions has been written down.",
  decisionsUnreadable: (why) => `The decisions cannot be read now: ${why}`,
  places: "Where things are",
  what: "What",
  machine: "Machine",
  path: "Path",
  opened: (by, on, day) => `Opened by ${by} on ${on}, ${day}`,
  you: "you",
  entries: (n) => `${n} entr${n === 1 ? "y" : "ies"}`,
  writers: (n) => `${n} writer${n === 1 ? "" : "s"}`,
  latest: (by, ago) => `latest: ${by}, ${ago}`,
  shown: (first, last, total) => `Entries ${first}–${last} of ${total}`,
  earlier: "Earlier entries",
  selfReported: "self-reported",
  mentionedBy: "Mentioned by",
  cites: "Answers",
  attachments: "Attachments",
  onMachine: (machine) => `on ${machine}`,
  edited: (by, day) => `edited by ${by}, ${day}`,
  day: (at) => at.toLocaleDateString("en", { year: "numeric", month: "long", day: "numeric" }),
  clock: (at) => `${pad(at.getHours())}:${pad(at.getMinutes())}`,
};
