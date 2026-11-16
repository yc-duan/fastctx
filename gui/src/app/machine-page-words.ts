// The words of a machine's own page (`world-gui.md`, 机器的详情): short
// labels and numbers with their units; what needs a sentence stands behind
// a pointer's rest.

import { failure, enProblems, zhProblems } from "./problem";
import { enBound, zhBound } from "./trouble-words";

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
  /** The machine's own jobs, its owner's, where the owner is not known. */
  ownJobs: string;
  /** One account's jobs on it, on hover: how many run, and the most it
   *  may run at once when its owner set one. */
  jobsOf(who: string, running: number, most: number | null): string;
  sharing: string;
  /** What the machine keeps free in memory for its agents: the setting's
   *  name, its choices, and the line on the memory's ring, held. */
  room: string;
  roomNone: string;
  roomKept(size: string): string;
  /** Turns waiting for that room now. */
  waitingMemory(count: number): string;
  roomFailed(why: string): string;
  /** How far it is toward its system's limits over the last day: the
   *  part's name, the span, each limit's name and why it matters, held,
   *  and how much of it is used. */
  limits: string;
  limitsSpan: string;
  /** This computer's Fastctx, as its node says, when there is something to
   *  say: held up or stuck, the rules it broke, a bound near its limit and
   *  the scenes it left; what each means behind the pointer. */
  health: string;
  healthState: string;
  healthBusy: string;
  healthBusyWhy(ms: number): string;
  healthStuck(count: number): string;
  healthStuckWhy: string;
  healthBroken: string;
  healthBrokenCount(count: number): string;
  healthNear: string;
  healthNearUsed(bound: string, used: number): string;
  healthScenes: string;
  healthScenesCount(count: number): string;
  healthScenesWhy: string;
  healthReveal: string;
  limit: Record<"inotify_instances" | "inotify_watches" | "open_files" | "tasks", { name: string; why: string }>;
  limitUsed(used: string, limit: string): string;

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
  /** Its name, among what can be done to it. */
  name: string;
  /** How long its node has run, among its facts. */
  node: string;
  /** Its direct links, in 「网络」. */
  direct: string;
  /** The three tabs, and the tab list as a whole. */
  tabs: { now: string; net: string; manage: string };
  tabsLabel: string;
  /** The Fastctx on it, on its identity line. */
  fastctxRuns(version: string): string;
  fastctxLatest: string;
  fastctxBehind(newer: string): string;
  /** Work of its own that is stuck, and for how long; what the node says of
   *  it is a hover away. */
  stuck(span: string): string;
  /** Its usage ledger keeps rows it could not read aside; its totals go
   *  without them. */
  ledgerRows(rows: number): string;
  /** Its usage ledger cannot be read or written now. */
  ledgerBroken: string;
  /** Another of its own warnings, its words a hover away. */
  healthOther: string;
  /** A place macOS kept private from it: on that Mac with the way to allow
   *  Fastctx in, elsewhere where it is allowed. */
  privacyHere(path: string): string;
  privacyThere(path: string): string;
  privacyGo: string;

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
  renameKeeps: "旧名称仍指向此机器",
  renameMore:
    "旧名称保留为别名：脚本、记录与其他机器中的旧名称仍指向此机器，其他机器不能再使用该名称。离线时，下次连接中枢服务器后改用新名称。",
  renameOffline: (machine) => `${machine} 离线，下次连接后改用新名称`,
  renamed: (from, to) => `已将 ${from} 改名为 ${to}`,
  renameFailed: failure(zhProblems, "改名失败"),
  onMap: "在图上查看",
  gone: "找不到该机器",
  formerly: (names) => `曾用名 ${names.join("、")}`,
  notFollowed: (hubs) => `${hubs.join("、")} 上仍为旧名称，连接后同步`,

  cpu: "处理器",
  memory: "内存",
  gpu: "显卡",
  cards: "网卡",
  disks: "磁盘",
  connection: "连线",
  about: "资料",
  present: "本机",
  othersAtWork: (count) => `另有 ${count} 个运行中`,
  ownJobs: "主人",
  jobsOf: (who, running, most) => `${who} 运行 ${running} 条作业${most ? `，上限 ${most} 条` : ""}`,
  sharing: "共享与协助",
  room: "内存余量",
  roomNone: "不保留",
  roomKept: (size) => `为智能体保留 ${size}：可用内存低于此值时，新一轮排队等待`,
  waitingMemory: (count) => `${count} 轮等待内存`,
  roomFailed: failure(zhProblems, "更改失败"),
  limits: "上限",
  limitsSpan: "近 24 小时",
  health: "Fastctx",
  healthState: "状态",
  healthBusy: "繁忙",
  healthBusyWhy: (ms) => `近一分钟内，任务最长等待 ${(ms / 1000).toFixed(1)} 秒`,
  healthStuck: (count) => `卡住 ${count} 件`,
  healthStuckWhy: "超过 30 秒无进展",
  healthBroken: "违规",
  healthBrokenCount: (count) => `${count.toLocaleString()} 次`,
  healthNear: "接近上限",
  healthNearUsed: (bound, used) => `${zhBound(bound)} ${used}%`,
  healthScenes: "现场",
  healthScenesCount: (count) => `${count} 份`,
  healthScenesWhy: "卡住时自动记录的现场，报告问题时附上",
  healthReveal: "在文件夹中显示",
  limit: {
    inotify_instances: { name: "inotify 实例", why: "该用户的程序合计可打开的 inotify 实例；用尽后无法新建监视，对话记录无法读取" },
    inotify_watches: { name: "inotify 监视", why: "该用户的程序合计可监视的文件与文件夹" },
    open_files: { name: "打开的文件", why: "本机服务可同时打开的文件与连接" },
    tasks: { name: "线程", why: "该用户可同时运行的线程" },
  },
  limitUsed: (used, limit) => `${used} / ${limit}`,

  threads: (count) => `${count} 线程`,
  load: (one, five, fifteen) => `负载 ${one}　${five}　${fifteen}`,
  peak: (percent) => `近两分钟最高 ${percent}`,
  swap: (windows, used, total) => `${windows ? "已提交" : "交换"} ${used} / ${total}`,
  vram: (used, total) => `显存 ${used} / ${total}`,
  of: (used, total) => `${used} / ${total}`,
  free: (free, total) => `可用 ${free}，共 ${total}`,
  virtualCard: "虚拟",
  cardOff: "未连接",
  moved: (down, up) => `累计 ↓ ${down} ↑ ${up}`,
  upFor: "已开机",
  bootAt: (stamp) => `开机于 ${stamp}`,
  nodeFor: (span) => `已运行 ${span}`,
  service: "开机服务",
  noService: "随登录运行",

  lineUp: (span) => `已连接 ${span}`,
  lineDown: (span) => `已断开 ${span}`,
  lineVia: (through) => `经 ${through}`,
  available: (percent) => `在线 ${percent}`,
  drops: (count) => (count ? `断开 ${count} 次` : "未断开"),
  longest: (span) => `最长 ${span}`,
  roundTrip: (median, highest) => `往返时延：中位 ${median}，最高 ${highest}`,
  ranges: { day: "24 小时", week: "7 天", month: "30 天" },
  wentDown: (at, span, why) => `${at} 断开 ${span}${why ? `：${why}` : ""}`,
  stillDown: (at, why) => `${at} 起断开${why ? `：${why}` : ""}`,
  started: (at) => `${at} 本机服务启动`,
  notRecorded: "没有记录",
  noLines: "不在任何网络中",

  kernel: "系统内核",
  host: "主机名",
  home: "主目录",
  booted: "开机于",
  version: "Fastctx",
  networks: "所在网络",
  group: "分组",
  name: "名称",
  node: "本机服务",
  direct: "直连",
  tabs: { now: "状况", net: "网络", manage: "管理" },
  tabsLabel: "机器详情",
  fastctxRuns: (version) => `Fastctx ${version}`,
  fastctxLatest: "已是最新",
  fastctxBehind: (newer) => `可更新到 ${newer}`,
  stuck: (span) => `有任务卡住 ${span}`,
  ledgerRows: (rows) => `用量账本有 ${rows} 行无法读取，已另存，统计不含这些行`,
  ledgerBroken: "用量账本暂时无法读写，恢复前暂停协助",
  healthOther: "有一条提醒",
  privacyHere: (path) => `macOS 未允许 Fastctx 访问 ${path}：在「完全磁盘访问权限」中开启 Fastctx；授权一次，更新后仍有效`,
  privacyThere: (path) => `macOS 未允许 Fastctx 访问 ${path}：在该 Mac 的「系统设置 › 隐私与安全性 › 完全磁盘访问权限」中开启 Fastctx`,
  privacyGo: "授权",

  reading: "读取中…",
  failed: "读取失败",
  older: "该机器的 Fastctx 版本较旧，更新后可查看当前状态",
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
    "The old name stays as an alias for scripts, records and other machines, and no other machine can take it. If offline, the machine takes the new name when it next connects to the hub.",
  renameOffline: (machine) => `${machine} is offline; it takes the new name when it connects`,
  renamed: (from, to) => `Renamed ${from} to ${to}`,
  renameFailed: failure(enProblems, "Could not rename"),
  onMap: "Show on the map",
  gone: "Machine not found",
  formerly: (names) => `Formerly ${names.join(", ")}`,
  notFollowed: (hubs) => `Still the old name on ${hubs.join(", ")} until it connects`,

  cpu: "Processor",
  memory: "Memory",
  gpu: "Graphics",
  cards: "Network cards",
  disks: "Disks",
  connection: "Connection",
  about: "Details",
  present: "This machine",
  othersAtWork: (count) => `${count} more running`,
  ownJobs: "Owner",
  jobsOf: (who, running, most) => `${who} runs ${running} job${running === 1 ? "" : "s"}${most ? `, at most ${most}` : ""}`,
  sharing: "Sharing",
  room: "Memory reserve",
  roomNone: "None",
  roomKept: (size) => `${size} kept for agents: below it, new turns wait their turn`,
  waitingMemory: (count) => `${count} turn${count === 1 ? "" : "s"} waiting for memory`,
  roomFailed: failure(enProblems, "Not changed"),
  limits: "Limits",
  limitsSpan: "Last 24 hours",
  health: "Fastctx",
  healthState: "State",
  healthBusy: "Busy",
  healthBusyWhy: (ms) => `In the last minute, work waited up to ${(ms / 1000).toFixed(1)} s for its turn`,
  healthStuck: (count) => `${count} stuck`,
  healthStuckWhy: "No progress for over 30 seconds",
  healthBroken: "Rules broken",
  healthBrokenCount: (count) => `${count.toLocaleString()} times`,
  healthNear: "Near a limit",
  healthNearUsed: (bound, used) => `${enBound(bound)} ${used}%`,
  healthScenes: "Scenes",
  healthScenesCount: (count) => `${count} kept`,
  healthScenesWhy: "What was going on when work got stuck, written down by itself; attach it when reporting a problem",
  healthReveal: "Show in folder",
  limit: {
    inotify_instances: { name: "inotify instances", why: "What this user's programs may open together; used up, no new watch opens and conversations' records are not read" },
    inotify_watches: { name: "inotify watches", why: "The files and folders this user's programs may watch together" },
    open_files: { name: "Open files", why: "The files and connections the node itself may hold open" },
    tasks: { name: "Threads", why: "The threads this user may run at once" },
  },
  limitUsed: (used, limit) => `${used} / ${limit}`,

  threads: (count) => `${count} threads`,
  load: (one, five, fifteen) => `load ${one}, ${five}, ${fifteen}`,
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
  nodeFor: (span) => `Up for ${span}`,
  service: "Starts with the computer",
  noService: "Runs while signed in",

  lineUp: (span) => `connected for ${span}`,
  lineDown: (span) => `down for ${span}`,
  lineVia: (through) => `via ${through}`,
  available: (percent) => `${percent} online`,
  drops: (count) => (count ? `${count} drop${count === 1 ? "" : "s"}` : "no drops"),
  longest: (span) => `longest ${span}`,
  roundTrip: (median, highest) => `Round trip: median ${median}, highest ${highest}`,
  ranges: { day: "24 h", week: "7 days", month: "30 days" },
  wentDown: (at, span, why) => `Down at ${at} for ${span}${why ? `: ${why}` : ""}`,
  stillDown: (at, why) => `Down since ${at}${why ? `: ${why}` : ""}`,
  started: (at) => `Local service started ${at}`,
  notRecorded: "Not recorded",
  noLines: "In no network",

  kernel: "OS kernel",
  host: "Host name",
  home: "Home",
  booted: "Started",
  version: "Fastctx",
  networks: "Networks",
  group: "Group",
  name: "Name",
  node: "Local service",
  direct: "Direct links",
  tabs: { now: "Status", net: "Network", manage: "Manage" },
  tabsLabel: "Machine details",
  fastctxRuns: (version) => `Fastctx ${version}`,
  fastctxLatest: "Up to date",
  fastctxBehind: (newer) => `${newer} available`,
  stuck: (span) => `A task has been stuck for ${span}`,
  ledgerRows: (rows) => `${rows} usage ledger row${rows === 1 ? "" : "s"} could not be read and were set aside; totals leave them out`,
  ledgerBroken: "The usage ledger cannot be read or written now; assistance is paused until it recovers",
  healthOther: "This machine has a warning",
  privacyHere: (path) => `macOS keeps ${path} from Fastctx: turn Fastctx on in Full Disk Access; once is enough, across versions`,
  privacyThere: (path) => `macOS keeps ${path} from Fastctx: turn Fastctx on in System Settings › Privacy & Security › Full Disk Access on that Mac`,
  privacyGo: "Allow",

  reading: "Reading…",
  failed: "Could not read",
  older: "Its Fastctx is older; update it to see its current status",
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
