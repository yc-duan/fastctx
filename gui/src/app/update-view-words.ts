// The words of the machines page's update view (`world-gui.md`, 界面的结构):
// the sentence behind each machine's mark, what the version ruler notes,
// the network's update policy, and what is done to the machines chosen.

import type { ActionReason, FailureReason } from "../world/updates";
import { enProblems, zhProblems, type ProblemWords } from "./problem";
import { enRepair, zhRepair, type RepairWords } from "./repair-words";
import { enTroubles, zhTroubles, type TroubleWords } from "./trouble-words";

export interface UpdateViewWords {
  /** The kernel's errors, in plain words (`problem.ts`). */
  problems: ProblemWords;
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
  /** A release going out a batch at a time (`rollout-window.ts`): the
   *  policy's item and the sheet's title, none, how it stands, and the
   *  ruler's notes of it and of a withdrawn release. */
  rollout: string;
  noRollout: string;
  rolloutPaused: string;
  rolloutStopped: string;
  rolloutMark(share: number): string;
  withdrawnMark: string;
  /** The rollout's sheet: its version, the share of the other machines,
   *  the machines that take it first and finding them, what is done. */
  rolloutVersion: string;
  rolloutAnother: string;
  rolloutShare: string;
  rolloutFirst: string;
  rolloutFind: string;
  rolloutFound: string;
  rolloutStart: string;
  rolloutSave: string;
  rolloutEnd: string;
  rolloutPause: string;
  rolloutResume: string;
  /** Why the hub stopped it: the machine and what went wrong there, and
   *  what that means behind the pointer. */
  rolloutStoppedBy(machine: string, what: string): string;
  rolloutStoppedWhy: string;
  /** What went wrong, as the alerts say it. */
  troubles: TroubleWords;
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
  problems: zhProblems,
  view: "更新",
  newest: "已是最新",
  behind: (version) => `可更新到 ${version}`,
  fetching: (percent, via) => `下载中${percent === null ? "" : ` ${percent}%`}${via === "github" ? "，从 GitHub" : via === "network" ? "，经网络" : ""}`,
  ready: (version) => `${version} 已下载，等待安装`,
  waitingIdle: "等待当前工作结束",
  waitingApp: "该机器下次打开 Fastctx 时更新",
  waitingSignIn: "登录后更新",
  installing: (version) => `正在安装 ${version}`,
  renewing: (version) => `正在切换到 ${version}`,
  administrator: "正在更新开机服务",
  due: (version, by) => `已安排：上线后更新到 ${version}（${by}）`,
  dueBlocked: (version, why) => `已安排更新到 ${version}，但${why}`,
  failed: (version, why) => `未能更新到 ${version}：${why}`,
  unknown: "运行版本未知",
  group: (newest, going, behind, failed) => [newest ? `已是最新 ${newest} 台` : "", going ? `更新中 ${going} 台` : "", behind ? `可更新 ${behind} 台` : "", failed ? `更新失败 ${failed} 台` : ""].filter(Boolean).join("，"),
  runs: (version) => `运行 ${version}`,
  hubMark: "中枢",
  minimumMark: "最低",
  older: "更早",
  machinesOn: (count, version) => `${count} 台运行 ${version}`,
  policy: "网络的更新",
  minimum: "网络要求至少",
  noMinimum: "不要求",
  managedAuto: "受管机器自动更新",
  rollout: "分批推广",
  noRollout: "不分批",
  rolloutPaused: "已暂停",
  rolloutStopped: "已停止",
  rolloutMark: (share) => `推广 ${share}%`,
  withdrawnMark: "已撤回",
  rolloutVersion: "版本",
  rolloutAnother: "选择其他版本",
  rolloutShare: "其余机器",
  rolloutFirst: "优先推送",
  rolloutFind: "查找机器",
  rolloutFound: "无匹配的机器",
  rolloutStart: "开始推广",
  rolloutSave: "保存",
  rolloutEnd: "结束推广",
  rolloutPause: "暂停",
  rolloutResume: "继续",
  rolloutStoppedBy: (machine, what) => `${machine} ${what}，推广已停止`,
  rolloutStoppedWhy: "此版本机器的故障比例高于其余机器",
  troubles: zhTroubles,
  auto: "自动更新",
  hubOld: "中枢为旧版本，更新中枢后方可在此管理更新",
  unread: "无法读取中枢的更新状态",
  updateTo: "更新到…",
  ways: "更新方式",
  cancel: "撤销安排",
  repair: "修复",
  cancelRepair: "撤销修复",
  bar: { updateTo: "更新到…", ways: "方式", cancel: "撤销", repair: "修复", cancelRepair: "撤销修复" },
  repairing: (layer) => `正在修复：${layer}`,
  repairDue: (by) => `已安排：上线后修复（${by}）`,
  repairDueBlocked: (why) => `已安排修复，但${why}`,
  repairFailed: "上次修复未完成",
  repairWords: zhRepair,
  notScheduled: (why) => `未能安排：${why}`,
  reasons: {
    not_owner: "无权管理此机器",
    visitor: "访客的机器仅归其本人",
    phone: "手机需在本机更新",
    too_old: "版本过旧，仅可更新到最新版",
    development: "开发版不参与更新",
    offline: "离线",
    unknown_version: "运行版本未知",
    missing_machine: "该机器已不在网络中",
    no_package: "此版本无对应安装包",
    not_newer: "已是此版本或更高版本",
    unavailable: "此版本暂不可用",
    other: "无法执行",
  },
  failures: {
    no_package: "此版本无对应安装包",
    signature: "安装包签名不匹配",
    disk: "磁盘无法写入",
    not_newer: "不高于当前运行版本",
    withdrawn: "网络已撤回此版本",
    sources: "所有来源均无法获取此版本",
    installer: "安装程序执行失败",
    other: "安装失败",
  },
};

export const enUpdateView: UpdateViewWords = {
  problems: enProblems,
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
  failed: (version, why) => `Did not update to ${version}: ${why}`,
  unknown: "Not known yet what it runs",
  group: (newest, going, behind, failed) => [newest ? `${newest} up to date` : "", going ? `${going} on the way` : "", behind ? `${behind} can update` : "", failed ? `${failed} did not update` : ""].filter(Boolean).join(", "),
  runs: (version) => `Runs ${version}`,
  hubMark: "Hub",
  minimumMark: "Minimum",
  older: "Older",
  machinesOn: (count, version) => `${count} on ${version}`,
  policy: "Network updates",
  minimum: "Network requires at least",
  noMinimum: "None",
  managedAuto: "Managed machines update by themselves",
  rollout: "Staged rollout",
  noRollout: "All at once",
  rolloutPaused: "Paused",
  rolloutStopped: "Stopped",
  rolloutMark: (share) => `Rollout ${share}%`,
  withdrawnMark: "Withdrawn",
  rolloutVersion: "Version",
  rolloutAnother: "Another version",
  rolloutShare: "Other machines",
  rolloutFirst: "First",
  rolloutFind: "Find machines",
  rolloutFound: "No machines match",
  rolloutStart: "Start rollout",
  rolloutSave: "Save",
  rolloutEnd: "End rollout",
  rolloutPause: "Pause",
  rolloutResume: "Resume",
  rolloutStoppedBy: (machine, what) => `${machine}: ${what.toLowerCase()}. The rollout stopped`,
  rolloutStoppedWhy: "Machines on this version had trouble more often than the rest",
  troubles: enTroubles,
  auto: "Update automatically",
  hubOld: "The hub runs an older version; updates can be managed here once it is updated",
  unread: "Could not read the hub's updates",
  updateTo: "Update to…",
  ways: "Update settings",
  cancel: "Cancel scheduled",
  repair: "Repair",
  cancelRepair: "Cancel repair",
  bar: { updateTo: "Update", ways: "Settings", cancel: "Cancel", repair: "Repair", cancelRepair: "Cancel repair" },
  repairing: (layer) => `Repairing: ${layer}`,
  repairDue: (by) => `Scheduled: repairs when back (by ${by})`,
  repairDueBlocked: (why) => `A repair is scheduled, but ${why}`,
  repairFailed: "Its last repair did not finish",
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
    unavailable: "This version cannot be had now",
    other: "Cannot be done",
  },
  failures: {
    no_package: "this version has no package for it",
    signature: "the package's signature did not match",
    disk: "the disk could not be written",
    not_newer: "it is not newer than what runs",
    withdrawn: "the network withdrew this version",
    sources: "no source could provide this version",
    installer: "the installer failed",
    other: "it did not install",
  },
};
