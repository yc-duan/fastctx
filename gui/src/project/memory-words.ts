// The words of a project's memory: its journals and decisions on the
// project's page, a document read, a decision edited, a journal written.

export interface MemoryWords {
  journals: string;
  specs: string;
  noJournals: string;
  noSpecs: string;
  newJournal: string;
  edit: string;
  save: string;
  cancel: string;
  version(n: number): string;
  versions: string;
  /** Looking at a version before the newest. */
  older(n: number): string;
  latest: string;
  /** Takes an earlier version's words as the newest. */
  restore: string;
  /** Another version came first while this one was edited. */
  changedElsewhere: string;
  loadThat: string;
  notSaved: string;
  unreadable: string;
  you: string;
  /** The new journal's form. */
  name: string;
  nameRule: string;
  description: string;
  body: string;
  write: string;
  /** Said when the button to write a journal is pointed at. */
  writeHint: string;
  taken: string;
  /** A day, short: the month and the day, with the year where not this one. */
  date(at: string, now: Date): string;
}

function shortDate(at: string, now: Date, locale: string): string {
  const day = new Date(at);
  const year = day.getFullYear() === now.getFullYear() ? undefined : "numeric";
  return day.toLocaleDateString(locale, { year, month: locale.startsWith("zh") ? "numeric" : "short", day: "numeric" });
}

export const zhMemory: MemoryWords = {
  journals: "手记",
  specs: "定论",
  noJournals: "还没有手记",
  noSpecs: "还没有定论",
  newJournal: "写一篇手记",
  edit: "编辑",
  save: "保存",
  cancel: "取消",
  version: (n) => `第 ${n} 版`,
  versions: "历史",
  older: (n) => `这是第 ${n} 版`,
  latest: "看最新的",
  restore: "以这一版为准",
  changedElsewhere: "别处刚改过",
  loadThat: "载入那一版",
  notSaved: "没存上",
  unreadable: "读不到这一篇",
  you: "你",
  name: "名字",
  nameRule: "只用字母、数字与连字符",
  description: "一句话说它是什么",
  body: "正文",
  write: "写下",
  writeHint: "写下之后不再改动",
  taken: "这个项目里已经有同名的一篇",
  date: (at, now) => shortDate(at, now, "zh-CN"),
};

export const enMemory: MemoryWords = {
  journals: "Journal",
  specs: "Decisions",
  noJournals: "No journal yet",
  noSpecs: "No decisions yet",
  newJournal: "Write a journal",
  edit: "Edit",
  save: "Save",
  cancel: "Cancel",
  version: (n) => `Version ${n}`,
  versions: "History",
  older: (n) => `This is version ${n}`,
  latest: "See the newest",
  restore: "Make this the newest",
  changedElsewhere: "Changed elsewhere just now",
  loadThat: "Load that version",
  notSaved: "Not saved",
  unreadable: "This one can't be read",
  you: "You",
  name: "Name",
  nameRule: "Letters, numbers and hyphens only",
  description: "What it is, in a line",
  body: "Text",
  write: "Write",
  writeHint: "Once written it is not changed",
  taken: "This project already has one by that name",
  date: (at, now) => shortDate(at, now, "en-GB"),
};
