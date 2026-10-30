// The words of the machines page's update view (`world-gui.md`, 界面的结构):
// the sentence behind each machine's mark, what the version ruler notes,
// the network's update policy, and what is done to the machines chosen.

import type { ActionReason, FailureReason } from "../world/updates";
import { enRepair, zhRepair, type RepairWords } from "./repair-words";

export interface UpdateViewWords {
  /** The view's name in the page's head. */
  view: string;
  /** What a machine's mark says when pointed at or held. */
  newest: string;
  behind(version: string): string;
  fetching(percent: number | null, via: "github" | "network" | null): string;
  ready(version: string): string;
  waitingIdle: string;
  waitingApp: string;
  /** No one is signed in to that computer: the application installs once
   *  someone is. */
  waitingSignIn: string;
  installing(version: string): string;
  renewing(version: string): string;
  administrator: string;
  due(version: string, by: string): string;
  dueBlocked(version: string, why: string): string;
  failed(version: string, why: string): string;
  unknown: string;
  /** A group's composition, said behind its small bar. */
  group(newest: number, going: number, behind: number, failed: number): string;
  /** What a machine runs, said behind its short version. */
  runs(version: string): string;
  /** The ruler's notes: the hub's version, the network's minimum, older
   *  releases put together. */
  hubMark: string;
  minimumMark: string;
  older: string;
  machinesOn(count: number, version: string): string;
  /** The network's update policy, the super administrator's, and its
   *  folded line on a phone. */
  policy: string;
  minimum: string;
  noMinimum: string;
  managedAuto: string;
  /** A switch of a machine's own updating by itself. */
  auto: string;
  /** A hub too old to tell the view; a view not read for another reason,
   *  the reason under the pointer. */
  hubOld: string;
  unread: string;
  /** What is done to the machines chosen, and its short words on a phone. */
  updateTo: string;
  ways: string;
  cancel: string;
  /** Repairing machines, and calling off a repair kept for one away. */
  repair: string;
  cancelRepair: string;
  bar: { updateTo: string; ways: string; cancel: string; repair: string; cancelRepair: string };
  /** A repair under way on it, at which layer. */
  repairing(layer: string): string;
  /** A repair kept for it until it is back, and who asked for it. */
  repairDue(by: string): string;
  repairDueBlocked(why: string): string;
  /** Its last repair stopped short. */
  repairFailed: string;
  repairWords: RepairWords;
  /** Machines a scheduling did not reach. */
  notScheduled(why: string): string;
  /** Why something cannot be done to a machine. */
  reasons: Record<ActionReason, string>;
  /** Why an update did not go through. */
  failures: Record<FailureReason["kind"], string>;
}

export const zhUpdateView: UpdateViewWords = {
  view: "更新",
  newest: "已是最新",
  behind: (version) => `可更新到 ${version}`,
  fetching: (percent, via) => `下载中${percent === null ? "" : ` ${percent}%`}${via === "github" ? "，从 GitHub" : via === "network" ? "，经网络" : ""}`,
  ready: (version) => `${version} 已下好，等装上`,
  waitingIdle: "等手上的活告一段落",
  waitingApp: "下次在那台打开 Fastctx 时更新",
  waitingSignIn: "等登录后更新",
  installing: (version) => `正在装上 ${version}`,
  renewing: (version) => `正在换上 ${version}`,
  administrator: "正在更新开机服务",
  due: (version, by) => `已安排：上线后更新到 ${version}（${by} 安排的）`,
  dueBlocked: (version, why) => `安排了更新到 ${version}，但${why}`,
  failed: (version, why) => `没更新到 ${version}：${why}`,
  unknown: "还不知道它在跑哪一版",
  group: (newest, going, behind, failed) => [newest ? `已是最新 ${newest} 台` : "", going ? `在路上 ${going} 台` : "", behind ? `可更新 ${behind} 台` : "", failed ? `没更新成 ${failed} 台` : ""].filter(Boolean).join("，"),
  runs: (version) => `在跑 ${version}`,
  hubMark: "中枢",
  minimumMark: "最低",
  older: "更早",
  machinesOn: (count, version) => `${count} 台在跑 ${version}`,
  policy: "网络的更新",
  minimum: "网络要求至少",
  noMinimum: "不要求",
  managedAuto: "受管机器自动更新",
  auto: "自动更新",
  hubOld: "中枢还是旧版本，换上新版后这里才能管更新",
  unread: "没读到中枢的更新情况",
  updateTo: "更新到…",
  ways: "更新方式",
  cancel: "撤销安排",
  repair: "修复",
  cancelRepair: "撤销修复",
  bar: { updateTo: "更新到…", ways: "方式", cancel: "撤销", repair: "修复", cancelRepair: "撤销修复" },
  repairing: (layer) => `正在修复：${layer}`,
  repairDue: (by) => `已安排：上线后修复（${by} 安排的）`,
  repairDueBlocked: (why) => `安排了修复，但${why}`,
  repairFailed: "上次修复没修好",
  repairWords: zhRepair,
  notScheduled: (why) => `没安排上：${why}`,
  reasons: {
    not_owner: "不是你能管的机器",
    visitor: "访客的机器只归他自己",
    phone: "手机在它自己上面更新",
    too_old: "版本太旧，只能更新到最新",
    development: "开发版不参与更新",
    offline: "不在线",
    unknown_version: "还不知道它在跑哪一版",
    missing_machine: "这台已不在网络里",
    no_package: "这一版没有它的包",
    not_newer: "已是这一版或更新",
    unavailable: "现在拿不到这一版",
    other: "做不了",
  },
  failures: {
    no_package: "这一版没有它的包",
    signature: "包的签名对不上",
    disk: "磁盘写不进去",
    not_newer: "不比在跑的新",
    sources: "几个来源都拿不到这一版",
    installer: "安装程序没装成",
    other: "没装成",
  },
};

export const enUpdateView: UpdateViewWords = {
  view: "Updates",
  newest: "Up to date",
  behind: (version) => `${version} available`,
  fetching: (percent, via) => `Downloading${percent === null ? "" : ` ${percent}%`}${via === "github" ? " from GitHub" : via === "network" ? " through the network" : ""}`,
  ready: (version) => `${version} downloaded, waiting to install`,
  waitingIdle: "Waiting for the work under way to finish",
  waitingApp: "Updates when Fastctx is next opened there",
  waitingSignIn: "Updates once someone signs in there",
  installing: (version) => `Installing ${version}`,
  renewing: (version) => `Switching to ${version}`,
  administrator: "Updating its startup service",
  due: (version, by) => `Scheduled: updates to ${version} when back (by ${by})`,
  dueBlocked: (version, why) => `Scheduled for ${version}, but ${why}`,
  failed: (version, why) => `Didn't update to ${version}: ${why}`,
  unknown: "Not known yet what it runs",
  group: (newest, going, behind, failed) => [newest ? `${newest} up to date` : "", going ? `${going} on the way` : "", behind ? `${behind} can update` : "", failed ? `${failed} didn't update` : ""].filter(Boolean).join(", "),
  runs: (version) => `Runs ${version}`,
  hubMark: "Hub",
  minimumMark: "Minimum",
  older: "Older",
  machinesOn: (count, version) => `${count} on ${version}`,
  policy: "Network updates",
  minimum: "Network requires at least",
  noMinimum: "None",
  managedAuto: "Managed machines update by themselves",
  auto: "Update automatically",
  hubOld: "The hub runs an older version; updates can be managed here once it is updated",
  unread: "Couldn't read the hub's updates",
  updateTo: "Update to…",
  ways: "Update settings",
  cancel: "Cancel scheduled",
  repair: "Repair",
  cancelRepair: "Cancel repair",
  bar: { updateTo: "Update", ways: "Settings", cancel: "Cancel", repair: "Repair", cancelRepair: "Cancel repair" },
  repairing: (layer) => `Repairing: ${layer}`,
  repairDue: (by) => `Scheduled: repairs when back (by ${by})`,
  repairDueBlocked: (why) => `A repair is scheduled, but ${why}`,
  repairFailed: "Its last repair didn't finish",
  repairWords: enRepair,
  notScheduled: (why) => `Not scheduled: ${why}`,
  reasons: {
    not_owner: "Not a machine you manage",
    visitor: "A visitor's machine is only theirs",
    phone: "A phone updates on itself",
    too_old: "Too old: it only updates to the newest",
    development: "Development builds don't take updates",
    offline: "Offline",
    unknown_version: "Not known yet what it runs",
    missing_machine: "No longer in the network",
    no_package: "This version has no package for it",
    not_newer: "Already on this version or newer",
    unavailable: "This version can't be had now",
    other: "Can't be done",
  },
  failures: {
    no_package: "this version has no package for it",
    signature: "the package's signature didn't match",
    disk: "the disk couldn't be written",
    not_newer: "it isn't newer than what runs",
    sources: "no source could provide this version",
    installer: "the installer failed",
    other: "it didn't install",
  },
};
