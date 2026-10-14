// The words of a machine's own page (`world-gui.md`, 机器的详情): short
// labels and numbers with their units; what needs a sentence stands behind
// a pointer's rest.

import { failure, enProblems, zhProblems } from "./problem";

export interface MachinePageWords {
  /** The page's name where the machine is not known yet. */
  title: string;
  rename: string;
  renameTitle(machine: string): string;
  renameSave: string;
  cancel: string;
  /** Under the field: what happens to the name it had. */
  renameKeeps: string;
  /** The whole of it, behind the pointer. */
  renameMore: string;
  /** An offline machine takes its new name when it connects next. */
  renameOffline(machine: string): string;
  renamed(from: string, to: string): string;
  renameFailed(why: string): string;
  /** The way to the drawing, with this machine chosen on it. */
  onMap: string;
  /** No machine of this name is reached from here. */
  gone: string;
  /** The names it had, each still naming it. */
  formerly(names: string[]): string;
  /** Hubs that still call this computer by an old name, until they follow. */
  notFollowed(hubs: string[]): string;

  cpu: string;
  memory: string;
  gpu: string;
  network: string;
  /** The section listing each network card, apart from the network the
   *  machine is in. */
  cards: string;
  disks: string;
  connection: string;
  about: string;
  present: string;
  /** Agents of other accounts at work or waiting on it, told its owner
   *  without their names. */
  othersAtWork(count: number): string;
  sharing: string;

  threads(count: number): string;
  load(one: string, five: string, fifteen: string): string;
  /** The highest the trace of the last minutes reached. */
  peak(percent: string): string;
  /** The page file a Windows machine sets aside, or a Linux one's swap. */
  swap(windows: boolean, used: string, total: string): string;
  vram(used: string, total: string): string;
  of(used: string, total: string): string;
  free(free: string, total: string): string;
  virtualCard: string;
  cardOff: string;
  /** Bytes a card moved since it came up. */
  moved(down: string, up: string): string;
  upFor: string;
  bootAt(stamp: string): string;
  nodeFor(span: string): string;
  service: string;
  noService: string;

  lineUp(span: string): string;
  lineDown(span: string): string;
  lineVia(through: string): string;
  available(percent: string): string;
  drops(count: number): string;
  longest(span: string): string;
  roundTrip(median: string, highest: string): string;
  ranges: { day: string; week: string; month: string };
  /** A drop on the line's strip, for the pointer. */
  wentDown(at: string, span: string, why: string): string;
  stillDown(at: string, why: string): string;
  started(at: string): string;
  notRecorded: string;
  noLines: string;

  kernel: string;
  host: string;
  home: string;
  booted: string;
  version: string;
  /** Where it stands in each network it is in. */
  networks: string;
  /** The group its owner put it in (`world-network.md`, 机器的分组). */
  group: string;

  reading: string;
  failed: string;
  older: string;
  offline: string;
  lastRead(ago: string): string;

  /** "3 天 7 小时", the two largest units that are not zero. */
  span(ms: number): string;
  /** A day and time, the date only when not today. */
  stamp(at: Date, now: Date): string;
  /** Hour marks under a day's strip, days under longer ones. */
  hour(at: Date): string;
  day(at: Date): string;
}

const pad = (n: number) => String(n).padStart(2, "0");
const clock = (at: Date) => `${pad(at.getHours())}:${pad(at.getMinutes())}`;
const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();

/** The two largest whole units of `ms`. */
function units(ms: number): { days: number; hours: number; minutes: number; seconds: number } {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return { days: Math.floor(seconds / 86400), hours: Math.floor(seconds / 3600) % 24, minutes: Math.floor(seconds / 60) % 60, seconds: seconds % 60 };
}

export const zhMachinePage: MachinePageWords = {
  title: "机器",
  rename: "重命名",
  renameTitle: (machine) => `重命名 ${machine}`,
  renameSave: "保存",
  cancel: "取消",
  renameKeeps: "旧名字仍指向它",
  renameMore:
    "旧名字留作它的别名：脚本、记录和别的机器上写着旧名字的地方照样找得到它，别的机器也不能再用这个名字。它离线的话，下次连上中枢服务器就换成新名字。",
  renameOffline: (machine) => `${machine} 离线，下次连上就换成新名字`,
  renamed: (from, to) => `已把 ${from} 改名为 ${to}`,
  renameFailed: failure(zhProblems, "改名失败"),
  onMap: "在图上查看",
  gone: "找不到这台机器",
  formerly: (names) => `曾用名 ${names.join("、")}`,
  notFollowed: (hubs) => `${hubs.join("、")} 上仍是旧名字，连上后跟上`,

  cpu: "处理器",
  memory: "内存",
  gpu: "显卡",
  network: "网络",
  cards: "网卡",
  disks: "磁盘",
  connection: "连线",
  about: "关于",
  present: "在这台上",
  othersAtWork: (count) => `另有 ${count} 个在跑`,
  sharing: "共享与协助",

  threads: (count) => `${count} 线程`,
  load: (one, five, fifteen) => `负载 ${one} · ${five} · ${fifteen}`,
  peak: (percent) => `近两分钟最高 ${percent}`,
  swap: (windows, used, total) => `${windows ? "已提交" : "交换"} ${used} / ${total}`,
  vram: (used, total) => `显存 ${used} / ${total}`,
  of: (used, total) => `${used} / ${total}`,
  free: (free, total) => `可用 ${free} · 共 ${total}`,
  virtualCard: "虚拟",
  cardOff: "未连接",
  moved: (down, up) => `累计 ↓ ${down} ↑ ${up}`,
  upFor: "已开机",
  bootAt: (stamp) => `开机于 ${stamp}`,
  nodeFor: (span) => `本机服务已运行 ${span}`,
  service: "开机服务",
  noService: "随登录运行",

  lineUp: (span) => `已连接 ${span}`,
  lineDown: (span) => `已断开 ${span}`,
  lineVia: (through) => `经 ${through}`,
  available: (percent) => `在线 ${percent}`,
  drops: (count) => (count ? `断开 ${count} 次` : "未断开"),
  longest: (span) => `最长 ${span}`,
  roundTrip: (median, highest) => `往返时延：中位 ${median} · 最高 ${highest}`,
  ranges: { day: "24 小时", week: "7 天", month: "30 天" },
  wentDown: (at, span, why) => `${at} 断开 ${span}${why ? `：${why}` : ""}`,
  stillDown: (at, why) => `${at} 起断开${why ? `：${why}` : ""}`,
  started: (at) => `${at} 本机服务启动`,
  notRecorded: "没有记录",
  noLines: "不在任何网络里",

  kernel: "内核",
  host: "主机名",
  home: "主目录",
  booted: "开机于",
  version: "Fastctx",
  networks: "网络",
  group: "分组",

  reading: "读取中…",
  failed: "读取失败",
  older: "这台的 Fastctx 较旧，更新后才能看到它此刻的状态",
  offline: "离线",
  lastRead: (ago) => `最后读取于 ${ago}`,

  span(ms) {
    const { days, hours, minutes, seconds } = units(ms);
    if (days) return hours ? `${days} 天 ${hours} 小时` : `${days} 天`;
    if (hours) return minutes ? `${hours} 小时 ${minutes} 分` : `${hours} 小时`;
    if (minutes) return `${minutes} 分钟`;
    return `${seconds} 秒`;
  },
  stamp: (at, now) => (sameDay(at, now) ? clock(at) : `${at.getMonth() + 1}月${at.getDate()}日 ${clock(at)}`),
  hour: (at) => clock(at),
  day: (at) => `${at.getMonth() + 1}/${at.getDate()}`,
};

export const enMachinePage: MachinePageWords = {
  title: "Machine",
  rename: "Rename",
  renameTitle: (machine) => `Rename ${machine}`,
  renameSave: "Save",
  cancel: "Cancel",
  renameKeeps: "Its old name still points to it",
  renameMore:
    "The old name stays as an alias: scripts, records and other machines that wrote it still find this machine, and no other machine can take it. If it is offline, it takes the new name the next time it connects to the hub.",
  renameOffline: (machine) => `${machine} is offline; it takes the new name when it connects`,
  renamed: (from, to) => `Renamed ${from} to ${to}`,
  renameFailed: failure(enProblems, "Couldn't rename"),
  onMap: "Show on the map",
  gone: "No such machine here",
  formerly: (names) => `Formerly ${names.join(", ")}`,
  notFollowed: (hubs) => `Still the old name on ${hubs.join(", ")} until it connects`,

  cpu: "Processor",
  memory: "Memory",
  gpu: "Graphics",
  network: "Network",
  cards: "Network cards",
  disks: "Disks",
  connection: "Connection",
  about: "About",
  present: "On it",
  othersAtWork: (count) => `${count} more running`,
  sharing: "Sharing",

  threads: (count) => `${count} threads`,
  load: (one, five, fifteen) => `load ${one} · ${five} · ${fifteen}`,
  peak: (percent) => `peak ${percent} in 2 min`,
  swap: (windows, used, total) => `${windows ? "committed" : "swap"} ${used} / ${total}`,
  vram: (used, total) => `VRAM ${used} / ${total}`,
  of: (used, total) => `${used} / ${total}`,
  free: (free, total) => `${free} free of ${total}`,
  virtualCard: "virtual",
  cardOff: "disconnected",
  moved: (down, up) => `in all ↓ ${down} ↑ ${up}`,
  upFor: "Up for",
  bootAt: (stamp) => `Started ${stamp}`,
  nodeFor: (span) => `Local service up for ${span}`,
  service: "Starts with the computer",
  noService: "Runs while signed in",

  lineUp: (span) => `connected for ${span}`,
  lineDown: (span) => `down for ${span}`,
  lineVia: (through) => `via ${through}`,
  available: (percent) => `${percent} online`,
  drops: (count) => (count ? `${count} drop${count === 1 ? "" : "s"}` : "no drops"),
  longest: (span) => `longest ${span}`,
  roundTrip: (median, highest) => `Round trip: median ${median} · highest ${highest}`,
  ranges: { day: "24 h", week: "7 days", month: "30 days" },
  wentDown: (at, span, why) => `Down at ${at} for ${span}${why ? `: ${why}` : ""}`,
  stillDown: (at, why) => `Down since ${at}${why ? `: ${why}` : ""}`,
  started: (at) => `Local service started ${at}`,
  notRecorded: "Not recorded",
  noLines: "In no network",

  kernel: "Kernel",
  host: "Host name",
  home: "Home",
  booted: "Started",
  version: "Fastctx",
  networks: "Networks",
  group: "Group",

  reading: "Reading…",
  failed: "Couldn't read",
  older: "Its Fastctx is older; update it to see how it is doing",
  offline: "Offline",
  lastRead: (ago) => `Last read ${ago}`,

  span(ms) {
    const { days, hours, minutes, seconds } = units(ms);
    if (days) return hours ? `${days}d ${hours}h` : `${days}d`;
    if (hours) return minutes ? `${hours}h ${minutes}m` : `${hours}h`;
    if (minutes) return `${minutes} min`;
    return `${seconds} s`;
  },
  stamp: (at, now) => (sameDay(at, now) ? clock(at) : `${at.toLocaleDateString("en", { month: "short", day: "numeric" })} ${clock(at)}`),
  hour: (at) => clock(at),
  day: (at) => `${at.getMonth() + 1}/${at.getDate()}`,
};
