// The words of machines chosen together (`machine-framed.ts`,
// `machine-batch.ts`, `machine-ways.ts`): each one's version, what is done
// to how many of them, and how an update of several goes.

export interface BatchWords {
  /** The tail of a chosen machine's row: what it runs and whether newer is out. */
  latest: string;
  newer: string;
  away: string;
  /** An action that does not reach every machine chosen, with how many it reaches. */
  some(label: string, count: number): string;
  share: string;
  group: string;
  update: string;
  ways: string;
  remove: string;
  clear: string;
  /** Why a row stands back: nothing here may be done to it. */
  notYours: string;
  /** A row's box in the list, a heading's, and choosing a box's machines
   *  from its "⋯". */
  pickOne(name: string): string;
  pickAll(label: string): string;
  pickGroup: string;

  /** What several run, as the window of versions says it. */
  allRun(count: number, version: string): string;
  oldestRuns(count: number, version: string): string;
  /** The plan of an update to `version`: what becomes of each machine. */
  planTitle(version: string): string;
  willUpdate: string;
  willDue: string;
  skipTaken: string;
  skipNotYours: string;
  skipHere: string;
  skipOlder: string;
  /** An older computer with the app, updated at it; one offline whose
   *  release or hub keeps no update for it till it is back. */
  skipOlderApp: string;
  skipAwayOlder: string;
  skipHubOlder: string;
  start(count: number): string;
  cancel: string;
  /** How each goes once started. */
  queued: string;
  asking: string;
  /** A package coming: how much of it, and whether through the network. */
  fetching(percent: number | null, received: string, network: boolean): string;
  updated(version: string): string;
  dueKept: string;
  finished(done: number, failed: number): string;
  close: string;

  /** Setting how several update. */
  waysTitle(count: number): string;
  auto: string;
  relays: string;
  on: string;
  off: string;
  keep: string;
  apply(count: number): string;
  set: string;
  skipAway: string;
  skipOlderWays: string;
}

export const zhBatch: BatchWords = {
  latest: "最新",
  newer: "可更新",
  away: "不在线",
  some: (label, count) => `${label}（${count} 台）`,
  share: "共享…",
  group: "分组",
  update: "更新…",
  ways: "更新方式",
  remove: "移除…",
  clear: "取消选择",
  notYours: "这台不归你管：只有它的主人和超级管理员能管它",
  pickOne: (name) => `选中 ${name}`,
  pickAll: (label) => `选中 ${label} 的全部机器`,
  pickGroup: "选中这一组",

  allRun: (count, version) => `这 ${count} 台都在跑 ${version}`,
  oldestRuns: (count, version) => `这 ${count} 台里最旧的在跑 ${version}`,
  planTitle: (version) => `更新到 ${version}`,
  willUpdate: "会更新",
  willDue: "不在线：记下，上线后更新",
  skipTaken: "跳过：已是这一版或更新",
  skipNotYours: "跳过：不是你能更新的",
  skipHere: "跳过：这台在设置里更新",
  skipOlder: "跳过：旧版本只能直接更新到最新",
  skipOlderApp: "跳过：旧版本的电脑在它自己的 Fastctx 里更新",
  skipAwayOlder: "跳过：不在线，旧版本上线后也不会照记下的更新",
  skipHubOlder: "跳过：不在线，这个网络的中枢较旧，记不下上线就更新",
  start: (count) => `开始更新（${count} 台）`,
  cancel: "取消",
  queued: "排队中",
  asking: "正在请它更新…",
  fetching: (percent, received, network) => `下载中 ${percent === null ? received : `${percent}%`}${network ? " · 经网络" : ""}`,
  updated: (version) => `已更新到 ${version}`,
  dueKept: "已记下：上线后更新",
  finished: (done, failed) => (failed ? `完成 ${done} 台，${failed} 台没成` : `完成 ${done} 台`),
  close: "完成",

  waysTitle: (count) => `这 ${count} 台怎样更新`,
  auto: "自动更新",
  relays: "帮网络传包",
  on: "开",
  off: "关",
  keep: "不改",
  apply: (count) => `应用到 ${count} 台`,
  set: "已设好",
  skipAway: "跳过：不在线",
  skipOlderWays: "跳过：旧版本不能从这里设",
};

export const enBatch: BatchWords = {
  latest: "Up to date",
  newer: "Update available",
  away: "Offline",
  some: (label, count) => `${label} (${count})`,
  share: "Share…",
  group: "Group",
  update: "Update…",
  ways: "How they update",
  remove: "Remove…",
  clear: "Clear",
  notYours: "Not yours to manage: only its owner and the super administrator manage it",
  pickOne: (name) => `Choose ${name}`,
  pickAll: (label) => `Choose every machine in ${label}`,
  pickGroup: "Choose this group",

  allRun: (count, version) => `All ${count} run ${version}`,
  oldestRuns: (count, version) => `The oldest of these ${count} runs ${version}`,
  planTitle: (version) => `Update to ${version}`,
  willUpdate: "Updates",
  willDue: "Offline: kept, updates once back",
  skipTaken: "Skipped: runs this version or newer",
  skipNotYours: "Skipped: not yours to update",
  skipHere: "Skipped: this device updates in its settings",
  skipOlder: "Skipped: an older version updates only to the newest",
  skipOlderApp: "Skipped: an older computer with the app updates in its own Fastctx",
  skipAwayOlder: "Skipped: offline, and an older version takes no update kept for it",
  skipHubOlder: "Skipped: offline, and this network's hub is older and keeps no update for later",
  start: (count) => `Update ${count}`,
  cancel: "Cancel",
  queued: "Queued",
  asking: "Asking it to update…",
  fetching: (percent, received, network) => `Downloading ${percent === null ? received : `${percent}%`}${network ? " · through the network" : ""}`,
  updated: (version) => `Updated to ${version}`,
  dueKept: "Kept: updates once back",
  finished: (done, failed) => (failed ? `${done} done, ${failed} not` : `${done} done`),
  close: "Done",

  waysTitle: (count) => `How these ${count} update`,
  auto: "Automatic updates",
  relays: "Pass packages on",
  on: "On",
  off: "Off",
  keep: "Leave as is",
  apply: (count) => `Apply to ${count}`,
  set: "Set",
  skipAway: "Skipped: offline",
  skipOlderWays: "Skipped: an older version is not set from here",
};
