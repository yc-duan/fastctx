// What the lists of changed files, a machine's folders and a file opened say
// (`gui-files`): the words a list of changed files is read in everywhere.

import type { How } from "./model";

export interface FilesWords {
  title: string;
  how: Record<How, string>;
  /** A file seen changed in its repository, rather than written by
   *  Fastctx's own commands: its lines are counted against the last commit. */
  seen: string;
  files(n: number): string;
  /** How many files changed, over a list. */
  changed(files: number): string;
  /** Whose session changed it, where it has no name of its own. */
  own(agent: string): string;
  /** A session of another machine, beside who changed it. */
  from(machine: string): string;
  /** Sessions that changed a file besides the one named. */
  others(n: number): string;
  /** Files of a list not shown until asked for. */
  moreFiles(n: number): string;
  /** A moment in a list: the time today, yesterday's with its day, else
   *  the date. */
  stamp(at: number, now: number): string;
  tree: {
    label: string;
    all: string;
    changed: string;
    /** What the switch between them chooses. */
    shows: string;
    reading: string;
    failed: string;
    empty: string;
    /** Nothing was changed under the folder shown. */
    unchanged: string;
    more(n: number): string;
    up: string;
    home: string;
    places: string;
    /** This machine does not list its files to this version. */
    unlisted: string;
  };
  view: {
    tabs: string;
    content: string;
    changes(n: number): string;
    outside: string;
    /** Opens the file among its machine's folders. */
    inFolders: string;
    reading: string;
    missing: string;
    failed: string;
    interrupted: string;
    /** The file is gone: only its changes are left to read. */
    removed: string;
    /** A file that is no text and no picture. */
    notShown: string;
    openWith: string;
    /** A change whose lines were not kept. */
    notKept: string;
    seenOnly: string;
    /** A change only seen in its repository, in a word beside it. */
    seenTag: string;
    uncounted: string;
    /** Lines left alone between two parts of a change. */
    gap(n: number | null): string;
    /** Lines of a long change not drawn. */
    longer(n: number): string;
    earlier(n: number): string;
    noChanges: string;
    picture: string;
  };
  page: {
    machines: string;
    /** No file is opened. */
    choose: string;
    offline: string;
    /** The files changed lately, offered where none is opened. */
    lately: string;
  };
  turn: {
    changed(n: number): string;
    total(n: number): string;
  };
  side: {
    name: string;
    changes: string;
    folders: string;
    shows: string;
    none: string;
    all(n: number): string;
  };
}

const clock = (date: Date) => `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
const daysBefore = (at: number, now: number) => Math.round((new Date(now).setHours(0, 0, 0, 0) - new Date(at).setHours(0, 0, 0, 0)) / 86_400_000);

export const zhFiles: FilesWords = {
  title: "文件",
  how: { added: "新建", changed: "修改", removed: "删除", copied: "复制到这里" },
  seen: "仓库中有变更，行数相对上次提交",
  files: (n) => `${n} 个文件`,
  changed: (files) => `${files} 个文件有改动`,
  own: (agent) => `${agent} 会话`,
  from: (machine) => `来自 ${machine}`,
  others: (n) => `等 ${n + 1} 个对话`,
  moreFiles: (n) => `还有 ${n} 个文件`,
  stamp: (at, now) => {
    const date = new Date(at);
    const days = daysBefore(at, now);
    if (days <= 0) return clock(date);
    if (days === 1) return `昨天 ${clock(date)}`;
    return `${new Date(now).getFullYear() === date.getFullYear() ? "" : `${date.getFullYear()}年`}${date.getMonth() + 1}月${date.getDate()}日 ${clock(date)}`;
  },
  tree: {
    label: "目录",
    all: "全部文件",
    changed: "有改动的",
    shows: "显示哪些文件",
    reading: "正在读取",
    failed: "读取失败",
    empty: "空文件夹",
    unchanged: "这里没有文件改动",
    more: (n) => `还有 ${n} 项`,
    up: "上一级",
    home: "主目录",
    places: "位置",
    unlisted: "这台电脑的版本还不能列出文件",
  },
  view: {
    tabs: "看什么",
    content: "内容",
    changes: (n) => (n ? `改动 ${n}` : "改动"),
    outside: "用本机程序打开",
    inFolders: "在目录中查看",
    reading: "正在读取",
    missing: "文件不存在",
    failed: "读取失败",
    interrupted: "连接中断，恢复后继续",
    removed: "文件已删除，只留下改动记录",
    notShown: "这种文件不在这里显示",
    openWith: "用本机程序打开",
    notKept: "这次改动的内容没有留存",
    seenOnly: "命令运行后在仓库中发现的变更，没有逐行记录",
    seenTag: "仓库中的变更",
    uncounted: "不是文本，未统计行数",
    gap: (n) => (n === null ? "⋯" : `${n} 行未改动`),
    longer: (n) => `还有 ${n} 行未显示`,
    earlier: (n) => `更早的 ${n} 次改动`,
    noChanges: "没有改动记录",
    picture: "图片",
  },
  page: {
    machines: "电脑",
    choose: "从左侧选择一个文件",
    offline: "这台电脑不在线",
    lately: "最近改动",
  },
  turn: {
    changed: (n) => `本轮改了 ${n} 个文件`,
    total: (n) => `对话共 ${n} 个文件`,
  },
  side: {
    name: "文件",
    changes: "改动",
    folders: "目录",
    shows: "看什么",
    none: "这个对话还没有改动文件",
    all: (n) => `全部 ${n} 个`,
  },
};

export const enFiles: FilesWords = {
  title: "Files",
  how: { added: "Added", changed: "Changed", removed: "Removed", copied: "Copied here" },
  seen: "Seen changed in its repository; lines are counted against the last commit",
  files: (n) => `${n} ${n === 1 ? "file" : "files"}`,
  changed: (files) => `${files} changed ${files === 1 ? "file" : "files"}`,
  own: (agent) => `A ${agent} session`,
  from: (machine) => `from ${machine}`,
  others: (n) => `and ${n} more`,
  moreFiles: (n) => `${n} more ${n === 1 ? "file" : "files"}`,
  stamp: (at, now) => {
    const date = new Date(at);
    const days = daysBefore(at, now);
    if (days <= 0) return clock(date);
    if (days === 1) return `Yesterday ${clock(date)}`;
    return `${date.toLocaleDateString("en", { month: "short", day: "numeric", ...(new Date(now).getFullYear() === date.getFullYear() ? {} : { year: "numeric" }) })} ${clock(date)}`;
  },
  tree: {
    label: "Folders",
    all: "All files",
    changed: "Changed",
    shows: "Which files are shown",
    reading: "Reading",
    failed: "Could not be read",
    empty: "Empty folder",
    unchanged: "No file was changed here",
    more: (n) => `${n} more`,
    up: "Up a folder",
    home: "Home",
    places: "Places",
    unlisted: "This computer's version cannot list its files yet",
  },
  view: {
    tabs: "What is shown",
    content: "Content",
    changes: (n) => (n ? `Changes ${n}` : "Changes"),
    outside: "Open with this device's program",
    inFolders: "Show among its folders",
    reading: "Reading",
    missing: "The file does not exist",
    failed: "Could not be read",
    interrupted: "Connection lost; it resumes when it is back",
    removed: "The file was removed; only its changes are left",
    notShown: "This kind of file is not shown here",
    openWith: "Open with this device's program",
    notKept: "The lines of this change were not kept",
    seenOnly: "Seen changed in its repository after a command; not recorded line by line",
    seenTag: "seen in the repository",
    uncounted: "Not text; lines are not counted",
    gap: (n) => (n === null ? "⋯" : `${n} unchanged ${n === 1 ? "line" : "lines"}`),
    longer: (n) => `${n} more ${n === 1 ? "line" : "lines"} not shown`,
    earlier: (n) => `${n} earlier ${n === 1 ? "change" : "changes"}`,
    noChanges: "No change is recorded",
    picture: "Picture",
  },
  page: {
    machines: "Computers",
    choose: "Choose a file on the left",
    offline: "This computer is offline",
    lately: "Changed lately",
  },
  turn: {
    changed: (n) => `${n} ${n === 1 ? "file" : "files"} changed this turn`,
    total: (n) => `${n} ${n === 1 ? "file" : "files"} in this conversation`,
  },
  side: {
    name: "Files",
    changes: "Changes",
    folders: "Folders",
    shows: "What is shown",
    none: "This conversation has changed no file yet",
    all: (n) => `All ${n}`,
  },
};
