// What a project's pages say, in the user's words (`world-gui.md`, 用词):
// a project's front is its overview; journals are written along the way
// and never changed; decisions are the canon the user confirmed.

export interface ProjectWords {
  overview: string;
  here: string;
  nobodyHere: string;
  running: string;
  ownerLeft: string;
  journals: string;
  noJournals: string;
  decisions: string;
  noDecisions: string;
  decisionsUnreadable(why: string): string;
  places: string;
  noPlaces: string;
  /** Shorter names for the tabs a narrow screen shows the sections behind. */
  hereTab: string;
  placesTab: string;
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
  here: "此刻在这里",
  nobodyHere: "此刻没有谁在这个项目里。",
  running: "还在跑的",
  ownerLeft: "起它的已经离开",
  journals: "手记",
  noJournals: "还没有手记。",
  decisions: "定论",
  noDecisions: "还没有写下定论的地方。",
  decisionsUnreadable: (why) => `定论此刻读不到：${why}`,
  places: "在哪里能找到什么",
  noPlaces: "地址簿还是空的。",
  hereTab: "此刻",
  placesTab: "地址簿",
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
  write: "写一条",
  newJournal: "新手记",
  writeTitle: (journal) => `在「${journal}」里写一条`,
  newJournalTitle: "开一份新手记",
  journalName: "主题名",
  journalNameHint: "用连字符连起来的小写英文词，给一个东西起名，不下结论，比如 webrtc-weak-network",
  description: "它记的是什么",
  descriptionHint: "一两句话说它的范围，而不是它记出了什么",
  entry: "这一条",
  entryHint: "写下就不能再改；写错了，再补一条更正。你写的就是原话。",
  save: "写下",
  saving: "正在写…",
  close: "关闭",
  failed: (why) => `没能写下：${why}`,
};

export const enProject: ProjectWords = {
  overview: "Overview",
  here: "Here now",
  nobodyHere: "Nobody is in this project right now.",
  running: "Still running",
  ownerLeft: "whoever started it has left",
  journals: "Journals",
  noJournals: "No journals yet.",
  decisions: "Decisions",
  noDecisions: "No place for decisions has been written down.",
  decisionsUnreadable: (why) => `The decisions cannot be read now: ${why}`,
  places: "Where things are",
  noPlaces: "The address book is empty.",
  hereTab: "Now",
  placesTab: "Places",
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
  write: "Write an entry",
  newJournal: "New journal",
  writeTitle: (journal) => `An entry in “${journal}”`,
  newJournalTitle: "Start a journal",
  journalName: "Subject",
  journalNameHint: "lowercase English words joined by hyphens, naming a thing, never a conclusion, such as webrtc-weak-network",
  description: "What it covers",
  descriptionHint: "a sentence or two on its scope, not on what it found",
  entry: "The entry",
  entryHint: "Once written it cannot be changed; to correct it, add another. What you write is quoted as your own words.",
  save: "Write it",
  saving: "Writing…",
  close: "Close",
  failed: (why) => `Could not write it: ${why}`,
};
