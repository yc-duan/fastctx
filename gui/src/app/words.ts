// What the frame around every page says: the sidebar, the new conversation,
// the settings. In the user's words (`world-gui.md`, 用词).

import type { AskWords } from "../conversation/asks";
import type { SaidWords } from "../conversation/said";
import type { CommandWords } from "../conversation/commands";
import type { FileWords } from "../conversation/mention";
import type { PresenceWords } from "./presence";
import type { ReaderWords } from "./reader";
import { sizeOf, type AttachWords } from "../conversation/attach";
import { enProblems, failure, zhProblems } from "./problem";

export interface ShellWords {
  app: string;
  world: string;
  projects: string;
  outside: string;
  machines: string;
  agents: string;
  features: string;
  /** The way to every conversation of a list. */
  all: string;
  /** The kind of device a conversation lives on, for a screen reader. */
  device(form: "desktop" | "laptop" | "server"): string;
  settings: string;
  newTalk: string;
  newProject: string;
  edit: string;
  openSidebar: string;
  foldSidebar: string;
  /** "Claude · laptop". */
  someone(agent: string, machine: string): string;
  colleagueFor(asker: string): string;
  busy(lasted: string): string;
  idle: string;
  waiting: string;
  offline: string;
  noProjects: string;
  nobodyHere: string;
  /** The World on this machine could not be asked. */
  unreachable: string;
  /** The World has been asked and has not answered for this many seconds. */
  unanswered(seconds: number): string;
  thisOne: string;
  phone: string;
  /** The way to log in, on a computer logged in nowhere. */
  logIn: string;
  account: string;
  /** Who came into the accounts lately, not yet shown, for a screen reader. */
  arrivals(n: number): string;
  /** A login's part of the sidebar, folded or not. */
  groupOf(login: string): string;
  more(n: number): string;
  span(ms: number): string;
  /** A project's fold button: the agents in it. */
  agentsIn(project: string): string;
  /** The window's caption buttons, where the shell draws them. */
  frame: { minimize: string; maximize: string; restore: string; close: string };
}

export interface NewTalkWords {
  english: boolean;
  title: string;
  /** Which agent, as one of its logins; and choosing another than the one
   *  chosen. */
  agent: string;
  another: string;
  /** The agents' groups: on the user's own machines, on machines of
   *  others they may work on (all shared with them, or not), lent. */
  mine: string;
  others(allShared: boolean): string;
  lent: string;
  ownedBy(owner: string): string;
  noAgents: string;
  /** A login lent, which its machine does not list. */
  lentGone: string;
  /** A login whose usage window is spent goes on with credits. */
  onCredits: string;
  /** Which account it works for. */
  account: string;
  worksOn: string;
  sameMachine: string;
  /** Why a lent agent is asked no machine to work on: the hover of its
   *  lent mark. */
  lentWorks(owner: string): string;
  /** The first use of an agent on another's machine: what it lets that
   *  machine do; `everything` for the super administrator. */
  reaches(owner: string, machine: string, everything: boolean): string;
  /** An agent lent to the account, by whom. */
  lentBy(owner: string): string;
  /** Where a lent agent's files other than images go. */
  filesOn: string;
  /** The hover of the files row's label. */
  filesOnWhat: string;
  noFilePlace: string;
  project: string;
  noProject: string;
  here: string;
  offline: string;
  noMachine: string;
  model: string;
  effort: string;
  defaultModel: string;
  asking: string;
  unsure: string;
  notInstalled: string;
  loggedOut: string;
  /** On Claude, where the user has not yet let it work without asking. */
  needsConsent: string;
  /** The consent sheet's button, when starting follows it. */
  allowAndStart: string;
  firstWords: string;
  hint: string;
  start: string;
  starting: string;
  /** Said while what is attached goes to the machine, before the agent is
   *  asked in. */
  sendingFiles: string;
  /** Said over the sheet while files are dragged onto it. */
  dropHere: string;
  filesNotSent: string;
  cancel: string;
  /** The conversation did not begin; the kernel's reason is its hover. */
  notOpened(why: string): string;
}

export const zhNewTalk: NewTalkWords = {
  english: false,
  title: "新对话",
  agent: "智能体",
  another: "更换",
  mine: "我的机器",
  others: (allShared) => (allShared ? "共享给我的" : "其他机器"),
  lent: "借给我的",
  ownedBy: (owner) => `${owner} 的`,
  noAgents: "没有可用的智能体",
  lentGone: "该登录已不可用",
  onCredits: "使用 credits",
  account: "账号",
  worksOn: "工作机器",
  sameMachine: "同一台",
  lentWorks: (owner) => `借自 ${owner}，在你的机器上工作，不接触 ${owner} 的电脑`,
  reaches: (owner, machine, everything) =>
    everything
      ? `首次使用后，${machine} 上的智能体可访问本服务器的所有机器，${owner} 也将能访问。可在账号页撤销。`
      : `首次使用后，${machine} 上的智能体可访问你的机器，${owner} 也将能访问。可在账号页撤销。`,
  lentBy: (owner) => `借自 ${owner}`,
  filesOn: "文件存放",
  filesOnWhat: "图片留在智能体所在机器，其他文件存放到所选机器",
  noFilePlace: "没有可存放文件的机器",
  project: "项目",
  noProject: "无",
  here: "本机",
  offline: "离线",
  noMachine: "没有在线的机器",
  model: "模型",
  effort: "推理强度",
  defaultModel: "默认",
  asking: "检测中…",
  unsure: "状态未知",
  notInstalled: "未安装",
  loggedOut: "未登录",
  needsConsent: "需要授权",
  allowAndStart: "授权并开始",
  firstWords: "输入第一条消息…",
  hint: "Enter 发送 · Shift+Enter 换行",
  start: "开始",
  starting: "正在启动…",
  sendingFiles: "正在上传文件…",
  dropHere: "拖放到这里",
  filesNotSent: "部分文件上传失败",
  cancel: "关闭",
  notOpened: () => "启动失败",
};

export const enNewTalk: NewTalkWords = {
  english: true,
  title: "New conversation",
  agent: "Agent",
  another: "Change",
  mine: "My machines",
  others: (allShared) => (allShared ? "Shared with me" : "Other machines"),
  lent: "Lent to me",
  ownedBy: (owner) => `${owner}'s`,
  noAgents: "No agents available",
  lentGone: "Login unavailable",
  onCredits: "on credits",
  account: "Account",
  worksOn: "Works on",
  sameMachine: "Same machine",
  lentWorks: (owner) => `Lent by ${owner}; works on your machines, never on ${owner}'s computer`,
  reaches: (owner, machine, everything) =>
    everything
      ? `After first use, agents on ${machine} can access every machine on this server, and so can ${owner}. Revoke anytime on the Account page.`
      : `After first use, agents on ${machine} can access your machines, and so can ${owner}. Revoke anytime on the Account page.`,
  lentBy: (owner) => `lent by ${owner}`,
  filesOn: "Files to",
  filesOnWhat: "Images stay with the agent; other files go to the machine you choose",
  noFilePlace: "No machine to store files",
  project: "Project",
  noProject: "None",
  here: "this computer",
  offline: "offline",
  noMachine: "No machines online",
  model: "Model",
  effort: "Reasoning",
  defaultModel: "default",
  asking: "Checking…",
  unsure: "Status unknown",
  notInstalled: "Not installed",
  loggedOut: "Not logged in",
  needsConsent: "Needs permission",
  allowAndStart: "Allow and start",
  firstWords: "Write the first message…",
  hint: "Enter to send · Shift+Enter for a new line",
  start: "Start",
  starting: "Starting…",
  sendingFiles: "Uploading files…",
  dropHere: "Drop files here",
  filesNotSent: "Some files failed to upload",
  cancel: "Close",
  notOpened: () => "Couldn't start",
};

export const zhPresence: PresenceWords = {
  asked: (who, what) =>
    `${who} ${{ answer: "问你", command: "想运行命令", change: "想改文件", tool: "想用工具", plan: "想按计划动手", form: "等你填写", other: "等你允许", waiting: "等你回复" }[what]}`,
  finished: (who) => `${who} 回复了`,
  open: "打开",
  stop: "停止",
  arrived: (name, device, login) => `新${device ? "设备" : "机器"} ${name} 加入 ${login}`,
  arrivedBody: "如果不是你添加的，请在账号页移除",
};

export const enPresence: PresenceWords = {
  asked: (who, what) =>
    `${who} ${{ answer: "asks", command: "wants to run a command", change: "wants to change files", tool: "wants to use a tool", plan: "wants to start on its plan", form: "needs a form filled in", other: "needs your OK", waiting: "is waiting for your reply" }[what]}`,
  finished: (who) => `${who} replied`,
  open: "Open",
  stop: "Stop",
  arrived: (name, device, login) => `New ${device ? "device" : "machine"} ${name} joined ${login}`,
  arrivedBody: "If you didn't add it, remove it on the Account page",
};

export interface SettingsWords {
  title: string;
  look: string;
  notify: string;
  notifyOn: string;
  notifyOff: string;
  keys: string;
  keyList: [string, string][];
  theme: string;
  system: string;
  themes: Record<"warm-black" | "ink-black" | "warm-white", string>;
  language: string;
  textSize: string;
  sizeDefault: string;
  sizes: Record<string, string>;
  device: string;
  machine: string;
  alone: string;
  link: string;
  /** A hub this computer or device is logged in to. */
  hub(name: string): string;
  /** The accounts it is logged in to there. */
  as(accounts: string): string;
  version: string;
  online(hub: string, since: string): string;
  connecting(error: string | null): string;
  stopped(reason: string): string;
  notInShell: string;
  service: string;
  /** Which World this computer runs, and putting the new one in place. */
  worldVersion: string;
  versionHere(version: string): string;
  /** The node against the program the interface carries; the builds are
   *  the hover. */
  versionDifferent(relation: "older" | "newer" | "different"): string;
  versionInstalled: string;
  /** A terminal or Codex that finds another fastctx first on its PATH. */
  pathLookup: Record<"windows" | "bash" | "unix" | "codex", string>;
  pathDifferent(where: string, version: string): string;
  pathUnknown(where: string): string;
  pathLegacy: string;
  pathTitle: string;
  pathOk: string;
  versionOld(node: string, nodeProtocol: number, client: string, clientProtocol: number): string;
  versionNone: string;
  renew: string;
  renewPhase: Record<import("../world/renewal").RenewalPhase, string>;
  renewFailed(why: string): string;
  guardian: string;
  guardianWhy: string;
  /** The app's own version, and a newer one from the project's releases. */
  app: string;
  appChecking: string;
  appLatest: string;
  appCheckAgain: string;
  appUnknown(why: string): string;
  appAvailable(version: string): string;
  appInstall: string;
  appDownloading(percent: number | null): string;
  appInstalling: string;
  appFailed(why: string): string;
  /** A phone: what it lets the World do in the background. */
  phoneNotices: string;
  phoneNoticesOn: string;
  phoneNoticesOff: string;
  phoneAllow: string;
  phoneOpenNotices: string;
  phoneBattery: string;
  phoneBatteryOn: string;
  phoneBatteryOff: string;
  phoneOpenBattery: string;
  phonePowerSave: string;
  phoneMaker(maker: string): string;
  /** What to set by hand on a maker's phones, which no app can read. */
  phoneVendor: Record<"honor" | "xiaomi" | "other", string[]>;
  phoneOpenApp: string;
  serviceOn: string;
  serviceOff: string;
  keep: string;
  keepAsks: string;
  kept: Record<"already" | "installed" | "declined" | "unsupported" | "removed" | "not_kept", string>;
  command(command: string): string;
  legacy: string;
  legacyCleaned(tidied: number, archived: number, place: string, waiting: number): string;
  legacyUnreadable(why: string): string;
}

export const zhSettings: SettingsWords = {
  title: "设置",
  look: "外观",
  keys: "快捷键",
  keyList: [
    ["Ctrl+K", "命令面板"],
    ["Ctrl+N", "新对话"],
    ["Ctrl+B", "显示/隐藏侧栏"],
    ["Ctrl+F", "在对话中查找"],
    ["Enter", "发送（Shift+Enter 换行）"],
    ["/", "对话命令"],
    ["Esc", "关闭菜单与查找"],
  ],
  notify: "通知",
  notifyOn: "等待回复或回复完成时通知",
  notifyOff: "关闭",
  theme: "主题",
  system: "跟随系统",
  themes: { "warm-black": "暖黑", "ink-black": "墨黑", "warm-white": "暖白" },
  language: "语言",
  textSize: "字号",
  sizeDefault: "默认",
  sizes: { "14": "小", "15": "标准", "16": "大", "17": "特大" },
  device: "本机",
  machine: "机器名",
  alone: "未加入世界",
  link: "服务器",
  hub: (name) => `服务器 ${name}`,
  as: (accounts) => (accounts ? `已登录 ${accounts}` : ""),
  version: "版本",
  online: (hub, since) => `已连接 ${hub} · ${since} 起`,
  connecting: () => "连接中",
  stopped: () => "未连接",
  notInShell: "浏览器预览，未连接世界",
  service: "开机自启",
  worldVersion: "本机服务",
  versionHere: (version) => version,
  versionDifferent: (relation) => (relation === "newer" ? "本机服务比应用新" : relation === "older" ? "本机服务较旧" : "本机服务与应用不一致"),
  versionInstalled: "已安装的程序与应用不一致",
  pathLookup: { windows: "命令提示符", bash: "Git Bash", unix: "终端", codex: "Codex" },
  pathDifferent: (where, version) => `${where} 使用的是另一份 fastctx（${version}）`,
  pathUnknown: (where) => `无法确认 ${where} 使用的 fastctx`,
  pathLegacy: "请勿运行旧版的 fastctx unapply：它会删除整个 .fastctx，包括世界的数据",
  pathTitle: "终端中的 fastctx",
  pathOk: "知道了",
  versionOld: (node, _nodeProtocol, client) => `本机服务 ${node} 与界面 ${client} 不兼容`,
  versionNone: "本机服务未运行",
  renew: "更新",
  renewPhase: {
    staged: "新版本已就绪",
    preparing: "正在准备…",
    waiting_for_idle: "等待当前工作告一段落…",
    awaiting_administrator: "需要管理员确认",
    awaiting_legacy_exit: "等待旧版本退出，重启电脑后完成",
    updating_guardian: "正在更新开机服务…",
    returning_to_service: "正在交回开机服务…",
    complete: "已更新",
    failed: "更新失败",
  },
  renewFailed: failure(zhProblems, "更新失败"),
  guardian: "管理员确认",
  guardianWhy: "更新开机服务，当前工作不受影响",
  app: "应用",
  appChecking: "正在检查更新…",
  appLatest: "已是最新版本",
  appCheckAgain: "检查更新",
  appUnknown: () => "无法检查更新",
  appAvailable: (version) => `新版本 ${version}`,
  appInstall: "下载并安装",
  appDownloading: (percent) => (percent === null ? "下载中…" : `下载中 ${percent}%`),
  appInstalling: "正在安装，应用将重新启动…",
  appFailed: failure(zhProblems, "安装失败"),
  phoneNotices: "通知",
  phoneNoticesOn: "已允许",
  phoneNoticesOff: "未允许",
  phoneAllow: "允许通知",
  phoneOpenNotices: "通知设置",
  phoneBattery: "电池优化",
  phoneBatteryOn: "不受限制",
  phoneBatteryOff: "受限制，通知可能延迟",
  phoneOpenBattery: "电池设置",
  phonePowerSave: "省电模式已开启",
  phoneMaker: (maker) => `${maker} 后台设置`,
  phoneVendor: {
    honor: [
      "设置 › 应用启动管理 › World：关闭自动管理，允许自启动、关联启动、后台活动",
      "设置 › 电池优化 › 所有应用 › World：不允许",
      "电池 › 更多电池设置：休眠时始终保持网络连接",
      "最近任务中下拉 World 卡片锁定",
    ],
    xiaomi: ["应用详情 › 省电策略：无限制", "应用详情 › 自启动：开启", "最近任务中长按 World 卡片锁定"],
    other: [],
  },
  phoneOpenApp: "应用详情",
  serviceOn: "已开启，断网后自动重连",
  serviceOff: "未开启",
  keep: "开启",
  keepAsks: "需要管理员确认",
  kept: {
    already: "已开启",
    installed: "已开启",
    declined: "未获管理员确认",
    unsupported: "此系统不支持",
    removed: "已关闭",
    not_kept: "未开启",
  },
  command: (command) => `以 root 运行：${command}`,
  legacy: "旧版 fastctx",
  legacyCleaned: (tidied, archived, place, waiting) =>
    `已清理 ${tidied} 项，归档 ${archived} 项到 ${place}${waiting > 0 ? `，${waiting} 项使用中` : ""}`,
  legacyUnreadable: () => "无法读取清理记录",
};

export const enSettings: SettingsWords = {
  title: "Settings",
  look: "Appearance",
  keys: "Shortcuts",
  keyList: [
    ["Ctrl+K", "Command palette"],
    ["Ctrl+N", "New conversation"],
    ["Ctrl+B", "Show/hide sidebar"],
    ["Ctrl+F", "Find in conversation"],
    ["Enter", "Send (Shift+Enter for new line)"],
    ["/", "Conversation commands"],
    ["Esc", "Close menus and find"],
  ],
  notify: "Notifications",
  notifyOn: "Notify when waiting or replied",
  notifyOff: "Off",
  theme: "Theme",
  system: "System",
  themes: { "warm-black": "Warm black", "ink-black": "Ink black", "warm-white": "Warm white" },
  language: "Language",
  textSize: "Text size",
  sizeDefault: "Default",
  sizes: { "14": "Small", "15": "Standard", "16": "Large", "17": "Extra large" },
  device: "This device",
  machine: "Machine name",
  alone: "Not in a World",
  link: "Server",
  hub: (name) => `Server ${name}`,
  as: (accounts) => (accounts ? `signed in to ${accounts}` : ""),
  version: "Version",
  online: (hub, since) => `connected to ${hub} · since ${since}`,
  connecting: () => "connecting",
  stopped: () => "not connected",
  notInShell: "Browser preview, not connected",
  service: "Start at boot",
  worldVersion: "Local service",
  versionHere: (version) => version,
  versionDifferent: (relation) => (relation === "newer" ? "Local service is newer than the app" : relation === "older" ? "Local service is outdated" : "Local service doesn't match the app"),
  versionInstalled: "The installed program doesn't match the app",
  pathLookup: { windows: "Command Prompt", bash: "Git Bash", unix: "Terminal", codex: "Codex" },
  pathDifferent: (where, version) => `${where} uses another fastctx (${version})`,
  pathUnknown: (where) => `Can't tell which fastctx ${where} uses`,
  pathLegacy: "Don't run the old fastctx unapply: it deletes the whole .fastctx, World data included",
  pathTitle: "fastctx in your terminal",
  pathOk: "Got it",
  versionOld: (node, _nodeProtocol, client) => `Local service ${node} isn't compatible with this app (${client})`,
  versionNone: "Local service not running",
  renew: "Update",
  renewPhase: {
    staged: "Update ready",
    preparing: "Preparing…",
    waiting_for_idle: "Waiting for current work to pause…",
    awaiting_administrator: "Needs administrator approval",
    awaiting_legacy_exit: "Waiting for the old version to exit; finishes after a restart",
    updating_guardian: "Updating startup service…",
    returning_to_service: "Handing back to the startup service…",
    complete: "Updated",
    failed: "Update failed",
  },
  renewFailed: failure(enProblems, "Update failed"),
  guardian: "Approve as administrator",
  guardianWhy: "Updates the startup service; current work continues",
  app: "App",
  appChecking: "Checking for updates…",
  appLatest: "Up to date",
  appCheckAgain: "Check for updates",
  appUnknown: () => "Couldn't check for updates",
  appAvailable: (version) => `Version ${version} available`,
  appInstall: "Download and install",
  appDownloading: (percent) => (percent === null ? "Downloading…" : `Downloading ${percent}%`),
  appInstalling: "Installing; the app will restart…",
  appFailed: failure(enProblems, "Install failed"),
  phoneNotices: "Notifications",
  phoneNoticesOn: "Allowed",
  phoneNoticesOff: "Not allowed",
  phoneAllow: "Allow notifications",
  phoneOpenNotices: "Notification settings",
  phoneBattery: "Battery optimization",
  phoneBatteryOn: "Unrestricted",
  phoneBatteryOff: "Restricted; notifications may be delayed",
  phoneOpenBattery: "Battery settings",
  phonePowerSave: "Power saving is on",
  phoneMaker: (maker) => `${maker} background settings`,
  phoneVendor: {
    honor: [
      "Settings › App launch › World: turn off auto-manage; allow auto-launch, secondary launch and background activity",
      "Settings › Battery optimization › All apps › World: Don't allow",
      "Battery › More battery settings: stay connected while asleep",
      "Pull World's card down in Recents to lock it",
    ],
    xiaomi: ["App details › Battery saver: No restrictions", "App details › Autostart: on", "Long-press World's card in Recents to lock it"],
    other: [],
  },
  phoneOpenApp: "App details",
  serviceOn: "On; reconnects automatically",
  serviceOff: "Off",
  keep: "Turn on",
  keepAsks: "Needs administrator approval",
  kept: {
    already: "On",
    installed: "On",
    declined: "Administrator approval declined",
    unsupported: "Not supported on this system",
    removed: "Turned off",
    not_kept: "Off",
  },
  command: (command) => `Run as root: ${command}`,
  legacy: "Old fastctx",
  legacyCleaned: (tidied, archived, place, waiting) =>
    `Cleaned ${tidied}, archived ${archived} to ${place}${waiting > 0 ? `, ${waiting} in use` : ""}`,
  legacyUnreadable: () => "Couldn't read the cleanup record",
};

export interface ReaderWordsSet {
  zh: ReaderWords;
  en: ReaderWords;
}

const units: Record<"zh" | "en", [string, string, string, string]> = { zh: ["字节", "KB", "MB", "GB"], en: ["bytes", "KB", "MB", "GB"] };

export const readerWords: ReaderWordsSet = {
  zh: {
    reread: "刷新",
    reading: "读取中…",
    tail: (shown, size) => `显示最后 ${shown}，共 ${size}`,
    notText: (size) => `非 UTF-8 文本（${size}）`,
    page: (page, count) => `${page} / ${count}`,
    fitWidth: "适应宽度",
    zoomIn: "放大",
    zoomOut: "缩小",
    pdfLocked: "PDF 已加密，无法打开",
    pdfFailed: failure(zhProblems, "无法显示 PDF"),
    failed: failure(zhProblems, "读取失败"),
    bytes: (count) => sizeOf(count, units.zh),
    modified: (ago) => `${ago}修改`,
  },
  en: {
    reread: "Refresh",
    reading: "Reading…",
    tail: (shown, size) => `Last ${shown} of ${size}`,
    notText: (size) => `Not UTF-8 text (${size})`,
    page: (page, count) => `${page} / ${count}`,
    fitWidth: "Fit width",
    zoomIn: "Zoom in",
    zoomOut: "Zoom out",
    pdfLocked: "This PDF is password-protected",
    pdfFailed: failure(enProblems, "Couldn't display PDF"),
    failed: failure(enProblems, "Couldn't read file"),
    bytes: (count) => sizeOf(count, units.en),
    modified: (ago) => `modified ${ago}`,
  },
};

export interface DetailsWords {
  open: string;
  close: string;
  model: string;
  modelSelf: string;
  machine: string;
  host: string;
  startedByWorld: string;
  origin: string;
  openedByYou: string;
  askedBy(asker: string, label: string): string;
  here: string;
  since(ago: string): string;
  state: string;
  busy(lasted: string): string;
  idle(ago: string): string;
  removed(ago: string): string;
  waiting: string;
  trouble: string;
  task: string;
  colleagues: string;
  worksOn: string;
  /** Where its commands start: the machine it lives on, or another. */
  worksOnLives: string;
  worksOnThere(machine: string): string;
  changeWorksOn: string;
  /** The choice of the machine it lives on. */
  livesHere(machine: string): string;
  worksOnSet(machine: string, lives: boolean): string;
  /** An agent lent to the user: who lent it, and that it works on no
   *  machine by default. */
  lentBy: string;
  worksLent: string;
  actingFor: string;
  separator: string;
  /** Which login of its agent it uses, where not the default one. */
  login: string;
  running: string;
  actions(shown: number, total: number): string;
  noActions: string;
  answers: string;
  records: string;
  stop: string;
  stopped: string;
  notRunning: string;
  exportMarkdown: string;
  exported: string;
  openOwn(agent: string): string;
  openedOwn: string;
  cannotLook(why: string): string;
  notFound: string;
  manage: string;
  move: string;
  moveTo: string;
  /** Beside the project it is in now. */
  inHere: string;
  leaveProject: string;
  noOtherProject: string;
  moved(project: string): string;
  left: string;
  remove: string;
  removeTitle(who: string): string;
  removeWhat: string;
  removeGo: string;
  /** Before what of a removal was not done. */
  removedButNot: string;
  /** A stage of an act that was not done: where, how, and the World's
   *  own account of it. */
  unfinished(stage: string, outcome: "pending" | "unsupported" | "failed", detail: string): string;
  cancel: string;
  working: string;
  failed(why: string): string;
}

export const zhDetails: DetailsWords = {
  open: "详情",
  close: "收起",
  model: "模型",
  modelSelf: "（自报）",
  machine: "所在机器",
  host: "程序",
  startedByWorld: "后台会话",
  origin: "来源",
  openedByYou: "由你开启",
  askedBy: (asker, label) => `${asker} 的同事 · ${label}`,
  here: "加入",
  since: (ago) => `${ago}前`,
  state: "状态",
  busy: (lasted) => `工作中 · ${lasted}`,
  idle: (ago) => `空闲 · ${ago}前`,
  removed: (ago) => `${ago}前已移除`,
  waiting: "等待回复",
  trouble: "异常",
  task: "任务",
  colleagues: "调用的模型",
  worksOn: "工作机器",
  worksOnLives: "所在机器",
  lentBy: "借自",
  worksLent: "每条命令指定",
  worksOnThere: (machine) => machine,
  changeWorksOn: "更改",
  livesHere: (machine) => `所在机器（${machine}）`,
  worksOnSet: (machine) => `下一条命令起在 ${machine} 上运行`,
  actingFor: "所属账号",
  separator: "、",
  login: "登录",
  running: "运行中的作业",
  actions: (shown, total) => (shown < total ? `最近动作 ${shown}/${total}` : "动作"),
  noActions: "暂无动作",
  answers: "已交回",
  records: "记录",
  stop: "停止",
  stopped: "已请求停止",
  notRunning: "当前没有运行",
  exportMarkdown: "复制为 Markdown",
  exported: "已复制",
  openOwn: (agent) => `在 ${agent} 中打开`,
  openedOwn: "已在新终端窗口中打开",
  cannotLook: () => "无法获取详情",
  notFound: "未找到该对话",
  manage: "管理",
  move: "移动到项目",
  moveTo: "目标项目",
  inHere: "当前",
  leaveProject: "移出项目",
  noOtherProject: "这台机器上没有其他项目",
  moved: (project) => `已移到「${project}」`,
  left: "已移出项目",
  remove: "移除",
  removeTitle: (who) => `移除 ${who}？`,
  removeWhat: "移除后不再显示，记录归档保留，运行中的作业不受影响。",
  removeGo: "移除",
  removedButNot: "已移除，未完成：",
  unfinished: (stage, outcome) =>
    `${{ host: "宿主", records: "记录", tools: "工具", notification: "通知" }[stage] ?? stage}${{ pending: "等待中", unsupported: "不支持", failed: "失败" }[outcome]}`,
  cancel: "关闭",
  working: "处理中…",
  failed: failure(zhProblems, "操作失败"),
};

export const enDetails: DetailsWords = {
  open: "Details",
  close: "Close",
  model: "Model",
  modelSelf: " (self-reported)",
  machine: "Machine",
  host: "Program",
  startedByWorld: "background session",
  origin: "Origin",
  openedByYou: "opened by you",
  askedBy: (asker, label) => `colleague of ${asker} · ${label}`,
  here: "Joined",
  since: (ago) => `${ago} ago`,
  state: "Status",
  busy: (lasted) => `working · ${lasted}`,
  idle: (ago) => `idle · ${ago} ago`,
  removed: (ago) => `removed ${ago} ago`,
  waiting: "waiting for your reply",
  trouble: "Issues",
  task: "Task",
  colleagues: "Models it called",
  worksOn: "Works on",
  worksOnLives: "its own machine",
  lentBy: "Lent by",
  worksLent: "chosen per command",
  worksOnThere: (machine) => machine,
  changeWorksOn: "Change",
  livesHere: (machine) => `its own machine (${machine})`,
  worksOnSet: (machine) => `Runs on ${machine} from the next command`,
  actingFor: "Account",
  separator: ", ",
  login: "Login",
  running: "Running jobs",
  actions: (shown, total) => (shown < total ? `Recent actions ${shown}/${total}` : "Actions"),
  noActions: "No actions yet",
  answers: "Handed back",
  records: "Records",
  stop: "Stop",
  stopped: "Stop requested",
  notRunning: "Not running",
  exportMarkdown: "Copy as Markdown",
  exported: "Copied",
  openOwn: (agent) => `Open in ${agent}`,
  openedOwn: "Opened in a new terminal window",
  cannotLook: () => "Couldn't load details",
  notFound: "Conversation not found",
  manage: "Manage",
  move: "Move to project",
  moveTo: "Project",
  inHere: "current",
  leaveProject: "Remove from project",
  noOtherProject: "No other projects on this machine",
  moved: (project) => `Moved to “${project}”`,
  left: "Removed from project",
  remove: "Remove",
  removeTitle: (who) => `Remove ${who}?`,
  removeWhat: "It will no longer be listed. Its records are archived; running jobs continue.",
  removeGo: "Remove",
  removedButNot: "Removed. Not finished: ",
  unfinished: (stage, outcome) =>
    `${{ host: "host", records: "records", tools: "tools", notification: "notification" }[stage] ?? stage} ${{ pending: "pending", unsupported: "unsupported", failed: "failed" }[outcome]}`,
  cancel: "Close",
  working: "Working…",
  failed: failure(enProblems, "Couldn't complete"),
};

export interface ProjectFormWords {
  newTitle: string;
  /** Which account a new project opens in. */
  account: string;
  editTitle(name: string): string;
  name: string;
  namePlaceholder: string;
  slug: string;
  slugHint: string;
  what: string;
  whatPlaceholder: string;
  machines: string;
  places: string;
  placesHint: string;
  placeWhat: string;
  placePath: string;
  /** The machine a place is on, named for a screen reader. */
  placeMachine: string;
  addPlace: string;
  removePlace: string;
  canon: string;
  canonHint: string;
  canonMachine: string;
  noCanon: string;
  open: string;
  save: string;
  saving: string;
  cancel: string;
  failed(why: string): string;
}

export const zhProjectForm: ProjectFormWords = {
  newTitle: "新建项目",
  account: "账号",
  editTitle: (name) => `编辑「${name}」`,
  name: "名称",
  namePlaceholder: "例如 mark50 研发",
  slug: "短名",
  slugHint: "小写字母开头，可含数字和连字符；创建后不可修改",
  what: "简介",
  whatPlaceholder: "这个项目做什么",
  machines: "机器",
  places: "位置",
  placesHint: "每行一项：名称、机器、绝对路径",
  placeWhat: "名称",
  placePath: "路径",
  placeMachine: "机器",
  addPlace: "添加",
  removePlace: "删除",
  canon: "定论",
  canonHint: "定论所在文件夹",
  canonMachine: "定论所在机器",
  noCanon: "无",
  open: "创建",
  save: "保存",
  saving: "保存中…",
  cancel: "关闭",
  failed: failure(zhProblems, "保存失败"),
};

export const enProjectForm: ProjectFormWords = {
  newTitle: "New project",
  account: "Account",
  editTitle: (name) => `Edit “${name}”`,
  name: "Name",
  namePlaceholder: "e.g. Mark 50 research",
  slug: "Short name",
  slugHint: "Starts with a lowercase letter; digits and hyphens allowed; can't be changed later",
  what: "Description",
  whatPlaceholder: "What this project is for",
  machines: "Machines",
  places: "Locations",
  placesHint: "One per line: name, machine, absolute path",
  placeWhat: "Name",
  placePath: "Path",
  placeMachine: "Machine",
  addPlace: "Add",
  removePlace: "Delete",
  canon: "Decisions",
  canonHint: "Folder with the decisions",
  canonMachine: "Machine with the decisions",
  noCanon: "None",
  open: "Create",
  save: "Save",
  saving: "Saving…",
  cancel: "Close",
  failed: failure(enProblems, "Couldn't save"),
};

/** The user's consent to Claude working without asking, on one machine,
 *  in one of its logins there: `account` names it where there are several. */
export interface ConsentWords {
  title(machine: string, account: string | null): string;
  what(machine: string, account: string | null): string;
  allow: string;
  withdrawTitle(machine: string, account: string | null): string;
  withdrawWhat: string;
  withdrawGo: string;
  working: string;
  failed(why: string): string;
  close: string;
}

export const zhConsent: ConsentWords = {
  title: (machine, account) => `允许 Claude${account ? `（${account}）` : ""}在 ${machine} 上自动运行？`,
  what: () =>
    "Claude 将直接运行命令、修改文件、访问网络，不再逐项确认，等同于 Claude Code 的「Bypass permissions」。可随时在机器页撤销。",
  allow: "允许",
  withdrawTitle: (machine, account) => `撤销 Claude${account ? `（${account}）` : ""}在 ${machine} 上的授权？`,
  withdrawWhat: "撤销后，世界将无法在这台机器上启动 Claude，直到再次授权。",
  withdrawGo: "撤销",
  working: "处理中…",
  failed: failure(zhProblems, "操作失败"),
  close: "关闭",
};

export const enConsent: ConsentWords = {
  title: (machine, account) => `Allow Claude${account ? ` (${account})` : ""} to run unattended on ${machine}?`,
  what: () =>
    "Claude will run commands, edit files and use the network without asking each time, the same as Claude Code's “Bypass permissions”. Revoke anytime on the Machines page.",
  allow: "Allow",
  withdrawTitle: (machine, account) => `Revoke Claude's permission${account ? ` for ${account}` : ""} on ${machine}?`,
  withdrawWhat: "The World won't be able to start Claude on this machine until you allow it again.",
  withdrawGo: "Revoke",
  working: "Working…",
  failed: failure(enProblems, "Couldn't complete"),
  close: "Close",
};

export interface PaletteWords {
  placeholder: string;
  nothing: string;
  actions: string;
  places: string;
  agents: string;
  machines: string;
  theme(name: string): string;
  /** Letting another device into an account. */
  invite(login: string): string;
  fold: string;
  hint: string;
}

export const zhPalette: PaletteWords = {
  placeholder: "搜索页面、对话、命令…",
  nothing: "无匹配结果",
  actions: "操作",
  places: "页面",
  agents: "对话",
  machines: "机器",
  theme: (name) => `主题：${name}`,
  invite: (login) => `添加设备到 ${login}`,
  fold: "显示/隐藏侧栏",
  hint: "↑↓ 选择 · Enter 打开 · Esc 关闭",
};

export const enPalette: PaletteWords = {
  placeholder: "Search pages, conversations, commands…",
  nothing: "No results",
  actions: "Actions",
  places: "Pages",
  agents: "Conversations",
  machines: "Machines",
  theme: (name) => `Theme: ${name}`,
  invite: (login) => `Add device to ${login}`,
  fold: "Show/hide sidebar",
  hint: "↑↓ to select · Enter to open · Esc to close",
};

export interface ComposerWords {
  asks: AskWords;
  attach: AttachWords;
  files: FileWords;
  commands: CommandWords;
  said: SaidWords;
  modelChip: string;
  noModels: string;
  /** Rewinding to the user's words: what it drops and what it does not. */
  rewindAsk: string;
  rewindGo: string;
  regenerateAsk: string;
  regenerateGo: string;
  forkAsk: string;
  forkGo: string;
  never: string;
  unknown(name: string): string;
  running(name: string): string;
  done(name: string): string;
  /** Carried out, with no word from the host on whether it took. */
  reported(name: string): string;
  /** The host refused, in its words; it is left on `model`. */
  refusedOn(said: string, model: string): string;
  failed(why: string): string;
  context(percent: number, used: number, window: number): string;
  usage: {
    title: string;
    context: string;
    contextUsed(used: number, window: number | null): string;
    limits: string;
    window(name: string): string;
    used(percent: string): string;
    resets(at: Date): string;
    cost(usd: string): string;
    none: string;
  };
}

export const zhComposer: ComposerWords = {
  files: {
    hint: "输入文件名搜索",
    searching: "搜索中…",
    none: "未找到匹配的文件",
    outside: "加入项目后可用 @ 引用文件",
    nowhere: "项目未设置这台机器上的文件位置",
  },
  attach: {
    attach: "添加文件",
    photo: "拍照",
    drop: () => "拖放文件到这里",
    remove: "移除",
    sending: "上传中…",
    failed: failure(zhProblems, "上传失败"),
    retry: "重试",
    size: (bytes) => sizeOf(bytes, ["B", "KB", "MB", "GB"]),
    wait: "文件上传完成后再发送",
  },
  asks: {
    waiting: "等待确认",
    command: (machine) => `请求在 ${machine} 上运行命令`,
    stdin: (machine) => `请求向 ${machine} 上运行中的程序发送输入`,
    network: (host, protocol) => `请求连接 ${host}${protocol ? `（${protocol}）` : ""}`,
    files: "请求修改文件",
    filesNamed: (count) => `请求修改 ${count} 个文件`,
    grantRoot: (root) => `并请求在本次对话中写入 ${root}`,
    reason: (why) => why,
    inDir: (cwd) => cwd,
    accept: "允许",
    acceptForSession: "本次对话都允许",
    acceptPrefix: (prefix) => `始终允许 ${prefix}`,
    networkRule: (allow, host) => (allow ? `始终允许连接 ${host}` : `始终禁止连接 ${host}`),
    decline: "拒绝，继续",
    cancel: "拒绝并停止",
    question: "提问",
    questions: (count) => `${count} 个问题`,
    several: "可多选",
    otherAnswer: "其他回答",
    secret: "输入内容不会显示",
    submit: "回答",
    fill: "提交",
    skip: "跳过",
    form: (server) => `${server} 请求填写表单`,
    link: (server) => `${server} 请求打开链接`,
    openLink: "打开链接",
    refuse: "拒绝",
    dismiss: "忽略",
    required: "必填",
    requiredOf: (name) => `必填：${name}`,
    formUnreadable: "无法显示此表单",
    sending: "提交中…",
    failed: failure(zhProblems, "提交失败"),
    cannot: (request) => `暂不支持的请求：${request}`,
    permissions: "请求更多权限（暂不支持）",
    verification: "请求身份验证（暂不支持）",
    tool: (name) => `请求使用 ${name}`,
    plan: "计划待批准",
    planAt: (path) => path,
    planGo: "批准并执行",
    planGoWith: (kept) => `批准 · ${kept}`,
    planKeep: "继续规划",
    planNote: "修改意见",
    rule: (allow, rules, where) =>
      `${allow === "allow" ? "始终允许" : allow === "deny" ? "始终禁止" : "始终询问"} ${rules}（${where}）`,
    unrule: (rules, where) => `移除规则 ${rules}（${where}）`,
    mode: (mode, where) => `切换到 ${mode} 模式（${where}）`,
    directories: (add, directories, where) => `${add ? "允许访问" : "取消访问"} ${directories}（${where}）`,
    kept: {
      session: "本次对话",
      localSettings: "项目本地设置",
      projectSettings: "项目设置",
      userSettings: "用户设置",
      cliArg: "本次启动",
    },
    answered: (said) => said,
    gone: "已在别处处理",
    settledBy: {
      accept: "已允许",
      session: "本次对话已允许",
      rule: "已添加规则",
      decline: "已拒绝",
      cancel: "已拒绝并停止",
      answered: "已回答",
      skipped: "已跳过",
      submitted: "已提交",
      refused: "已拒绝",
      dismissed: "已忽略",
      opened: "已打开链接",
      planGo: "已批准",
      planKeep: "继续规划",
    },
  },
  commands: {
    world: "世界",
    compact: "压缩对话",
    model: "切换模型",
    rename: "重命名",
    fork: "分叉对话",
    stop: "停止",
    noCommands: "无匹配命令",
    effort: "推理强度",
    current: "当前",
    renameArgs: "<名称>",
    needsArgs: (usage) => `需要参数：${usage}`,
    host: {
      compact: "压缩对话",
      model: "切换模型与推理强度",
      rename: "重命名对话",
      fork: "分叉为新对话",
      review: "审查未提交的改动",
      mcp: "MCP 服务",
      skills: "技能",
      status: "状态",
      usage: "用量",
    },
  },
  said: {
    model: (model, effort) => `下一轮起使用 ${model}${effort ? ` · ${effort}` : ""}`,
    compact: "正在压缩…",
    rewind: "已回退，文件改动未撤销",
    review: "正在审查…",
    skill: (name) => `已发送 /${name}`,
    close: "关闭",
    skills: "技能",
    noSkills: "没有可用的技能",
    mcp: "MCP 服务",
    noServers: "没有已连接的 MCP 服务",
    server: (state) => ({ running: "运行中", ready: "就绪", enabled: "已启用", connected: "已连接", connecting: "连接中", starting: "启动中", disabled: "已停用", failed: "启动失败" })[state] ?? state,
    tools: (count) => `${count} 个工具`,
    toolsFailed: () => "获取工具失败",
  },
  modelChip: "模型",
  noModels: "没有可选的模型",
  rewindAsk: "从这条消息重新开始？之后的对话将被移除，文件改动不会撤销。",
  rewindGo: "重新开始",
  regenerateAsk: "重新回答这一轮？这一轮及之后的对话将被移除，文件改动不会撤销。",
  regenerateGo: "重新回答",
  forkAsk: "从这里分叉出新对话？新对话保留到此为止的内容。",
  forkGo: "分叉",
  never: "取消",
  unknown: (name) => `未知命令 /${name}`,
  running: (name) => `正在执行 /${name}…`,
  done: (name) => `/${name} 已完成`,
  reported: (name) => `/${name} 已执行`,
  refusedOn: (said, model) => `${said}（当前模型 ${model}）`,
  failed: failure(zhProblems, "执行失败"),
  context: (percent, used, window) => `上下文 ${percent}%（${used.toLocaleString()} / ${window.toLocaleString()}）`,
  usage: {
    title: "用量",
    context: "上下文",
    contextUsed: (used, window) => (window ? `${used.toLocaleString()} / ${window.toLocaleString()}` : used.toLocaleString()),
    limits: "额度",
    window: (name) => {
      const minutes = /(\d+)m$/.exec(name);
      if (minutes) {
        const m = Number(minutes[1]);
        return m % 1440 === 0 ? `${m / 1440} 天` : m % 60 === 0 ? `${m / 60} 小时` : `${m} 分钟`;
      }
      return ({ five_hour: "5 小时", seven_day: "7 天" } as Record<string, string>)[name] ?? name;
    },
    used: (percent) => `已用 ${percent}%`,
    resets: (at) => `${at.getMonth() + 1}月${at.getDate()}日 ${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")} 重置`,
    cost: (usd) => `本次对话 $${usd}`,
    none: "暂无用量数据",
  },
};

export const enComposer: ComposerWords = {
  files: {
    hint: "Search by file name",
    searching: "Searching…",
    none: "No matching files",
    outside: "Join a project to mention files with @",
    nowhere: "This project has no file locations on that machine",
  },
  attach: {
    attach: "Add files",
    photo: "Take a photo",
    drop: () => "Drop files here",
    remove: "Remove",
    sending: "Uploading…",
    failed: failure(enProblems, "Upload failed"),
    retry: "Retry",
    size: (bytes) => sizeOf(bytes, ["B", "KB", "MB", "GB"]),
    wait: "Wait for uploads to finish",
  },
  asks: {
    waiting: "Needs your approval",
    command: (machine) => `Run a command on ${machine}`,
    stdin: (machine) => `Send input to a program on ${machine}`,
    network: (host, protocol) => `Connect to ${host}${protocol ? ` (${protocol})` : ""}`,
    files: "Edit files",
    filesNamed: (count) => `Edit ${count} ${count === 1 ? "file" : "files"}`,
    grantRoot: (root) => `and write to ${root} for this conversation`,
    reason: (why) => why,
    inDir: (cwd) => cwd,
    accept: "Allow",
    acceptForSession: "Allow for this conversation",
    acceptPrefix: (prefix) => `Always allow ${prefix}`,
    networkRule: (allow, host) => (allow ? `Always allow ${host}` : `Always block ${host}`),
    decline: "Deny and continue",
    cancel: "Deny and stop",
    question: "Question",
    questions: (count) => `${count} questions`,
    several: "Select all that apply",
    otherAnswer: "Other answer",
    secret: "Input is hidden",
    submit: "Answer",
    fill: "Submit",
    skip: "Skip",
    form: (server) => `${server} requests a form`,
    link: (server) => `${server} requests to open a link`,
    openLink: "Open link",
    refuse: "Decline",
    dismiss: "Ignore",
    required: "Required",
    requiredOf: (name) => `Required: ${name}`,
    formUnreadable: "This form can't be displayed",
    sending: "Submitting…",
    failed: failure(enProblems, "Couldn't submit"),
    cannot: (request) => `Unsupported request: ${request}`,
    permissions: "Requests more permissions (not supported)",
    verification: "Requests verification (not supported)",
    tool: (name) => `Use ${name}`,
    plan: "Plan ready for approval",
    planAt: (path) => path,
    planGo: "Approve and run",
    planGoWith: (kept) => `Approve · ${kept}`,
    planKeep: "Keep planning",
    planNote: "Changes to the plan",
    rule: (allow, rules, where) =>
      `${allow === "allow" ? "Always allow" : allow === "deny" ? "Always block" : "Always ask for"} ${rules} (${where})`,
    unrule: (rules, where) => `Remove rule ${rules} (${where})`,
    mode: (mode, where) => `Switch to ${mode} mode (${where})`,
    directories: (add, directories, where) => `${add ? "Allow access to" : "Remove access to"} ${directories} (${where})`,
    kept: {
      session: "this conversation",
      localSettings: "local project settings",
      projectSettings: "project settings",
      userSettings: "user settings",
      cliArg: "this launch",
    },
    answered: (said) => said,
    gone: "Handled elsewhere",
    settledBy: {
      accept: "Allowed",
      session: "Allowed for this conversation",
      rule: "Rule added",
      decline: "Denied",
      cancel: "Denied and stopped",
      answered: "Answered",
      skipped: "Skipped",
      submitted: "Submitted",
      refused: "Declined",
      dismissed: "Ignored",
      opened: "Link opened",
      planGo: "Approved",
      planKeep: "Kept planning",
    },
  },
  commands: {
    world: "World",
    compact: "Compact conversation",
    model: "Switch model",
    rename: "Rename",
    fork: "Fork conversation",
    stop: "Stop",
    noCommands: "No matching commands",
    effort: "Reasoning",
    current: "current",
    renameArgs: "<name>",
    needsArgs: (usage) => `Needs arguments: ${usage}`,
    host: {
      compact: "Compact conversation",
      model: "Switch model and reasoning",
      rename: "Rename conversation",
      fork: "Fork into a new conversation",
      review: "Review uncommitted changes",
      mcp: "MCP servers",
      skills: "Skills",
      status: "Status",
      usage: "Usage",
    },
  },
  said: {
    model: (model, effort) => `Using ${model}${effort ? ` · ${effort}` : ""} from the next turn`,
    compact: "Compacting…",
    rewind: "Rewound; file changes were kept",
    review: "Reviewing…",
    skill: (name) => `Sent /${name}`,
    close: "Close",
    skills: "Skills",
    noSkills: "No skills available",
    mcp: "MCP servers",
    noServers: "No MCP servers connected",
    server: (state) => ({ running: "running", ready: "ready", enabled: "enabled", connected: "connected", connecting: "connecting", starting: "starting", disabled: "disabled", failed: "failed" })[state] ?? state,
    tools: (count) => `${count} ${count === 1 ? "tool" : "tools"}`,
    toolsFailed: () => "Couldn't load tools",
  },
  modelChip: "Model",
  noModels: "No models available",
  rewindAsk: "Restart from this message? Later messages will be removed; file changes stay.",
  rewindGo: "Restart",
  regenerateAsk: "Answer this turn again? It and later messages will be removed; file changes stay.",
  regenerateGo: "Answer again",
  forkAsk: "Fork a new conversation from here? It keeps everything up to this point.",
  forkGo: "Fork",
  never: "Cancel",
  unknown: (name) => `Unknown command /${name}`,
  running: (name) => `Running /${name}…`,
  done: (name) => `/${name} done`,
  reported: (name) => `/${name} ran`,
  refusedOn: (said, model) => `${said} (now on ${model})`,
  failed: failure(enProblems, "Command failed"),
  context: (percent, used, window) => `Context ${percent}% (${used.toLocaleString()} / ${window.toLocaleString()})`,
  usage: {
    title: "Usage",
    context: "Context",
    contextUsed: (used, window) => (window ? `${used.toLocaleString()} / ${window.toLocaleString()}` : used.toLocaleString()),
    limits: "Limits",
    window: (name) => {
      const minutes = /(\d+)m$/.exec(name);
      if (minutes) {
        const m = Number(minutes[1]);
        return m % 1440 === 0 ? `${m / 1440} d` : m % 60 === 0 ? `${m / 60} h` : `${m} min`;
      }
      return ({ five_hour: "5 h", seven_day: "7 d" } as Record<string, string>)[name] ?? name;
    },
    used: (percent) => `${percent}% used`,
    resets: (at) => `Resets ${at.toLocaleString("en", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false })}`,
    cost: (usd) => `This conversation $${usd}`,
    none: "No usage data",
  },
};

function zhSpan(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "不到一分钟";
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时`;
  return `${Math.floor(hours / 24)} 天`;
}

function enSpan(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h`;
  const days = Math.floor(hours / 24);
  return `${days} day${days === 1 ? "" : "s"}`;
}

export const zhShell: ShellWords = {
  app: "World",
  world: "世界",
  projects: "项目",
  outside: "对话",
  machines: "机器",
  agents: "智能体",
  features: "功能",
  all: "全部",
  device: (form) => ({ desktop: "台式机", laptop: "笔记本", server: "服务器" })[form],
  settings: "设置",
  newTalk: "新对话",
  newProject: "新建项目",
  edit: "编辑",
  openSidebar: "显示侧栏",
  foldSidebar: "隐藏侧栏",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueFor: (asker) => `${asker} 的同事`,
  busy: (lasted) => `工作中 · ${lasted}`,
  idle: "空闲",
  waiting: "等待回复",
  offline: "离线",
  noProjects: "暂无项目",
  nobodyHere: "暂无对话",
  unreachable: "本机服务未连接",
  unanswered: (seconds) => `本机服务 ${seconds} 秒未响应`,
  thisOne: "本机",
  phone: "手机",
  logIn: "加入世界",
  account: "账号",
  arrivals: (n) => `${n} 台新设备`,
  groupOf: (login) => login,
  more: (n) => `另外 ${n} 个`,
  span: zhSpan,
  agentsIn: (project) => `${project} 中的对话`,
  frame: { minimize: "最小化", maximize: "最大化", restore: "还原", close: "关闭" },
};

export const enShell: ShellWords = {
  app: "World",
  world: "World",
  projects: "Projects",
  outside: "Conversations",
  machines: "Machines",
  agents: "Agents",
  features: "Features",
  all: "All",
  device: (form) => form,
  settings: "Settings",
  newTalk: "New conversation",
  newProject: "New project",
  edit: "Edit",
  openSidebar: "Show sidebar",
  foldSidebar: "Hide sidebar",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueFor: (asker) => `colleague of ${asker}`,
  busy: (lasted) => `working · ${lasted}`,
  idle: "idle",
  waiting: "waiting for your reply",
  offline: "offline",
  noProjects: "No projects yet",
  nobodyHere: "No conversations",
  unreachable: "Local service not connected",
  unanswered: (seconds) => `Local service not responding for ${seconds} s`,
  thisOne: "this computer",
  phone: "phone",
  logIn: "Join a World",
  account: "Account",
  arrivals: (n) => `${n} new device${n === 1 ? "" : "s"}`,
  groupOf: (login) => login,
  more: (n) => `${n} more`,
  span: enSpan,
  agentsIn: (project) => `Conversations in ${project}`,
  frame: { minimize: "Minimize", maximize: "Maximize", restore: "Restore", close: "Close" },
};
