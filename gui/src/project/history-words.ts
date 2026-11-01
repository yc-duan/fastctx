// The words of a history (`world-history.md`): its documents on a project's
// page and in a conversation's side panel, a document read, edited, taken
// down and brought back, and a new one written.

export interface HistoryWords {
  history: string;
  noDocuments: string;
  newDocument: string;
  edit: string;
  save: string;
  cancel: string;
  version(n: number): string;
  versions: string;
  /** The newest version, marked among them. */
  current: string;
  /** Opens a version to read. */
  look: string;
  /** The text field, edited. */
  text: string;
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
  /** A conversation, or a page, on a machine this device's account may
   *  not use, as who wrote something. */
  elsewhere(page: boolean): string;
  /** Taking a document down, and the card that asks first. */
  retire: string;
  retireTitle(name: string): string;
  retireWhat: string;
  /** A document taken down, read. */
  retired: string;
  bringBack: string;
  /** The new document's card. */
  name: string;
  nameRule: string;
  description: string;
  firstParagraph: string;
  write: string;
  taken: string;
  /** Who changed it last and when, said where the pointer rests on a row. */
  changed(who: string, ago: string): string;
  /** A day, short: the month and the day, with the year where not this one. */
  date(at: string, now: Date): string;
}

function shortDate(at: string, now: Date, locale: string): string {
  const day = new Date(at);
  const year = day.getFullYear() === now.getFullYear() ? undefined : "numeric";
  return day.toLocaleDateString(locale, { year, month: locale.startsWith("zh") ? "numeric" : "short", day: "numeric" });
}

export const zhHistory: HistoryWords = {
  history: "历史",
  noDocuments: "还没有记下什么",
  newDocument: "写一篇",
  edit: "编辑",
  save: "保存",
  cancel: "取消",
  version: (n) => `第 ${n} 版`,
  versions: "版本",
  current: "当前",
  look: "查看",
  text: "正文",
  older: (n) => `这是第 ${n} 版`,
  latest: "看最新的",
  restore: "以这一版为准",
  changedElsewhere: "别处刚改过",
  loadThat: "载入那一版",
  notSaved: "没存上",
  unreadable: "读不到这一篇",
  you: "你",
  elsewhere: (page) => (page ? "一个页面" : "一段对话"),
  retire: "撤下",
  retireTitle: (name) => `撤下「${name}」？`,
  retireWhat: "撤下之后，智能体不再拿到这一篇；各个版本都留着，随时能恢复。",
  retired: "已撤下",
  bringBack: "恢复",
  name: "名字",
  nameRule: "只用小写字母、数字与连字符",
  description: "管什么，以及会被拿来搜它的词",
  firstParagraph: "第一段",
  write: "写下",
  taken: "已经有同名的一篇",
  changed: (who, ago) => `${who}　${ago}`,
  date: (at, now) => shortDate(at, now, "zh-CN"),
};

export const enHistory: HistoryWords = {
  history: "History",
  noDocuments: "Nothing recorded yet",
  newDocument: "Write one",
  edit: "Edit",
  save: "Save",
  cancel: "Cancel",
  version: (n) => `Version ${n}`,
  versions: "Versions",
  current: "Current",
  look: "Open",
  text: "Text",
  older: (n) => `This is version ${n}`,
  latest: "See the newest",
  restore: "Make this the newest",
  changedElsewhere: "Changed elsewhere just now",
  loadThat: "Load that version",
  notSaved: "Not saved",
  unreadable: "This one can't be read",
  you: "You",
  elsewhere: (page) => (page ? "A page" : "A conversation"),
  retire: "Take down",
  retireTitle: (name) => `Take down “${name}”?`,
  retireWhat: "Agents stop receiving it once it is taken down. Every version stays, and it can be brought back.",
  retired: "Taken down",
  bringBack: "Bring back",
  name: "Name",
  nameRule: "Lower-case letters, numbers and hyphens only",
  description: "What it covers, and the words to find it by",
  firstParagraph: "First paragraph",
  write: "Write",
  taken: "One by that name already exists",
  changed: (who, ago) => `${who}　${ago}`,
  date: (at, now) => shortDate(at, now, "en-GB"),
};
