// What the frame around every page says: the sidebar, the new conversation,
// the settings. In the user's words (`world-gui.md`, 用词).

import type { FolderPickerWords } from "./folder-picker";
import { dollars, underCent } from "../world/money";
import { agentName } from "../world/names";
import type { AskWords } from "../conversation/asks";
import type { SaidWords } from "../conversation/said";
import type { CommandWords } from "../conversation/commands";
import type { FileWords } from "../conversation/mention";
import type { PresenceWords } from "./presence";
import type { ReaderWords } from "./reader";
import { sizeOf, type AttachWords } from "../conversation/attach";
import { enProblems, failure, zhProblems } from "./problem";
import { enMoment, zhMoment } from "./when-words";
import { en as conversationEn, zh as conversationZh } from "../words";

export interface ShellWords {
  /** An agent assisting the user, as whose it is, never its machine. */
  someoneAssisting(agent: string, owner: string): string;
  /** A conversation forked from another. */
  forked: string;
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
  /** A page head's way back, for a screen reader: to the page named. */
  backTo(page: string): string;
  /** "laptop 上的 Claude". */
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
  /** The networks at the top of the sidebar. */
  networks: string;
  newNetwork: string;
  joinNetwork: string;
  /** Joining one more, once this device is in a network. */
  joinAnother: string;
  admin: string;
  member: string;
  /** How many machines a network has, as its row says it. */
  machinesCount(n: number): string;
  /** The user's own page, named in the sidebar by their name there. */
  account: string;
  /** Who came into the accounts lately, not yet shown, for a screen reader. */
  arrivals(n: number): string;
  /** A login's part of the sidebar, folded or not. */
  groupOf(login: string): string;
  more(n: number): string;
  span(ms: number): string;
  /** A project's fold button: the agents in it. */
  agentsIn(project: string): string;
  /** A project's "⋯" in the sidebar, and what is in it before its sharing. */
  projectMore: string;
  projectNewTalk: string;
  projectEdit: string;
  /** A project opened with no conversation in it yet. */
  noTalks: string;
  /** The window's caption buttons, where the shell draws them. */
  frame: { minimize: string; maximize: string; restore: string; close: string };
}

/** The machines and folders a conversation is pointed at
 *  (`focus-panel.ts`). */
export interface FocusWords {
  title: string;
  /** The chip, when none is chosen. */
  none: string;
  here: string;
  offline: string;
  addFolder: string;
  remove: string;
  noMachine: string;
  /** Over the machines chosen. */
  chosen: string;
  /** Over the user's own machines, another person's, and every other
   *  person's where each is a row of their own. */
  mine: string;
  theirs(owner: string): string;
  others: string;
  /** The field that narrows the machines, and when none is left. */
  search: string;
  noMatch: string;
  done: string;
  picker: FolderPickerWords;
  /** How many more are chosen than the chip names. */
  more(count: number): string;
  saving: string;
  failed(why: string): string;
  /** Every machine was taken off, and the way to put them back. */
  cleared: string;
  undo: string;
}

export const zhFolderPicker: FolderPickerWords = {
  title: (machine) => `选择 ${machine} 上的文件夹`,
  close: "关闭",
  home: "主目录",
  up: "上一级",
  reading: "读取中…",
  noFolders: "没有子文件夹",
  path: "路径",
  unreadable: "打不开这个文件夹",
  privacy: "macOS 没让 Fastctx 进这个文件夹：在「系统设置 › 隐私与安全性 › 完全磁盘访问权限」里打开 Fastctx，授权一次，换版也有效",
  privacyGo: "去授权",
  cancel: "取消",
  choose: "选择文件夹",
};

export const enFolderPicker: FolderPickerWords = {
  title: (machine) => `Choose a folder on ${machine}`,
  close: "Close",
  home: "Home",
  up: "Up",
  reading: "Reading…",
  noFolders: "No folders here",
  path: "Path",
  unreadable: "This folder could not be opened",
  privacy: "macOS keeps this folder from Fastctx: turn Fastctx on in System Settings › Privacy & Security › Full Disk Access; once is enough, across versions",
  privacyGo: "Allow",
  cancel: "Cancel",
  choose: "Choose folder",
};

export const zhFocus: FocusWords = {
  title: "机器与文件夹",
  none: "机器与文件夹",
  here: "本机",
  offline: "离线",
  addFolder: "文件夹",
  remove: "移除",
  noMachine: "没有可用的机器",
  chosen: "已选",
  mine: "我的机器",
  theirs: (owner) => `${owner} 的机器`,
  others: "其他人的机器",
  search: "搜索机器",
  noMatch: "没有对得上的机器",
  done: "完成",
  picker: zhFolderPicker,
  more: (count) => `+${count}`,
  saving: "保存中…",
  failed: (why) => `没改成：${why}`,
  cleared: "已清空",
  undo: "撤回",
};

export const enFocus: FocusWords = {
  title: "Machines and folders",
  none: "Machines and folders",
  here: "this computer",
  offline: "offline",
  addFolder: "Folder",
  remove: "Remove",
  noMachine: "No machine available",
  chosen: "Chosen",
  mine: "My machines",
  theirs: (owner) => `${owner}'s machines`,
  others: "Others' machines",
  search: "Search machines",
  noMatch: "No machine matches",
  done: "Done",
  picker: enFolderPicker,
  more: (count) => `+${count}`,
  saving: "Saving…",
  failed: (why) => `Not changed: ${why}`,
  cleared: "Cleared",
  undo: "Undo",
};

export interface NewTalkWords {
  english: boolean;
  title: string;
  /** Which agent, as one of its logins; and choosing another than the one
   *  chosen. */
  agent: string;
  /** The agents' groups: on the user's own machines, on machines of
   *  others shared with them or that an administrator may use, assisting
   *  them. */
  mine: string;
  others(shared: boolean): string;
  assisting: string;
  ownedBy(owner: string): string;
  /** Over the table: all agents, rather than one of them. */
  /** Another person's row in the table: how many machines, how many
   *  logins. */
  personSub(machines: number, logins: number): string;
  /** Where none of the machines has a login of this agent. */
  noneOf(agent: string): string;
  /** The field that narrows many agents to those typed for. */
  narrow: string;
  /** An assisting login, which its machine does not list. */
  assistantGone: string;
  /** A login whose usage window is spent goes on with credits. */
  onCredits: string;
  /** A usage window past the reset it was read before. */
  limitReset: string;
  /** A Claude login with a long-lived token, which names no plan. */
  token: string;
  /** Which account it works for. */
  account: string;
  /** What an assisting agent works on: the hover of its mark. */
  assistantWorks(owner: string): string;
  /** An agent assisting the account, whose it is. */
  assistFrom(owner: string): string;
  /** Where an assisting agent is listed: whose agents, never its machine. */
  assistAgents(owner: string): string;
  /** When an assistance ends. */
  assistUntil(at: Date, now: Date): string;
  /** An assisting agent was given files other than images, and no machine of
   *  the user's can take them. */
  noFilePlace: string;
  project: string;
  /** The project label when the conversation begins in none. */
  outsideProject: string;
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
  /** The input's placeholder, for the program chosen. */
  say(agent: string | null): string;
  start: string;
  starting: string;
  /** Said while what is attached goes to the machine, before the agent is
   *  asked in. */
  sendingFiles: string;
  /** Said over the sheet while files are dragged onto it. */
  dropHere: string;
  filesNotSent: string;
  /** The conversation did not begin; the kernel's reason is its hover. */
  notOpened(why: string): string;
}

export const zhNewTalk: NewTalkWords = {
  english: false,
  title: "新对话",
  agent: "智能体",
  mine: "我的机器",
  others: (shared) => (shared ? "共享给我的" : "其他成员的"),
  assisting: "协助我的",
  ownedBy: (owner) => `${owner} 的`,
  personSub: (machines, logins) => `${machines} 台　${logins} 个登录`,
  noneOf: (agent) => `没有可用的 ${agent}`,
  narrow: "搜索机器或账号",
  assistantGone: "该登录已不可用",
  onCredits: "使用 credits",
  limitReset: "已重置",
  token: "长期令牌",
  account: "账号",
  assistantWorks: (owner) => `来自 ${owner}，在你的机器上工作`,
  assistFrom: (owner) => `来自 ${owner}`,
  assistAgents: (owner) => `${owner} 的智能体`,
  assistUntil: (at, now) => `到 ${zhMoment.short(at, now)}`,
  noFilePlace: "没有能放文件的机器",
  project: "项目",
  outsideProject: "不在项目里",
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
  say: (agent) => (agent ? `发消息给 ${agent}…` : "写下第一句话…"),
  start: "开始",
  starting: "正在启动…",
  sendingFiles: "正在上传文件…",
  dropHere: "拖放到这里",
  filesNotSent: "部分文件上传失败",
  notOpened: () => "启动失败",
};

export const enNewTalk: NewTalkWords = {
  english: true,
  title: "New conversation",
  agent: "Agent",
  mine: "My machines",
  others: (shared) => (shared ? "Shared with me" : "Other members'"),
  assisting: "Assisting me",
  ownedBy: (owner) => `${owner}'s`,
  personSub: (machines, logins) => `${machines} machine${machines === 1 ? "" : "s"}, ${logins} login${logins === 1 ? "" : "s"}`,
  noneOf: (agent) => `No ${agent} logins available`,
  narrow: "Search machines or accounts",
  assistantGone: "Login unavailable",
  onCredits: "on credits",
  limitReset: "reset",
  token: "Token",
  account: "Account",
  assistantWorks: (owner) => `From ${owner}; works on your machines`,
  assistFrom: (owner) => `from ${owner}`,
  assistAgents: (owner) => `${owner}'s agents`,
  assistUntil: (at, now) => `until ${enMoment.short(at, now)}`,
  noFilePlace: "No machine can take the files",
  project: "Project",
  outsideProject: "No project",
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
  say: (agent) => (agent ? `Message ${agent}…` : "Write the first words…"),
  start: "Start",
  starting: "Starting…",
  sendingFiles: "Uploading files…",
  dropHere: "Drop files here",
  filesNotSent: "Some files failed to upload",
  notOpened: () => "Couldn't start",
};

export const zhPresence: PresenceWords = {
  asked: (who, what) =>
    `${who} ${{ answer: "问你", command: "想运行命令", change: "想改文件", tool: "想用工具", plan: "想按计划动手", form: "等你填写", other: "等你允许", waiting: "等你回复" }[what]}`,
  finished: (who) => `${who} 回复了`,
  urgent: (who) => `${who} 有要紧的事`,
  compacted: "对话已压缩",
  open: "打开",
  stop: "停止",
  arrived: (name, device, login) => `新${device ? "设备" : "机器"} ${name} 加入 ${login}`,
  arrivedManaged: (login) => `有受管机器用入网码加入 ${login}`,
  arrivedBody: "如果不是你添加的，请在账号页移除",
  dissolved: (hubs) => `${hubs.join("、")} 已解散`,
  dissolvedBody: "这台电脑已离开，Fastctx 和数据都还在",
  machineNotice: ({ machine, by, account, action: a }) => {
    const who = by ?? "中枢";
    switch (a.kind) {
      case "rename":
        return `${who} 把 ${machine} 改名为 ${a.to}`;
      case "updated":
        return `${who} 把 ${machine} 更新到 ${a.to}`;
      case "remove":
        return `${who} 把 ${machine} 移出了网络${a.then === "uninstall" ? "并卸载了 Fastctx" : a.then === "everything" ? "并彻底移除了 Fastctx" : ""}`;
      case "share":
        if (!a.shared && a.to === account) return by ? `${by} 收回了 ${machine} 给你的共享` : `${machine} 跑着中枢，不再共享给你`;
        return a.shared ? `${who} 把 ${machine} 共享给了 ${a.to}` : `${who} 收回了 ${machine} 给 ${a.to} 的共享`;
      case "assist":
        return a.assisting ? `${who} 让 ${machine} 上的 ${agentName(a.agent)} 协助 ${a.to}` : `${who} 结束了 ${machine} 上的 ${agentName(a.agent)} 对 ${a.to} 的协助`;
    }
  },
};

export const enPresence: PresenceWords = {
  asked: (who, what) =>
    `${who} ${{ answer: "asks", command: "wants to run a command", change: "wants to change files", tool: "wants to use a tool", plan: "wants to start on its plan", form: "needs a form filled in", other: "needs your OK", waiting: "is waiting for your reply" }[what]}`,
  finished: (who) => `${who} replied`,
  urgent: (who) => `${who} has something urgent`,
  compacted: "Conversation compacted",
  open: "Open",
  stop: "Stop",
  arrived: (name, device, login) => `New ${device ? "device" : "machine"} ${name} joined ${login}`,
  arrivedManaged: (login) => `Managed machines joined ${login} by a code`,
  arrivedBody: "If you didn't add it, remove it on the Account page",
  dissolved: (hubs) => `${hubs.join(", ")} ${hubs.length > 1 ? "were" : "was"} dissolved`,
  dissolvedBody: "This computer has left; Fastctx and your data are untouched",
  machineNotice: ({ machine, by, account, action: a }) => {
    const who = by ?? "The hub";
    switch (a.kind) {
      case "rename":
        return `${who} renamed ${machine} to ${a.to}`;
      case "updated":
        return `${who} updated ${machine} to ${a.to}`;
      case "remove":
        return `${who} took ${machine} off the network${a.then === "uninstall" ? " and uninstalled Fastctx" : a.then === "everything" ? " and removed Fastctx entirely" : ""}`;
      case "share":
        if (!a.shared && a.to === account) return by ? `${by} took back ${machine}'s share with you` : `${machine} runs the hub and is no longer shared with you`;
        return a.shared ? `${who} shared ${machine} with ${a.to}` : `${who} took back ${machine}'s share with ${a.to}`;
      case "assist":
        return a.assisting ? `${who} let ${agentName(a.agent)} on ${machine} assist ${a.to}` : `${who} stopped ${agentName(a.agent)} on ${machine} assisting ${a.to}`;
    }
  },
};

export interface SettingsWords {
  title: string;
  look: string;
  notify: string;
  notifyOn: string;
  notifyOff: string;
  /** How long this computer keeps a command's output once its end was
   *  delivered. */
  retention: string;
  /** This computer's data: a row, and the way to its page. */
  data: string;
  dataOpen: string;
  proxy: string;
  /** The proxy's label among this computer's facts. */
  proxyShort: string;
  proxyWhat: string;
  proxySystem: string;
  proxyChange: string;
  proxySave: string;
  proxyCancel: string;
  proxyWrong: string;
  proxyFailed: string;
  days(n: number): string;
  keys: string;
  keyList: [string, string][];
  theme: string;
  system: string;
  themes: Record<"warm-black" | "ink-black" | "warm-white", string>;
  language: string;
  textSize: string;
  sizeDefault: string;
  /** How much the interface moves: as the system asks, always, or not at
   *  all (`motion.ts`). */
  motion: string;
  motions: Record<"system" | "always" | "off", string>;
  sizes: Record<string, string>;
  device: string;
  machine: string;
  /** The same row on a phone, which logs in but is never a machine. */
  phoneName: string;
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
  dissolved: string;
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
  /** What a startup service does, when its state is pointed at. */
  serviceWhat: string;
  serviceOff: string;
  keep: string;
  keepAsks: string;
  kept: Record<"already" | "installed" | "declined" | "unsupported" | "removed" | "not_kept", string>;
  command(command: string): string;
  legacy: string;
  legacyCleaned(tidied: number, archived: number, place: string, waiting: number): string;
  legacyUnreadable(why: string): string;
  /** What became of an old fastctx, in a word; the sentence is said when pointed at. */
  legacyDone: string;
  legacyBusy(waiting: number): string;
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
  retention: "命令输出保留",
  data: "本机数据",
  dataOpen: "查看与管理",
  proxy: "连中枢的代理",
  proxyShort: "代理",
  proxyWhat: "这台电脑要经 HTTP 代理才连得上中枢服务器时填，比如 http://127.0.0.1:7890；清空就跟随系统",
  proxySystem: "跟随系统",
  proxyChange: "修改",
  proxySave: "保存",
  proxyCancel: "取消",
  proxyWrong: "写成 http://主机:端口",
  proxyFailed: "没改成",
  days: (n) => `${n} 天`,
  theme: "主题",
  system: "跟随系统",
  themes: { "warm-black": "暖黑", "ink-black": "墨黑", "warm-white": "暖白" },
  language: "语言",
  textSize: "字号",
  sizeDefault: "默认",
  motion: "动效",
  motions: { system: "跟随系统", always: "总是播放", off: "关闭" },
  sizes: { "14": "小", "15": "标准", "16": "大", "17": "特大" },
  device: "本机",
  machine: "机器名",
  phoneName: "手机名称",
  alone: "未加入网络",
  link: "中枢服务器",
  hub: (name) => `中枢服务器 ${name}`,
  as: (accounts) => (accounts ? `已登录 ${accounts}` : ""),
  version: "版本",
  online: (hub, since) => `已连接 ${hub}，${since} 起`,
  connecting: () => "连接中",
  stopped: () => "未连接",
  dissolved: "网络已解散",
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
    awaiting_administrator: "正在更新开机服务…",
    awaiting_legacy_exit: "等待旧版本退出，重启电脑后完成",
    updating_guardian: "正在更新开机服务…",
    returning_to_service: "正在交回开机服务…",
    complete: "已更新",
    failed: "更新失败",
  },
  renewFailed: failure(zhProblems, "更新失败"),
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
      "设置 › 应用启动管理 › Fastctx：关闭自动管理，允许自启动、关联启动、后台活动",
      "设置 › 电池优化 › 所有应用 › Fastctx：不允许",
      "电池 › 更多电池设置：休眠时始终保持网络连接",
      "最近任务中下拉 Fastctx 卡片锁定",
    ],
    xiaomi: ["应用详情 › 省电策略：无限制", "应用详情 › 自启动：开启", "最近任务中长按 Fastctx 卡片锁定"],
    other: [],
  },
  phoneOpenApp: "应用详情",
  serviceOn: "已开启",
  serviceWhat: "断网后自动重连",
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
  legacyDone: "已整理",
  legacyBusy: (waiting) => `${waiting} 项在用`,
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
  retention: "Keep command output",
  data: "Data",
  dataOpen: "View and manage",
  proxy: "Proxy to the hub",
  proxyShort: "Proxy",
  proxyWhat: "Where this computer reaches the hub only through an HTTP proxy, as http://127.0.0.1:7890; emptied, the system's settings again",
  proxySystem: "The system's",
  proxyChange: "Change",
  proxySave: "Save",
  proxyCancel: "Cancel",
  proxyWrong: "Write it as http://host:port",
  proxyFailed: "Not changed",
  days: (n) => (n === 1 ? "1 day" : `${n} days`),
  theme: "Theme",
  system: "System",
  themes: { "warm-black": "Warm black", "ink-black": "Ink black", "warm-white": "Warm white" },
  language: "Language",
  textSize: "Text size",
  motion: "Motion",
  motions: { system: "System", always: "Always", off: "Off" },
  sizeDefault: "Default",
  sizes: { "14": "Small", "15": "Standard", "16": "Large", "17": "Extra large" },
  device: "This device",
  machine: "Name",
  phoneName: "Phone name",
  alone: "Not in a network",
  link: "Hub",
  hub: (name) => `Hub ${name}`,
  as: (accounts) => (accounts ? `signed in to ${accounts}` : ""),
  version: "Version",
  online: (hub, since) => `connected to ${hub} since ${since}`,
  connecting: () => "connecting",
  stopped: () => "not connected",
  dissolved: "network dissolved",
  notInShell: "Browser preview, not connected",
  service: "Start at boot",
  worldVersion: "Local service",
  versionHere: (version) => version,
  versionDifferent: (relation) => (relation === "newer" ? "Local service is newer than the app" : relation === "older" ? "Local service is outdated" : "Local service doesn't match the app"),
  versionInstalled: "The installed program doesn't match the app",
  pathLookup: { windows: "Command Prompt", bash: "Git Bash", unix: "Terminal", codex: "Codex" },
  pathDifferent: (where, version) => `${where} uses another fastctx (${version})`,
  pathUnknown: (where) => `Can't tell which fastctx ${where} uses`,
  pathLegacy: "Don't run the old fastctx unapply: it deletes the whole .fastctx, Fastctx data included",
  pathTitle: "fastctx in your terminal",
  pathOk: "Got it",
  versionOld: (node, _nodeProtocol, client) => `Local service ${node} isn't compatible with this app (${client})`,
  versionNone: "Local service not running",
  renew: "Update",
  renewPhase: {
    staged: "Update ready",
    preparing: "Preparing…",
    waiting_for_idle: "Waiting for current work to pause…",
    awaiting_administrator: "Updating startup service…",
    awaiting_legacy_exit: "Waiting for the old version to exit; finishes after a restart",
    updating_guardian: "Updating startup service…",
    returning_to_service: "Handing back to the startup service…",
    complete: "Updated",
    failed: "Update failed",
  },
  renewFailed: failure(enProblems, "Update failed"),
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
      "Settings › App launch › Fastctx: turn off auto-manage; allow auto-launch, secondary launch and background activity",
      "Settings › Battery optimization › All apps › Fastctx: Don't allow",
      "Battery › More battery settings: stay connected while asleep",
      "Pull Fastctx's card down in Recents to lock it",
    ],
    xiaomi: ["App details › Battery saver: No restrictions", "App details › Autostart: on", "Long-press Fastctx's card in Recents to lock it"],
    other: [],
  },
  phoneOpenApp: "App details",
  serviceOn: "On",
  serviceWhat: "Reconnects by itself after the network drops",
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
  legacyDone: "Tidied",
  legacyBusy: (waiting) => `${waiting} in use`,
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
    readingShare: (percent) => `读取中… ${percent}%`,
    interrupted: conversationZh.watchInterrupted(null),
    brokenFor: (took, why) => conversationZh.brokenFor(took, why),
    took: (ms) => conversationZh.took(ms),
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
    readingShare: (percent) => `Reading… ${percent}%`,
    interrupted: conversationEn.watchInterrupted(null),
    brokenFor: (took, why) => conversationEn.brokenFor(took, why),
    took: (ms) => conversationEn.took(ms),
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
  /** An agent assisting the user: whose it is. */
  assistFrom: string;
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
  /** A stage of an act that was not done: where, how, and the World's
   *  own account of it. */
  unfinished(stage: string, outcome: "pending" | "unsupported" | "failed", detail: string): string;
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
  askedBy: (asker, label) => `${asker} 调用的模型：${label}`,
  here: "加入",
  since: (ago) => `${ago}前`,
  state: "状态",
  busy: (lasted) => `工作中　${lasted}`,
  idle: (ago) => `空闲　${ago}前`,
  removed: (ago) => `${ago}前已移除`,
  waiting: "等待回复",
  trouble: "异常",
  task: "任务",
  colleagues: "调用的模型",
  assistFrom: "来自",
  actingFor: "所属账号",
  separator: "、",
  login: "登录",
  running: "运行中的命令",
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
  unfinished: (stage, outcome) =>
    `${{ host: "宿主", records: "记录", tools: "工具", notification: "通知" }[stage] ?? stage}${{ pending: "等待中", unsupported: "不支持", failed: "失败" }[outcome]}`,
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
  askedBy: (asker, label) => `called by ${asker}: ${label}`,
  here: "Joined",
  since: (ago) => `${ago} ago`,
  state: "Status",
  busy: (lasted) => `working, ${lasted}`,
  idle: (ago) => `idle, ${ago} ago`,
  removed: (ago) => `removed ${ago} ago`,
  waiting: "waiting for your reply",
  trouble: "Issues",
  task: "Task",
  colleagues: "Models it called",
  assistFrom: "From",
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
  unfinished: (stage, outcome) =>
    `${{ host: "host", records: "records", tools: "tools", notification: "notification" }[stage] ?? stage} ${{ pending: "pending", unsupported: "unsupported", failed: "failed" }[outcome]}`,
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
  /** What the instructions are and how agents are given them. */
  whatTip: string;
  machines: string;
  places: string;
  /** Over the machines it spans and the places named on each. */
  machinesPlaces: string;
  noMachines: string;
  /** What the form still needs, said beside the field. */
  needName: string;
  needWhat: string;
  needMachine: string;
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
  what: "须知",
  whatPlaceholder: "这个项目里的智能体每次都会读到：它是做什么的、怎么做、要注意什么",
  whatTip: "写给在这个项目里干活的智能体：要照做的规矩，要知道的事。从 Fastctx 开的对话一开始就带着它，压缩之后也还在；改了以后，正在干活的对话下一次调用世界的工具时，会收到新的全文。",
  machines: "机器",
  places: "位置",
  machinesPlaces: "机器与位置",
  noMachines: "还没有选机器",
  needName: "写一个名称",
  needWhat: "写几句须知",
  needMachine: "至少选一台机器",
  addPlace: "位置",
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
  what: "Instructions",
  whatPlaceholder: "What every agent here reads each time: what this project is for, how to work in it, what to watch out for",
  whatTip: "For every agent working in this project: what to follow and what to know. Conversations opened from Fastctx start with them and keep them after compacting; once they change, a conversation at work gets the new text with its next call of a World tool.",
  machines: "Machines",
  places: "Locations",
  machinesPlaces: "Machines and locations",
  noMachines: "No machine chosen",
  needName: "Give it a name",
  needWhat: "Write a few instructions",
  needMachine: "Choose at least one machine",
  addPlace: "Location",
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
    "Claude 将直接运行命令、修改文件、访问网络，不再逐项确认，等同于 Claude Code 的「Bypass permissions」。可随时在智能体页撤销。",
  allow: "允许",
  withdrawTitle: (machine, account) => `撤销 Claude${account ? `（${account}）` : ""}在 ${machine} 上的授权？`,
  withdrawWhat: "撤销后，Fastctx 将无法在这台机器上启动 Claude，直到再次授权。",
  withdrawGo: "撤销",
  working: "处理中…",
  failed: failure(zhProblems, "操作失败"),
  close: "关闭",
};

export const enConsent: ConsentWords = {
  title: (machine, account) => `Allow Claude${account ? ` (${account})` : ""} to run unattended on ${machine}?`,
  what: () =>
    "Claude will run commands, edit files and use the network without asking each time, the same as Claude Code's “Bypass permissions”. Revoke anytime on the Agents page.",
  allow: "Allow",
  withdrawTitle: (machine, account) => `Revoke Claude's permission${account ? ` for ${account}` : ""} on ${machine}?`,
  withdrawWhat: "Fastctx won't be able to start Claude on this machine until you allow it again.",
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
  hint: "↑↓ 选择　Enter 打开　Esc 关闭",
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
  hint: "↑↓ to select, Enter to open, Esc to close",
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
  /** A command that waits for the turn under way to end. */
  afterTurn(name: string): string;
  /** A command the node took to do once the turn ends: beside it while it
   *  is done; withdrawing one; and one whose host did not say what came
   *  of it. */
  nowRunning: string;
  withdraw: string;
  unknownOutcome(name: string): string;
  /** What the conversation offers could not be read: a command typed is
   *  kept in the box, not sent. */
  noCommands: string;
  running(name: string): string;
  done(name: string): string;
  /** Carried out, with no word from the host on whether it took. */
  reported(name: string): string;
  /** The host refused, in its words; it is left on `model`. */
  refusedOn(said: string, model: string): string;
  /** The fork exists, but not all of the conversation came over; why is the hover. */
  forkIncomplete: string;
  failed(why: string): string;
  context(percent: number, used: number, window: number): string;
  usage: {
    title: string;
    context: string;
    contextUsed(used: number, window: number | null): string;
    limits: string;
    /** How old the limits shown are: they are what the host last pushed. */
    asOf(minutes: number): string;
    window(name: string): string;
    used(percent: string): string;
    resets(at: Date): string;
    /** How long until a window resets, under its ring. */
    resetsIn(ms: number): string;
    /** A window past the reset it was read before, under its ring. */
    reset: string;
    /** What it had used when it was read, before that reset. */
    usedBefore(percent: string): string;
    /** What Claude Code reckons the conversation has cost so far. */
    cost(usd: number): string;
    /** How that figure is reckoned, said where the pointer rests. */
    costWhy: string;
    none: string;
    /** How much of the last request's prompt the provider's cache served. */
    hit: string;
    /** How long the provider keeps the cache the last request left, where
     *  the host says, and once it is gone. */
    warm: string;
    cold: string;
    left(ms: number): string;
    ttl(ms: number): string;
    contextTitle(used: number, window: number | null): string;
    hitTitle(hit: string): string;
    warmTitle(left: string, ttl: string): string;
    coldTitle: string;
    /** The requests of the stretch chosen on the chart, how long it ran, and
     *  what its prompts and answers were made of. */
    requests(n: number, span: string): string;
    span(ms: number): string;
    kinds: { fresh: string; write: string; read: string; output: string };
    highest: string;
    lowest: string;
    /** Where the pointer rests on the chart: how much the cache served,
     *  and when. */
    readout(hit: string, time: string): string;
    stretch: string;
    /** Each step's context (`context-sheet.ts`): the way in, the page's
     *  title and figures, what grew it, a step as the pointer reads it,
     *  and the calls that took the most room. */
    steps: string;
    close: string;
    stepsNow: string;
    start: string;
    afterCompact: string;
    causes: { call: string; answer: string; other: string };
    compactions(n: number): string;
    compacted: string;
    stepSaid(step: number, context: string, grew: string): string;
    step(step: number): string;
    heaviest: string;
    heaviestWhy: string;
    /** Back from a step's own list to the heaviest calls. */
    backToHeaviest: string;
    /** What a step held past the rows shown. */
    rest(count: number): string;
    noCalls: string;
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
    resuming: "等待续传",
    failed: failure(zhProblems, "上传失败"),
    retry: "重试",
    size: (bytes) => sizeOf(bytes, ["B", "KB", "MB", "GB"]),
    wait: "文件上传完成后再发送",
    imagesOnly: "没有你的电脑在线，存不了这个文件",
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
    planGoWith: (kept) => `批准，${kept}`,
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
    world: "Fastctx",
    compact: "压缩对话",
    model: "切换模型",
    rename: "重命名",
    fork: "分叉对话",
    stop: "停止",
    goal: "设为目标",
    goalArgs: "<要它做成什么>",
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
    model: (model, effort) => `下一轮起使用 ${model}${effort ? `（${effort}）` : ""}`,
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
  afterTurn: (name) => `这一轮结束后才能 /${name}`,
  nowRunning: "正在做",
  withdraw: "撤回",
  unknownOutcome: (name) => `/${name} 不确定做成了没有`,
  noCommands: "没读到这段对话能用的命令，命令没送出",
  running: (name) => `正在执行 /${name}…`,
  done: (name) => `/${name} 已完成`,
  reported: (name) => `/${name} 已执行`,
  refusedOn: (said, model) => `${said}（当前模型 ${model}）`,
  forkIncomplete: "已分叉，部分内容没带过去",
  failed: failure(zhProblems, "执行失败"),
  context: (percent, used, window) => `上下文 ${percent}%（${used.toLocaleString()} / ${window.toLocaleString()}）`,
  usage: {
    title: "用量",
    context: "上下文",
    contextUsed: (used, window) => (window ? `${used.toLocaleString()} / ${window.toLocaleString()}` : used.toLocaleString()),
    limits: "额度",
    asOf: (minutes) =>
      minutes < 1 ? "截至刚刚" : minutes < 60 ? `截至 ${minutes} 分钟前` : minutes < 1440 ? `截至 ${Math.round(minutes / 60)} 小时前` : `截至 ${Math.round(minutes / 1440)} 天前`,
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
    resetsIn: (ms) => `${zhLeft(ms)}后重置`,
    reset: "已重置",
    usedBefore: (percent) => `重置前已用 ${percent}%`,
    cost: (usd) => (underCent(usd) ? "本次对话 不到 $0.01" : `本次对话 约 ${dollars(usd)}`),
    costWhy: "由 Claude Code 自己计算：它按内置的 Anthropic API 官方价目表，把每个请求的 token 折成美元，累加成会话总额；Fastctx 经官方 mod 接口的 session.measure 事件读取这个总额，按增量记账。订阅登录不实际扣这笔钱。",
    none: "暂无用量数据",
    hit: "缓存命中",
    warm: "缓存还热",
    cold: "已冷",
    left: (ms) => (ms < 60_000 ? `${Math.max(1, Math.ceil(ms / 1000))} 秒` : ms < 3_600_000 ? `${Math.ceil(ms / 60_000)} 分` : `${Math.round(ms / 3_600_000)} 小时`),
    ttl: (ms) => (ms >= 3_600_000 ? `${Math.round(ms / 3_600_000)} 小时缓存` : `${Math.round(ms / 60_000)} 分钟缓存`),
    contextTitle: (used, window) => (window ? `上下文 ${used.toLocaleString()} / ${window.toLocaleString()}，${Math.round((used / window) * 100)}%` : `上下文 ${used.toLocaleString()}`),
    hitTitle: (hit) => `最近一次请求，缓存命中 ${hit}`,
    warmTitle: (left, ttl) => `从最后一次回答算起，缓存还能留 ${left}（${ttl}）；过了这段时间，下一次请求要把整个上下文重新写进缓存`,
    coldTitle: "缓存已经过期，下一次请求要把整个上下文重新写进缓存",
    requests: (n, span) => `${n} 次请求，${span}`,
    span: (ms) => {
      const minutes = Math.round(ms / 60_000);
      if (minutes < 1) return "不到一分钟";
      if (minutes < 60) return `${minutes} 分钟`;
      const hours = Math.floor(minutes / 60);
      return minutes % 60 ? `${hours} 小时 ${minutes % 60} 分` : `${hours} 小时`;
    },
    kinds: { fresh: "新输入", write: "写缓存", read: "读缓存", output: "输出" },
    highest: "最高",
    lowest: "最低",
    readout: (hit, time) => `${hit}　${time}`,
    stretch: "选一段",
    steps: "每一步的上下文",
    close: "关闭",
    stepsNow: "现在",
    start: "开头",
    afterCompact: "压缩后留下的",
    causes: { call: "工具返回", answer: "回答", other: "其他" },
    compactions: () => "次压缩",
    compacted: "压缩",
    stepSaid: (step, context, grew) => [`第 ${step} 步`, `上下文 ${context}`, grew].filter(Boolean).join("　"),
    step: (step) => `第 ${step} 步`,
    heaviest: "最占地方的工具调用",
    heaviestWhy: "一步里几次调用各占多少，按各自返回了多少来分",
    backToHeaviest: "最占地方的",
    rest: (count) => `其余 ${count} 项`,
    noCalls: "这一步没有工具调用",
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
    resuming: "Waiting to resume",
    failed: failure(enProblems, "Upload failed"),
    retry: "Retry",
    size: (bytes) => sizeOf(bytes, ["B", "KB", "MB", "GB"]),
    wait: "Wait for uploads to finish",
    imagesOnly: "None of your computers is online to keep this file",
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
    planGoWith: (kept) => `Approve, ${kept}`,
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
    world: "Fastctx",
    compact: "Compact conversation",
    model: "Switch model",
    rename: "Rename",
    fork: "Fork conversation",
    stop: "Stop",
    goal: "Set a goal",
    goalArgs: "<what it should get done>",
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
    model: (model, effort) => `Using ${model}${effort ? ` (${effort})` : ""} from the next turn`,
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
  afterTurn: (name) => `/${name} can run once this turn ends`,
  nowRunning: "Running",
  withdraw: "Withdraw",
  unknownOutcome: (name) => `Not sure whether /${name} went through`,
  noCommands: "Couldn't read this conversation's commands; the command wasn't sent",
  running: (name) => `Running /${name}…`,
  done: (name) => `/${name} done`,
  reported: (name) => `/${name} ran`,
  refusedOn: (said, model) => `${said} (now on ${model})`,
  forkIncomplete: "Forked; part of it did not come over",
  failed: failure(enProblems, "Command failed"),
  context: (percent, used, window) => `Context ${percent}% (${used.toLocaleString()} / ${window.toLocaleString()})`,
  usage: {
    title: "Usage",
    context: "Context",
    contextUsed: (used, window) => (window ? `${used.toLocaleString()} / ${window.toLocaleString()}` : used.toLocaleString()),
    limits: "Limits",
    asOf: (minutes) =>
      minutes < 1 ? "as of just now" : minutes < 60 ? `as of ${minutes} min ago` : minutes < 1440 ? `as of ${Math.round(minutes / 60)} h ago` : `as of ${Math.round(minutes / 1440)} d ago`,
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
    resetsIn: (ms) => `resets in ${enLeft(ms)}`,
    reset: "reset",
    usedBefore: (percent) => `${percent}% used before it reset`,
    cost: (usd) => (underCent(usd) ? "This conversation under $0.01" : `This conversation about ${dollars(usd)}`),
    costWhy: "Calculated by Claude Code itself: it prices each request's tokens with its built-in Anthropic API price list and adds them into the session's total; Fastctx reads that total from the official mod API's session.measure event and records each increase. A subscription login is not charged this amount.",
    none: "No usage data",
    hit: "Cache hit",
    warm: "Cache warm",
    cold: "Cold",
    left: (ms) => (ms < 60_000 ? `${Math.max(1, Math.ceil(ms / 1000))}s` : ms < 3_600_000 ? `${Math.ceil(ms / 60_000)}m` : `${Math.round(ms / 3_600_000)}h`),
    ttl: (ms) => (ms >= 3_600_000 ? `${Math.round(ms / 3_600_000)}h cache` : `${Math.round(ms / 60_000)}m cache`),
    contextTitle: (used, window) => (window ? `Context ${used.toLocaleString()} / ${window.toLocaleString()}, ${Math.round((used / window) * 100)}%` : `Context ${used.toLocaleString()}`),
    hitTitle: (hit) => `Last request: ${hit} of the prompt served from the cache`,
    warmTitle: (left, ttl) => `The cache stays ${left} more from the last answer (${ttl}); after that the next request writes the whole context to it again`,
    coldTitle: "The cache has expired: the next request writes the whole context to it again",
    requests: (n, span) => `${n} requests, ${span}`,
    span: (ms) => {
      const minutes = Math.round(ms / 60_000);
      if (minutes < 1) return "under a minute";
      if (minutes < 60) return `${minutes} min`;
      const hours = Math.floor(minutes / 60);
      return minutes % 60 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
    },
    kinds: { fresh: "fresh input", write: "cache write", read: "cache read", output: "output" },
    highest: "highest",
    lowest: "lowest",
    readout: (hit, time) => `${hit}  ${time}`,
    stretch: "Choose a stretch",
    steps: "Context, step by step",
    close: "Close",
    stepsNow: "now",
    start: "to begin with",
    afterCompact: "left by compaction",
    causes: { call: "tool results", answer: "answers", other: "other" },
    compactions: (n) => `compaction${n === 1 ? "" : "s"}`,
    compacted: "compacted",
    stepSaid: (step, context, grew) => [`step ${step}`, `context ${context}`, grew].filter(Boolean).join(", "),
    step: (step) => `step ${step}`,
    heaviest: "Calls that took the most room",
    heaviestWhy: "Within a step, its calls share what it added by how much each returned",
    backToHeaviest: "Most room",
    rest: (count) => `${count} more`,
    noCalls: "No tool calls in this step",
  },
};

/** How long until something, as a ring's line says it: minutes, hours, days. */
function zhLeft(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} 小时`;
  const days = Math.floor(hours / 24);
  return hours % 24 ? `${days} 天 ${hours % 24} 小时` : `${days} 天`;
}

function enLeft(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h`;
  const days = Math.floor(hours / 24);
  return hours % 24 ? `${days} d ${hours % 24} h` : `${days} d`;
}

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
  app: "Fastctx",
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
  backTo: (page) => `返回${page}`,
  someone: (agent, machine) => `${machine} 上的 ${agent}`,
  someoneAssisting: (agent, owner) => `${owner} 的 ${agent}`,
  forked: "分叉出来的对话",
  colleagueFor: (asker) => `${asker} 调用的模型`,
  busy: (lasted) => `工作中　${lasted}`,
  idle: "空闲",
  waiting: "等待回复",
  offline: "离线",
  noProjects: "暂无项目",
  nobodyHere: "暂无对话",
  unreachable: "本机服务未连接",
  unanswered: (seconds) => `本机服务 ${seconds} 秒未响应`,
  thisOne: "本机",
  phone: "手机",
  logIn: "加入网络",
  networks: "网络",
  newNetwork: "新建网络…",
  joinNetwork: "加入网络…",
  joinAnother: "加入另一个网络…",
  admin: "管理员",
  member: "成员",
  machinesCount: (n) => `${n} 台机器`,
  account: "我",
  arrivals: (n) => `${n} 台新设备`,
  groupOf: (login) => login,
  more: (n) => `另外 ${n} 个`,
  span: zhSpan,
  agentsIn: (project) => `${project} 中的对话`,
  projectMore: "项目操作",
  projectNewTalk: "新对话",
  projectEdit: "编辑项目…",
  noTalks: "暂无对话",
  frame: { minimize: "最小化", maximize: "最大化", restore: "还原", close: "关闭" },
};

export const enShell: ShellWords = {
  app: "Fastctx",
  world: "Home",
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
  backTo: (page) => `Back to ${page}`,
  someone: (agent, machine) => `${agent} on ${machine}`,
  someoneAssisting: (agent, owner) => `${owner}'s ${agent}`,
  forked: "Forked from another conversation",
  colleagueFor: (asker) => `called by ${asker}`,
  busy: (lasted) => `working, ${lasted}`,
  idle: "idle",
  waiting: "waiting for your reply",
  offline: "offline",
  noProjects: "No projects yet",
  nobodyHere: "No conversations",
  unreachable: "Local service not connected",
  unanswered: (seconds) => `Local service not responding for ${seconds} s`,
  thisOne: "this computer",
  phone: "phone",
  logIn: "Join a network",
  networks: "Networks",
  newNetwork: "New network…",
  joinNetwork: "Join a network…",
  joinAnother: "Join another network…",
  admin: "Administrator",
  member: "Member",
  machinesCount: (n) => (n === 1 ? "1 machine" : `${n} machines`),
  account: "Me",
  arrivals: (n) => `${n} new device${n === 1 ? "" : "s"}`,
  groupOf: (login) => login,
  more: (n) => `${n} more`,
  span: enSpan,
  agentsIn: (project) => `Conversations in ${project}`,
  projectMore: "Project actions",
  projectNewTalk: "New conversation",
  projectEdit: "Edit project…",
  noTalks: "No conversations yet",
  frame: { minimize: "Minimize", maximize: "Maximize", restore: "Restore", close: "Close" },
};
