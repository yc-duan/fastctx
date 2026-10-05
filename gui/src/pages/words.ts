// What the interface says about pages, in the user's words (`world-gui.md`,
// 用词): a page is something a resident wrote into a project; what it may
// do is said as the thing itself, a command on a machine, words to an
// agent, an entry in a journal.

export interface PageWords {
  pages: string;
  noPages: string;
  unreadable(why: string): string;
  version(n: number): string;
  written(by: string, ago: string): string;
  you: string;
  aPage: string;
  /** The header's buttons. */
  acts: string;
  versions: string;
  reload: string;
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
  noPages: "还没有页面。居民写下的看板、对照表会出现在这里。",
  unreadable: (why) => `页面此刻读不到：${why}`,
  version: (n) => `第 ${n} 版`,
  written: (by, ago) => `${by} · ${ago}`,
  you: "你",
  aPage: "一页页面",
  acts: "它能做的事",
  versions: "版本",
  reload: "重新载入",
  withdraw: "撤下",
  run: (machine) => `在 ${machine} 上跑`,
  say: (who) => `对 ${who} 说话`,
  journal: (name) => `往手记 ${name} 里写`,
  askTitle: "这一页想做几件事",
  askWhat: (n) =>
    `除了读世界里的东西，这一版还想做 ${n} 件事。你同意的，它在你打开它时可以做；做的事都记作这一页做的，不算你亲手做的。不同意也能照常看。`,
  agreedBefore: "你先前同意过的",
  readOnly: "只看",
  agree: "同意",
  agreeChosen: "同意勾选的",
  notNow: "先不",
  noActs: "这一版只读世界里的东西，不做别的事。",
  actsTitle: "它能做的事",
  agreed: "已同意",
  notAgreed: "未同意",
  change: "改成勾选的",
  revoke: "都收回",
  versionsTitle: "版本",
  current: "当前",
  withdrawnMark: "撤下",
  restoredFrom: (n) => `照第 ${n} 版`,
  look: "看",
  bringBack: "回到这一版",
  bringBackWhat: (n) => `把第 ${n} 版的内容发成新的一版，成为当前的。之前的每一版都还在。`,
  withdrawTitle: "撤下这一页",
  withdrawWhat: "撤下也是写一版：这一页不再显示，它能做的事也一并停下。之前的每一版都还在，随时能回到其中一版。",
  older: (n, current) => `这是第 ${n} 版，当前是第 ${current} 版。旧的一版只能看，不能做事。`,
  seeCurrent: "看当前的",
  withdrawnNow: (n) => `这一页在第 ${n} 版撤下了。`,
  bringBackLast: (n) => `回到第 ${n} 版`,
  close: "关闭",
  working: "正在办…",
  failed: (why) => `没办成：${why}`,
};

export const enPages: PageWords = {
  pages: "Pages",
  noPages: "No pages yet. Dashboards and tables the residents write appear here.",
  unreadable: (why) => `The pages cannot be read now: ${why}`,
  version: (n) => `version ${n}`,
  written: (by, ago) => `${by} · ${ago}`,
  you: "you",
  aPage: "a page",
  acts: "What it may do",
  versions: "Versions",
  reload: "Reload",
  withdraw: "Take down",
  run: (machine) => `Run on ${machine}`,
  say: (who) => `Speak to ${who}`,
  journal: (name) => `Write in the journal ${name}`,
  askTitle: "This page wants to do things",
  askWhat: (n) =>
    `Besides reading the World, this version wants to do ${n === 1 ? "one thing" : `${n} things`}. What you agree to it may do while you have it open, and each is recorded as the page's doing, not yours. You can look at it either way.`,
  agreedBefore: "You agreed before",
  readOnly: "Look only",
  agree: "Agree",
  agreeChosen: "Agree to the ticked",
  notNow: "Not now",
  noActs: "This version only reads the World and does nothing else.",
  actsTitle: "What it may do",
  agreed: "Agreed",
  notAgreed: "Not agreed",
  change: "Agree to the ticked",
  revoke: "Take it all back",
  versionsTitle: "Versions",
  current: "current",
  withdrawnMark: "taken down",
  restoredFrom: (n) => `as version ${n}`,
  look: "Look",
  bringBack: "Bring this back",
  bringBackWhat: (n) => `Publishes what version ${n} held as a new version, the current one. Every version before stays.`,
  withdrawTitle: "Take this page down",
  withdrawWhat: "Taking it down is a version too: the page is no longer shown, and what it could do stops with it. Every version before stays, and any can be brought back.",
  older: (n, current) => `This is version ${n}; version ${current} is current. An older version can be looked at but does nothing.`,
  seeCurrent: "See the current one",
  withdrawnNow: (n) => `This page was taken down in version ${n}.`,
  bringBackLast: (n) => `Bring back version ${n}`,
  close: "Close",
  working: "Working…",
  failed: (why) => `Not done: ${why}`,
};
