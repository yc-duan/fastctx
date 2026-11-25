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
  /** It was deleted for good and written anew while this one was edited. */
  rewritten: string;
  loadThat: string;
  notSaved: string;
  unreadable: string;
  you: string;
  /** The user, as who wrote a version on a machine they may see. */
  youOn(machine: string): string;
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
  /** Deleting a document for good, and the card that asks first. */
  remove: string;
  removeTitle(name: string): string;
  removeWhat: string;
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
  /** A project's history, as the mark that opens it in a conversation's
   *  side names it. */
  of(project: string): string;
  /** What a conversation wrote in its own history before it joined its
   *  project. */
  earlier: string;
  /** The box that finds documents by their name and what they cover, and
   *  what it says when it finds none. */
  find: string;
  noneFound: string;
  /** A conversation the World no longer lists, as the way back names it. */
  untitled: string;
}

function shortDate(at: string, now: Date, locale: string): string {
  const day = new Date(at);
  const year = day.getFullYear() === now.getFullYear() ? undefined : "numeric";
  return day.toLocaleDateString(locale, { year, month: locale.startsWith("zh") ? "numeric" : "short", day: "numeric" });
}

export const zhHistory: HistoryWords = {
  history: "笔记",
  noDocuments: "暂无笔记",
  newDocument: "新建笔记",
  edit: "编辑",
  save: "保存",
  cancel: "取消",
  version: (n) => `第 ${n} 版`,
  versions: "版本",
  current: "当前",
  look: "查看",
  text: "正文",
  older: (n) => `第 ${n} 版`,
  latest: "查看最新版",
  restore: "恢复此版",
  changedElsewhere: "已在别处修改",
  rewritten: "此篇已在别处删除并重写",
  loadThat: "载入该版本",
  notSaved: "保存失败",
  unreadable: "无法读取",
  you: "你",
  youOn: (machine) => `${machine} 上的你`,
  elsewhere: (page) => (page ? "一个页面" : "一段对话"),
  retire: "撤下",
  retireTitle: (name) => `撤下「${name}」？`,
  retireWhat: "撤下后智能体不再读取此篇；各版本保留，可随时恢复。",
  retired: "已撤下",
  bringBack: "恢复",
  remove: "彻底删除",
  removeTitle: (name) => `彻底删除「${name}」？`,
  removeWhat: "删除后所有版本都不能再读取，也无法恢复；以后可以用同一个名称新写一篇。",
  name: "名称",
  nameRule: "只用小写字母、数字与连字符",
  description: "适用范围与检索关键词",
  firstParagraph: "第一段",
  write: "写下",
  taken: "已存在同名笔记",
  changed: (who, ago) => `${who}　${ago}`,
  date: (at, now) => shortDate(at, now, "zh-CN"),
  of: (project) => `${project}${/[A-Za-z0-9]$/.test(project) ? " " : ""}的笔记`,
  earlier: "此前的笔记",
  find: "搜索笔记",
  noneFound: "没有找到",
  untitled: "对话",
};

export const enHistory: HistoryWords = {
  history: "Notes",
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
  rewritten: "Deleted and written anew elsewhere",
  loadThat: "Load that version",
  notSaved: "Not saved",
  unreadable: "This one cannot be read",
  you: "You",
  youOn: (machine) => `You on ${machine}`,
  elsewhere: (page) => (page ? "A page" : "A conversation"),
  retire: "Take down",
  retireTitle: (name) => `Take down “${name}”?`,
  retireWhat: "Agents stop receiving it once it is taken down. Every version stays, and it can be brought back.",
  retired: "Taken down",
  bringBack: "Bring back",
  remove: "Delete for good",
  removeTitle: (name) => `Delete “${name}” for good?`,
  removeWhat: "Every version goes and none can be brought back. The name can be used again for a new one.",
  name: "Name",
  nameRule: "Lower-case letters, numbers and hyphens only",
  description: "What it covers, and the words to find it by",
  firstParagraph: "First paragraph",
  write: "Write",
  taken: "One by that name already exists",
  changed: (who, ago) => `${who}　${ago}`,
  date: (at, now) => shortDate(at, now, "en-GB"),
  of: (project) => `Notes of ${project}`,
  earlier: "Earlier notes",
  find: "Find a note",
  noneFound: "Nothing found",
  untitled: "Conversation",
};
