// What the interface says about pages, in the user's words (`world-gui.md`,
// 用词): a page is something a resident wrote into a project; what it may
// do is said as the thing itself, a command on a machine, words to an
// agent, an entry in a journal.

import { enProblems, failure, zhProblems } from "../app/problem";

export interface PageWords {
  pages: string;
  noPages: string;
  version(n: number): string;
  written(by: string, ago: string): string;
  you: string;
  /** A conversation, or a page, on a machine this device's account may
   *  not use, as who wrote a version. */
  elsewhere(page: boolean): string;
  aPage: string;
  /** The header's buttons. */
  acts: string;
  versions: string;
  reload: string;
  /** The page navigated its own frame somewhere else. */
  wentAway: string;
  withdraw: string;
  /** What a version may do. */
  run(machine: string): string;
  say(who: string): string;
  journal(name: string): string;
  /** The consent sheet. */
  askTitle: string;
  askWhat(n: number): string;
  agreedBefore: string;
  readOnly: string;
  agree: string;
  agreeChosen: string;
  notNow: string;
  noActs: string;
  /** The acts sheet, after the first time. */
  actsTitle: string;
  agreed: string;
  notAgreed: string;
  change: string;
  revoke: string;
  /** Versions. */
  versionsTitle: string;
  current: string;
  withdrawnMark: string;
  restoredFrom(n: number): string;
  look: string;
  bringBack: string;
  bringBackWhat(n: number): string;
  withdrawTitle: string;
  withdrawWhat: string;
  /** Banners. */
  older(n: number, current: number): string;
  seeCurrent: string;
  withdrawnNow(n: number): string;
  bringBackLast(n: number): string;
  close: string;
  working: string;
  failed(why: string): string;
}

export const zhPages: PageWords = {
  pages: "页面",
  noPages: "暂无页面",
  version: (n) => `第 ${n} 版`,
  written: (by, ago) => `${by}　${ago}`,
  you: "你",
  elsewhere: (page) => (page ? "一个页面" : "一段对话"),
  aPage: "页面",
  acts: "权限",
  versions: "版本",
  reload: "重新载入",
  wentAway: "页面已跳转到其他内容并被关闭，重新载入可恢复",
  withdraw: "撤下",
  run: (machine) => `在 ${machine} 上运行`,
  say: (who) => `向 ${who} 发消息`,
  journal: (name) => `写入手记 ${name}`,
  askTitle: "页面请求权限",
  askWhat: (n) => `此版本请求 ${n} 项权限，同意后在打开时可执行，操作记录为页面所为。不同意也可正常浏览。`,
  agreedBefore: "已同意过",
  readOnly: "仅浏览",
  agree: "同意",
  agreeChosen: "同意所选",
  notNow: "暂不",
  noActs: "此版本只读取数据",
  actsTitle: "权限",
  agreed: "已同意",
  notAgreed: "未同意",
  change: "保存",
  revoke: "全部撤销",
  versionsTitle: "版本",
  current: "当前",
  withdrawnMark: "已撤下",
  restoredFrom: (n) => `恢复自第 ${n} 版`,
  look: "查看",
  bringBack: "恢复此版",
  bringBackWhat: (n) => `将第 ${n} 版发布为新的当前版本，历史版本保留。`,
  withdrawTitle: "撤下页面？",
  withdrawWhat: "页面将不再显示，其权限一并停止。历史版本保留，可随时恢复。",
  older: (n, current) => `第 ${n} 版（当前为第 ${current} 版），旧版本仅可浏览`,
  seeCurrent: "查看当前版本",
  withdrawnNow: (n) => `已在第 ${n} 版撤下`,
  bringBackLast: (n) => `恢复第 ${n} 版`,
  close: "关闭",
  working: "处理中…",
  failed: failure(zhProblems, "操作失败"),
};

export const enPages: PageWords = {
  pages: "Pages",
  noPages: "No pages yet",
  version: (n) => `version ${n}`,
  written: (by, ago) => `${by}, ${ago}`,
  you: "you",
  elsewhere: (page) => (page ? "a page" : "a conversation"),
  aPage: "page",
  acts: "Permissions",
  versions: "Versions",
  reload: "Reload",
  wentAway: "The page navigated away and was closed; reload to restore it",
  withdraw: "Take down",
  run: (machine) => `Run on ${machine}`,
  say: (who) => `Message ${who}`,
  journal: (name) => `Write to journal ${name}`,
  askTitle: "This page requests permissions",
  askWhat: (n) => `This version requests ${n} permission${n === 1 ? "" : "s"}. Allowed actions run while it's open and are recorded as the page's. You can browse either way.`,
  agreedBefore: "Previously allowed",
  readOnly: "Browse only",
  agree: "Allow",
  agreeChosen: "Allow selected",
  notNow: "Not now",
  noActs: "This version only reads data",
  actsTitle: "Permissions",
  agreed: "Allowed",
  notAgreed: "Not allowed",
  change: "Save",
  revoke: "Revoke all",
  versionsTitle: "Versions",
  current: "current",
  withdrawnMark: "taken down",
  restoredFrom: (n) => `restored from version ${n}`,
  look: "View",
  bringBack: "Restore this version",
  bringBackWhat: (n) => `Publishes version ${n} as the new current version; history is kept.`,
  withdrawTitle: "Take down this page?",
  withdrawWhat: "The page will be hidden and its permissions stop. History is kept and can be restored.",
  older: (n, current) => `Version ${n} (current is ${current}); older versions are view-only`,
  seeCurrent: "View current",
  withdrawnNow: (n) => `Taken down in version ${n}`,
  bringBackLast: (n) => `Restore version ${n}`,
  close: "Close",
  working: "Working…",
  failed: failure(enProblems, "Couldn't complete"),
};
