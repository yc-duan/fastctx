// The words of new versions: the sidebar's button, its card, the settings'
// rows, the window of versions, another machine's update, and the network's
// policy.

import type { ReachFailure } from "../world/updates";
import { enRepair, zhRepair, type RepairWords } from "./repair-words";
import { enProblems, failure, zhProblems } from "./problem";

/** The window of versions (`version-window.ts`). */
export interface VersionWords {
  title: string;
  close: string;
  /** What `who` runs: none for this device. */
  runs(who: string | null, version: string): string;
  filterLabel: string;
  filter: { all: string; stable: string; test: string };
  /** A test release's tag. */
  test: string;
  /** A release past the major version of the hubs, and why it matters. */
  pastHub: string;
  pastHubWhy: string;
  /** The day a release came, written as a day of the year. */
  day(iso: string): string;
  size(bytes: number): string;
  running: string;
  pick: string;
  picking: string;
  /** A newer release whose package for that machine cannot be had now. */
  unavailable: string;
  none: string;
  looking: string;
  looked(iso: string): string;
  /** GitHub did not answer; what follows is what the network has. */
  unreachable: string;
  lookAgain: string;
  failed(why: string): string;
  /** Picking the network's lowest version. */
  minimumTitle: string;
  minimumNow(version: string | null): string;
  ask: string;
  asked: string;
  askNone: string;
}

const ago = (iso: string, zh: boolean): string => {
  const minutes = Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 60_000));
  if (minutes < 1) return zh ? "刚才查过" : "Checked just now";
  if (minutes < 60) return zh ? `查于 ${minutes} 分钟前` : `Checked ${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return zh ? `查于 ${hours} 小时前` : `Checked ${hours} h ago`;
  const days = Math.round(hours / 24);
  return zh ? `查于 ${days} 天前` : `Checked ${days} days ago`;
};

const megabytes = (bytes: number): string => `${(bytes / 1_000_000).toFixed(1)} MB`;

const clock = (at: Date): string => `${at.getHours()}:${String(at.getMinutes()).padStart(2, "0")}`;

export const zhVersions: VersionWords = {
  title: "版本",
  close: "关闭",
  runs: (who, version) => (who === null ? `这台在跑 ${version}` : `${who} 在跑 ${version}`),
  filterLabel: "筛选版本",
  filter: { all: "全部", stable: "正式版", test: "测试版" },
  test: "测试版",
  pastHub: "超出中枢的大版本",
  pastHubWhy: "中枢还在更早的大版本上：自动更新不会换到这一版，免得连不上网络；手动挑照样可以",
  day: (iso) => {
    const day = new Date(iso);
    const year = day.getFullYear() === new Date().getFullYear() ? "" : `${day.getFullYear()}年`;
    return `${year}${day.getMonth() + 1}月${day.getDate()}日`;
  },
  size: megabytes,
  running: "正在用",
  pick: "更新到这一版",
  picking: "正在开始…",
  unavailable: "现在拿不到",
  none: "还没查到任何版本",
  looking: "正在查…",
  looked: (iso) => ago(iso, true),
  unreachable: "连不上 GitHub，下面是网络里拿得到的版本",
  lookAgain: "重新查",
  failed: failure(zhProblems, "没有做成"),
  minimumTitle: "网络要求的最低版本",
  minimumNow: (version) => (version ? `现在要求至少 ${version}` : "现在不要求最低版本"),
  ask: "设为最低",
  asked: "当前要求",
  askNone: "不要求最低版本",
};

export const enVersions: VersionWords = {
  title: "Versions",
  close: "Close",
  runs: (who, version) => (who === null ? `This device runs ${version}` : `${who} runs ${version}`),
  filterLabel: "Filter versions",
  filter: { all: "All", stable: "Stable", test: "Beta" },
  test: "Beta",
  pastHub: "Past the hub's major version",
  pastHubWhy:
    "The hub is on an earlier major version: automatic updates do not take this one, lest the machine no longer reach the network; picking it yourself still works",
  day: (iso) => {
    const day = new Date(iso);
    const options: Intl.DateTimeFormatOptions =
      day.getFullYear() === new Date().getFullYear() ? { month: "short", day: "numeric" } : { year: "numeric", month: "short", day: "numeric" };
    return day.toLocaleDateString("en", options);
  },
  size: megabytes,
  running: "In use",
  pick: "Update to this",
  picking: "Starting…",
  unavailable: "Not available now",
  none: "No versions found yet",
  looking: "Checking…",
  looked: (iso) => ago(iso, false),
  unreachable: "Can't reach GitHub; these are the versions the network has",
  lookAgain: "Check again",
  failed: failure(enProblems, "Couldn't do that"),
  minimumTitle: "The network's minimum version",
  minimumNow: (version) => (version ? `At least ${version} is asked for now` : "No minimum is asked for now"),
  ask: "Set as minimum",
  asked: "Asked for now",
  askNone: "Ask for no minimum",
};

export interface UpdateWords {
  /** The window of versions. */
  versions: VersionWords;
  /** The sidebar's button. */
  bar: string;
  barFetching(percent: number | null): string;
  barReady: string;
  barInstalling: string;
  /** The card: its title with a new version, and with none; the
   *  settings' button that opens it. */
  title: string;
  here: string;
  open: string;
  close: string;
  named(version: string): string;
  beta: string;
  size(megabytes: string): string;
  viaNetwork: string;
  go: string;
  restart: string;
  retry: string;
  repair: RepairWords;
  fetching: string;
  waiting: string;
  installing: string;
  phoneAsks: string;
  permission: string;
  minimum(version: string): string;
  failed(why: string): string;
  /** A setting of this device's updates not changed, the reason under the pointer. */
  notSet: string;
  /** The settings' row, and the card's row of the version here. */
  updates: string;
  version: string;
  current(version: string): string;
  look: string;
  looking: string;
  latest: string;
  available(version: string): string;
  unreachable: string;
  heardFromNetwork: string;
  /** How the last look on GitHub went, where it did not: limited until
   *  when, an error, or why it could not be reached; when it looks again. */
  rateLimited(until: Date | null): string;
  githubError(status: number): string;
  checkFailed: string;
  reach: Record<ReachFailure, string>;
  nextCheck(at: Date): string;
  /** The card's word on the network's machines this device's accounts may
   *  update that can take a newer version, opening the machines' updates. */
  others(count: number): string;
  auto: string;
  /** Beside automatic updates the owner never chose. */
  autoByDefault: string;
  on: string;
  off: string;
  /** Passing packages on, said short, and whole under the pointer. */
  relay: string;
  relayWhole: string;
  /** Another machine. */
  updateMachine: string;
  machineBehind(version: string): string;
  machineUpdating: string;
  /** The network's policy. */
  policy: string;
  minimumField: string;
  minimumNone: string;
  minimumPick: string;
  managedAuto: string;
  save: string;
}

export const zhUpdate: UpdateWords = {
  versions: zhVersions,
  bar: "新版本",
  barFetching: (percent) => (percent === null ? "下载中" : `下载中 ${percent}%`),
  barReady: "重启以更新",
  barInstalling: "正在更新",
  title: "新版本",
  here: "更新",
  open: "更新…",
  close: "关闭",
  named: (version) => `Fastctx ${version}`,
  beta: "测试版",
  size: (megabytes) => `${megabytes} MB`,
  viaNetwork: "经网络下载",
  go: "更新",
  restart: "重启并更新",
  retry: "重试",
  repair: zhRepair,
  fetching: "下载中",
  waiting: "等手上的活告一段落",
  installing: "正在安装，应用会自己重新打开",
  phoneAsks: "系统会请你确认安装",
  permission: "在系统设置里允许 Fastctx 安装应用，回来再点更新",
  minimum: (version) => `网络要求至少 ${version}`,
  failed: (why) => `没能更新：${why}`,
  notSet: "没改成",
  updates: "更新",
  version: "版本",
  current: (version) => `Fastctx ${version}`,
  look: "检查更新",
  looking: "正在检查…",
  latest: "已是最新",
  available: (version) => `有新版本 ${version}`,
  unreachable: "连不上 GitHub",
  heardFromNetwork: "经网络得知",
  rateLimited: (until) => (until ? `GitHub 限流，${clock(until)} 以后再查` : "GitHub 限流，稍后再查"),
  githubError: (status) => `GitHub 出了错（${status}）`,
  checkFailed: "没查成",
  reach: { dns: "找不到 GitHub 的地址", connect: "连不上 GitHub", tls: "和 GitHub 的安全连接没建起来", timeout: "连 GitHub 超时", interrupted: "和 GitHub 的连接断了", proxy: "代理没连通", other: "连不上 GitHub" },
  nextCheck: (at) => `${clock(at)} 再查`,
  others: (count) => `网络里还有 ${count} 台能更新`,
  auto: "自动更新",
  autoByDefault: "默认",
  on: "开",
  off: "关",
  relay: "帮网络传包",
  relayWhole: "帮网络里的设备传更新包",
  updateMachine: "更新",
  machineBehind: (version) => `${version} 可用`,
  machineUpdating: "正在更新…",
  policy: "网络的更新",
  minimumField: "最低版本",
  minimumNone: "不要求",
  minimumPick: "挑一版",
  managedAuto: "受管机器自动更新",
  save: "保存",
};

export const enUpdate: UpdateWords = {
  versions: enVersions,
  bar: "New version",
  barFetching: (percent) => (percent === null ? "Downloading" : `Downloading ${percent}%`),
  barReady: "Restart to update",
  barInstalling: "Updating",
  title: "New version",
  here: "Updates",
  open: "Updates…",
  close: "Close",
  named: (version) => `Fastctx ${version}`,
  beta: "Beta",
  size: (megabytes) => `${megabytes} MB`,
  viaNetwork: "Downloaded through the network",
  go: "Update",
  restart: "Restart and update",
  retry: "Try again",
  repair: enRepair,
  fetching: "Downloading",
  waiting: "Waiting for the work under way",
  installing: "Installing; the app opens again by itself",
  phoneAsks: "The system will ask you to confirm",
  permission: "Allow Fastctx to install apps in the system settings, then tap Update again",
  minimum: (version) => `The network asks for at least ${version}`,
  failed: (why) => `Couldn't update: ${why}`,
  notSet: "Couldn't change it",
  updates: "Updates",
  version: "Version",
  current: (version) => `Fastctx ${version}`,
  look: "Check for updates",
  looking: "Checking…",
  latest: "Up to date",
  available: (version) => `Version ${version} available`,
  unreachable: "Can't reach GitHub",
  heardFromNetwork: "Heard from the network",
  rateLimited: (until) => (until ? `GitHub is rate-limiting; checks again after ${clock(until)}` : "GitHub is rate-limiting; checks again later"),
  githubError: (status) => `GitHub returned an error (${status})`,
  checkFailed: "Couldn't check",
  reach: { dns: "Can't find GitHub's address", connect: "Can't reach GitHub", tls: "Couldn't set up a secure connection to GitHub", timeout: "GitHub timed out", interrupted: "The connection to GitHub broke", proxy: "The proxy didn't connect", other: "Can't reach GitHub" },
  nextCheck: (at) => `Checks again at ${clock(at)}`,
  others: (count) => `${count} more in the network can update`,
  auto: "Automatic updates",
  autoByDefault: "By default",
  on: "On",
  off: "Off",
  relay: "Pass packages on",
  relayWhole: "Pass updates to the network's devices",
  updateMachine: "Update",
  machineBehind: (version) => `${version} available`,
  machineUpdating: "Updating…",
  policy: "Network updates",
  minimumField: "Minimum version",
  minimumNone: "None",
  minimumPick: "Pick one",
  managedAuto: "Managed machines update by themselves",
  save: "Save",
};
