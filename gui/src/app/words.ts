// What the frame around every page says: the sidebar, the new conversation,
// the settings. In the user's words (`world-gui.md`, 用词).

import type { AskWords } from "../conversation/asks";
import type { FileWords } from "../conversation/mention";
import type { PresenceWords } from "./presence";
import type { ReaderWords } from "./reader";
import { sizeOf, type AttachWords } from "../conversation/attach";

export interface ShellWords {
  app: string;
  world: string;
  projects: string;
  outside: string;
  machines: string;
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
  asleep: string;
  offline: string;
  noProjects: string;
  nobodyHere: string;
  /** The World on this machine could not be asked. */
  unreachable: string;
  /** The World has been asked and has not answered for this many seconds. */
  unanswered(seconds: number): string;
  thisOne: string;
  phone: string;
  joinOrFound: string;
  more(n: number): string;
  span(ms: number): string;
  /** A project's fold button: the agents in it. */
  agentsIn(project: string): string;
  /** The window's caption buttons, where the shell draws them. */
  frame: { minimize: string; maximize: string; restore: string; close: string };
}

export interface NewTalkWords {
  title: string;
  machine: string;
  agent: string;
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
  notOpened(why: string): string;
}

export const zhNewTalk: NewTalkWords = {
  title: "新对话",
  machine: "在哪台机器",
  agent: "用哪一位",
  project: "进哪个项目",
  noProject: "不进项目",
  here: "这台",
  offline: "离线",
  noMachine: "没有在线的机器",
  model: "模型",
  effort: "推理强度",
  defaultModel: "默认",
  asking: "正在问…",
  unsure: "说不准能不能用",
  notInstalled: "没装",
  loggedOut: "没登录",
  firstWords: "对它说的第一句话…",
  hint: "Enter 开始 · Shift+Enter 换行",
  start: "开始",
  starting: "正在请它进来…",
  sendingFiles: "正在送文件…",
  dropHere: "放下，就随第一句话一起带过去",
  filesNotSent: "有文件没送过去，见上面；再试一次，或者拿掉它",
  cancel: "关闭",
  notOpened: (why) => `没能请它进来：${why}`,
};

export const enNewTalk: NewTalkWords = {
  title: "New conversation",
  machine: "On which machine",
  agent: "With whom",
  project: "In which project",
  noProject: "No project",
  here: "this one",
  offline: "offline",
  noMachine: "No machine is online",
  model: "Model",
  effort: "Reasoning",
  defaultModel: "default",
  asking: "asking…",
  unsure: "may or may not work",
  notInstalled: "not installed",
  loggedOut: "not logged in",
  firstWords: "The first thing to say…",
  hint: "Enter to begin · Shift+Enter for a new line",
  start: "Begin",
  starting: "Asking it in…",
  sendingFiles: "Sending files…",
  dropHere: "Drop to bring along with the first words",
  filesNotSent: "A file did not get there, see above; try again, or remove it",
  cancel: "Close",
  notOpened: (why) => `Could not ask it in: ${why}`,
};

export const zhPresence: PresenceWords = {
  waiting: (who) => `${who} 在等你回答`,
  waitingBody: (town) => (town ? `在项目 ${town} 里。打开它，看它要你答什么。` : "打开它，看它要你答什么。"),
  finished: (who) => `${who} 说完了`,
  finishedBody: (town, last) => [town ? `项目 ${town}` : null, last].filter(Boolean).join(" · ") || "打开它，看它说了什么。",
  open: "打开",
  stop: "停下",
};

export const enPresence: PresenceWords = {
  waiting: (who) => `${who} is waiting for you`,
  waitingBody: (town) => (town ? `In ${town}. Open it to see what it asks.` : "Open it to see what it asks."),
  finished: (who) => `${who} finished`,
  finishedBody: (town, last) => [town, last].filter(Boolean).join(" · ") || "Open it to see what it said.",
  open: "Open",
  stop: "Stop",
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
  sizes: Record<string, string>;
  device: string;
  machine: string;
  world: string;
  alone: string;
  link: string;
  version: string;
  online(hub: string, since: string): string;
  connecting(error: string | null): string;
  stopped(reason: string): string;
  notInShell: string;
  service: string;
  serviceOn: string;
  serviceOff: string;
  keep: string;
  keepAsks: string;
  release: string;
  releaseAgain: string;
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
    ["Ctrl+K", "命令面板：开对话、去某个地方、换主题"],
    ["Ctrl+N", "新对话"],
    ["Ctrl+B", "收起或展开侧栏"],
    ["Ctrl+F", "在这段对话里找"],
    ["Enter", "发出去；Shift+Enter 换行"],
    ["/", "在输入框开头：这段对话能用的命令"],
    ["Esc", "关掉菜单、卡片和查找"],
  ],
  notify: "通知",
  notifyOn: "在等你、说完了时叫我",
  notifyOff: "不叫我",
  theme: "主题",
  system: "跟随系统",
  themes: { "warm-black": "暖黑", "ink-black": "墨黑", "warm-white": "暖白" },
  language: "语言",
  textSize: "正文字号",
  sizes: { "14": "小", "15": "标准", "16": "大", "17": "更大" },
  device: "这台设备",
  machine: "机器名",
  world: "世界",
  alone: "独自一台",
  link: "与中枢",
  version: "版本",
  online: (hub, since) => `连着 ${hub}，从 ${since} 起`,
  connecting: (error) => (error ? `正在连，上一次没连上：${error}` : "正在连"),
  stopped: (reason) => `没有在连：${reason}`,
  notInShell: "在浏览器里看的是样例，没有连着世界。",
  service: "开机服务",
  serviceOn: "装着：这台电脑开机后，没人登录，世界也在这里",
  serviceOff: "没装：只有你登录着时，世界才在这里",
  keep: "装上",
  keepAsks: "要管理员确认一次",
  release: "拿掉",
  releaseAgain: "再按一次，拿掉开机服务",
  kept: {
    already: "已经装着",
    installed: "装上了",
    declined: "没有拿到管理员确认，什么都没改",
    unsupported: "这个系统上没有世界的开机服务",
    removed: "拿掉了：之后只有你登录着、有程序把它起来时，世界才在这里",
    not_kept: "本来就没有装",
  },
  command: (command) => `要用管理员身份跑这一条：${command}`,
  legacy: "旧版 fastctx",
  legacyCleaned: (tidied, archived, place, waiting) =>
    `它留下的：${tidied} 处原样未动的已收拾，${archived} 样归档在 ${place}，删不删由你${waiting > 0 ? `；还有 ${waiting} 样在用，等不用了再收` : ""}`,
  legacyUnreadable: (why) => `收拾它留下的东西的记录读不了：${why}`,
};

export const enSettings: SettingsWords = {
  title: "Settings",
  look: "Look",
  keys: "Keys",
  keyList: [
    ["Ctrl+K", "Palette: begin a conversation, go somewhere, change the theme"],
    ["Ctrl+N", "New conversation"],
    ["Ctrl+B", "Fold or unfold the sidebar"],
    ["Ctrl+F", "Find in this conversation"],
    ["Enter", "Send; Shift+Enter for a new line"],
    ["/", "At the start of the box: this conversation's commands"],
    ["Esc", "Close menus, cards and find"],
  ],
  notify: "Notifications",
  notifyOn: "Call me when an agent waits or finishes",
  notifyOff: "Don't",
  theme: "Theme",
  system: "As the system",
  themes: { "warm-black": "Warm black", "ink-black": "Ink black", "warm-white": "Warm white" },
  language: "Language",
  textSize: "Text size",
  sizes: { "14": "Small", "15": "Standard", "16": "Large", "17": "Larger" },
  device: "This device",
  machine: "Machine",
  world: "World",
  alone: "On its own",
  link: "To the hub",
  version: "Version",
  online: (hub, since) => `connected to ${hub} since ${since}`,
  connecting: (error) => (error ? `connecting; the last try failed: ${error}` : "connecting"),
  stopped: (reason) => `not connecting: ${reason}`,
  notInShell: "In a browser this is a specimen, not connected to the World.",
  service: "Startup service",
  serviceOn: "Installed: the World is here from boot, before anyone signs in",
  serviceOff: "Not installed: the World is here only while you are signed in",
  keep: "Install",
  keepAsks: "Asks an administrator once",
  release: "Remove",
  releaseAgain: "Press again to remove the startup service",
  kept: {
    already: "Already installed",
    installed: "Installed",
    declined: "No administrator approved it; nothing changed",
    unsupported: "This system has no startup service for the World",
    removed: "Removed: the World is here only while you are signed in and something starts it",
    not_kept: "There was none to remove",
  },
  command: (command) => `Run this as root: ${command}`,
  legacy: "Old fastctx",
  legacyCleaned: (tidied, archived, place, waiting) =>
    `What it left: ${tidied} untouched files tidied, ${archived} archived in ${place}, yours to delete${waiting > 0 ? `; ${waiting} still in use, put away once they are not` : ""}`,
  legacyUnreadable: (why) => `The record of tidying what it left could not be read: ${why}`,
};

export interface MachinesWords {
  title: string;
  here: string;
  online(since: string): string;
  offline(since: string | null): string;
  unknown: string;
  revoked(by: string, reason: string): string;
  factsUnknown(why: string): string;
  agents: string;
  nobody: string;
  jobs(n: number): string;
  home: string;
  via(route: string): string;
  phone: string;
  device: string;
  hosts: string;
  asking: string;
  notAsked(why: string): string;
  notInstalled: string;
  loggedIn: string;
  loggedOut: string;
  loginUnknown: string;
  toolsIn: string;
  toolsOut: string;
  toolsOld: string;
  putIn: string;
  takeOut: string;
  renew: string;
  applied: string;
  notApplied(why: string): string;
}

export const zhMachines: MachinesWords = {
  phone: "手机",
  device: "一部带着你身份的设备，上面不跑任何东西",
  hosts: "上面的两家智能体",
  asking: "正在问它上面的智能体…",
  notAsked: (why) => `问不到它上面的智能体：${why}`,
  notInstalled: "没装",
  loggedIn: "已登录",
  loggedOut: "没登录",
  loginUnknown: "登没登录说不准",
  toolsIn: "你在这里开的会话用着世界的工具",
  toolsOut: "你在这里开的会话没接入世界的工具",
  toolsOld: "接入的还是旧版 fastctx",
  putIn: "接入",
  takeOut: "拿出",
  renew: "换成新版",
  applied: "改好了",
  notApplied: (why) => `没改成：${why}`,
  title: "机器",
  here: "这台",
  online: (since) => `在线，${since}起`,
  offline: (since) => (since ? `离线，${since}起` : "离线"),
  unknown: "不知道在不在线",
  revoked: (by, reason) => `已被 ${by} 吊销：${reason}`,
  factsUnknown: (why) => `问不到它的情况：${why}`,
  agents: "上面的智能体",
  nobody: "此刻没有智能体",
  jobs: (n) => `${n} 个作业在跑`,
  home: "家目录",
  via: (route) => `经${route}连着中枢`,
};

export const enMachines: MachinesWords = {
  phone: "phone",
  device: "A device that carries you; nothing runs on it",
  hosts: "Its agents",
  asking: "Asking about its agents…",
  notAsked: (why) => `Its agents could not be asked about: ${why}`,
  notInstalled: "not installed",
  loggedIn: "logged in",
  loggedOut: "not logged in",
  loginUnknown: "login not known",
  toolsIn: "sessions you open here have the World's tools",
  toolsOut: "sessions you open here don't have the World's tools",
  toolsOld: "still has the old fastctx",
  putIn: "Put in",
  takeOut: "Take out",
  renew: "Use the new one",
  applied: "Done",
  notApplied: (why) => `Not done: ${why}`,
  title: "Machines",
  here: "this one",
  online: (since) => `online since ${since}`,
  offline: (since) => (since ? `offline since ${since}` : "offline"),
  unknown: "not known whether online",
  revoked: (by, reason) => `revoked by ${by}: ${reason}`,
  factsUnknown: (why) => `could not be asked: ${why}`,
  agents: "Agents on it",
  nobody: "No agent right now",
  jobs: (n) => `${n} job${n === 1 ? "" : "s"} running`,
  home: "Home",
  via: (route) => `reaches the hub ${route}`,
};

export interface ReaderWordsSet {
  zh: ReaderWords;
  en: ReaderWords;
}

const units: Record<"zh" | "en", [string, string, string, string]> = { zh: ["字节", "KB", "MB", "GB"], en: ["bytes", "KB", "MB", "GB"] };

export const readerWords: ReaderWordsSet = {
  zh: {
    reread: "重新读",
    reading: "正在读…",
    tail: (shown, size) => `只显示最后 ${shown}（共 ${size}）`,
    notText: (size) => `它不是 UTF-8 文本（${size}），这里不猜着解码`,
    failed: (why) => `读不到：${why}`,
    bytes: (count) => sizeOf(count, units.zh),
    modified: (ago) => `${ago}改过`,
  },
  en: {
    reread: "Read again",
    reading: "Reading…",
    tail: (shown, size) => `only the last ${shown} of ${size}`,
    notText: (size) => `It is not UTF-8 text (${size}); it is not decoded by guess here`,
    failed: (why) => `Could not read it: ${why}`,
    bytes: (count) => sizeOf(count, units.en),
    modified: (ago) => `changed ${ago}`,
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
  asleep(ago: string): string;
  waiting: string;
  trouble: string;
  task: string;
  colleagues: string;
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
  sleep: string;
  wake: string;
  /** Why an agent in the middle of a turn cannot be put to sleep. */
  cannotSleepBusy: string;
  slept: string;
  woke: string;
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
  cancel: string;
  working: string;
  failed(why: string): string;
}

export const zhDetails: DetailsWords = {
  open: "关于它",
  close: "收起",
  model: "模型",
  modelSelf: "（自报）",
  machine: "在哪台机器",
  host: "程序",
  startedByWorld: "世界拉起的后台会话",
  origin: "来历",
  openedByYou: "你开启的",
  askedBy: (asker, label) => `受 ${asker} 之托：${label}`,
  here: "来了",
  since: (ago) => `${ago}前`,
  state: "此刻",
  busy: (lasted) => `在干活，已 ${lasted}`,
  idle: (ago) => `空闲，${ago}前说完`,
  asleep: (ago) => `睡着了，${ago}前`,
  waiting: "在等你回答",
  trouble: "出过的状况",
  task: "它接的活",
  colleagues: "它请的同事",
  running: "它起的、还在跑的",
  actions: (shown, total) => (shown < total ? `最近的动作（${total} 个里的最后 ${shown} 个）` : "它的动作"),
  noActions: "还没有动作。",
  answers: "交回过的",
  records: "记录在",
  stop: "停下这一轮",
  stopped: "已请它停下",
  notRunning: "它此刻没有在跑的一轮",
  exportMarkdown: "复制整段对话（Markdown）",
  exported: "整段对话已复制为 Markdown",
  openOwn: (agent) => `在 ${agent} 自己的界面里打开`,
  openedOwn: "已在一个新的终端窗口里打开",
  cannotLook: (why) => `问不到它的详情：${why}`,
  notFound: "世界里没有找到它。",
  manage: "管理",
  sleep: "让它睡着",
  wake: "叫醒它",
  cannotSleepBusy: "它正在跑一轮，跑完了才能睡",
  slept: "它睡着了；谁对它说话，它就醒来",
  woke: "它醒了，在场",
  move: "换项目",
  moveTo: "换到哪个项目",
  inHere: "在这里",
  leaveProject: "离开项目",
  noOtherProject: "这台机器上没有别的项目",
  moved: (project) => `已替它换到「${project}」，并照实告诉了它是你换的`,
  left: "已替它离开项目，并照实告诉了它是你做的",
  remove: "移除",
  removeTitle: (who) => `移除 ${who}？`,
  removeWhat:
    "世界不再把它列为居民，界面与 fastctx w 里都看不到它了。它的记录挪进归档，一个字都不删；它起的、还在跑的作业照跑，列进没人看着的那一段。",
  removeGo: "移除",
  cancel: "关闭",
  working: "正在办…",
  failed: (why) => `没办成：${why}`,
};

export const enDetails: DetailsWords = {
  open: "About it",
  close: "Close",
  model: "Model",
  modelSelf: " (self-reported)",
  machine: "Machine",
  host: "Program",
  startedByWorld: "a background session the World started",
  origin: "Came",
  openedByYou: "opened by you",
  askedBy: (asker, label) => `for ${asker}: ${label}`,
  here: "Here",
  since: (ago) => `${ago} ago`,
  state: "Now",
  busy: (lasted) => `working, for ${lasted}`,
  idle: (ago) => `idle, finished ${ago} ago`,
  asleep: (ago) => `asleep, since ${ago} ago`,
  waiting: "waiting for your answer",
  trouble: "Trouble",
  task: "Its task",
  colleagues: "Colleagues it asked",
  running: "Still running, started by it",
  actions: (shown, total) => (shown < total ? `Latest actions (last ${shown} of ${total})` : "Its actions"),
  noActions: "No actions yet.",
  answers: "What it handed back",
  records: "Records",
  stop: "Stop this turn",
  stopped: "Asked to stop",
  notRunning: "It is running no turn now",
  exportMarkdown: "Copy the conversation as Markdown",
  exported: "The conversation is copied as Markdown",
  openOwn: (agent) => `Open in ${agent}'s own interface`,
  openedOwn: "Opened in a new terminal window",
  cannotLook: (why) => `Its details could not be asked: ${why}`,
  notFound: "It was not found in the World.",
  manage: "Manage",
  sleep: "Put it to sleep",
  wake: "Wake it",
  cannotSleepBusy: "It is in the middle of a turn; it can sleep once the turn ends",
  slept: "It is asleep; speaking to it wakes it",
  woke: "It is awake and here",
  move: "Move to a project",
  moveTo: "Which project",
  inHere: "in it now",
  leaveProject: "Out of any project",
  noOtherProject: "No other project spans this machine",
  moved: (project) => `Moved it to “${project}” for you, and told it so plainly`,
  left: "Took it out of the project for you, and told it so plainly",
  remove: "Remove",
  removeTitle: (who) => `Remove ${who}?`,
  removeWhat:
    "The World stops listing it as a resident; it no longer shows here or in fastctx w. Its records are moved to the archive, nothing deleted; jobs it started keep running and are listed with the unattended ones.",
  removeGo: "Remove",
  cancel: "Close",
  working: "Working…",
  failed: (why) => `Not done: ${why}`,
};

export interface ProjectFormWords {
  newTitle: string;
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
  newTitle: "新项目",
  editTitle: (name) => `修改「${name}」`,
  name: "名字",
  namePlaceholder: "给它起个名字，比如 mark50 研发",
  slug: "短名",
  slugHint: "小写字母、数字与连字符，以字母开头；命令里、目录里用它，开了就不再改",
  what: "它是什么",
  whatPlaceholder: "一两句话，说这个项目为了什么",
  machines: "横跨哪几台机器",
  places: "在哪里能找到什么",
  placesHint: "一行一样东西：它是什么、在哪台机器、原生的绝对路径",
  placeWhat: "是什么",
  placePath: "路径",
  placeMachine: "在哪台机器",
  addPlace: "再加一行",
  removePlace: "删掉这一行",
  canon: "定论在哪",
  canonHint: "存放定论的文件夹",
  canonMachine: "定论在哪台机器",
  noCanon: "不写",
  open: "开这个项目",
  save: "存成新的一版",
  saving: "正在写…",
  cancel: "关闭",
  failed: (why) => `没能写下：${why}`,
};

export const enProjectForm: ProjectFormWords = {
  newTitle: "New project",
  editTitle: (name) => `Edit “${name}”`,
  name: "Name",
  namePlaceholder: "Name it, such as Mark 50 research",
  slug: "Short name",
  slugHint: "lowercase letters, digits and hyphens, starting with a letter; used in commands and directories, and never changed",
  what: "What it is",
  whatPlaceholder: "A sentence or two on what the project is for",
  machines: "Machines it spans",
  places: "Where things are",
  placesHint: "One thing a line: what it is, on which machine, its native absolute path",
  placeWhat: "What",
  placePath: "Path",
  placeMachine: "On which machine",
  addPlace: "Add a line",
  removePlace: "Remove this line",
  canon: "Decisions",
  canonHint: "the folder holding the decisions",
  canonMachine: "Machine the decisions are on",
  noCanon: "none",
  open: "Open the project",
  save: "Save as the next version",
  saving: "Writing…",
  cancel: "Close",
  failed: (why) => `Could not write it: ${why}`,
};

export interface DeviceWords {
  invite: string;
  inviteTitle: string;
  inviteWhat: string;
  aComputer: string;
  aPhone: string;
  computerHint: string;
  phoneHint: string;
  phoneSteps: string[];
  inviteName: string;
  inviteNameHint: string;
  valid: string;
  qr: string;
  hours(n: number): string;
  make: string;
  making: string;
  code: string;
  copy: string;
  copied: string;
  until(at: string): string;
  howTo: string;
  howToSteps: string[];
  again: string;
  revoke: string;
  revokeTitle(name: string): string;
  revokeWhat(name: string): string;
  reason: string;
  reasonDefault: string;
  revokeGo: string;
  leave: string;
  leaveTitle: string;
  leaveWhat: string;
  leaveGo: string;
  working: string;
  close: string;
  failed(why: string): string;
  specimen: string;
}

export const zhDevices: DeviceWords = {
  invite: "邀请一台设备",
  inviteTitle: "邀请一台设备",
  inviteWhat: "它是",
  aComputer: "一台电脑",
  aPhone: "一部手机",
  computerHint: "智能体可以住在它上面，世界会在它上面干活。",
  phoneHint: "它带着你的身份进来，只用来看和说话，上面不跑任何东西。",
  inviteName: "它叫什么",
  inviteNameHint: "小写字母、数字与连字符；不写就由它自己起名",
  valid: "多久内有效",
  qr: "邀请码的二维码，用新设备扫它",
  hours: (n) => (n < 24 ? `${n} 小时` : `${n / 24} 天`),
  make: "生成邀请码",
  making: "正在生成…",
  code: "邀请码",
  copy: "复制",
  copied: "已复制",
  until: (at) => `${at}之前有效，只能用一次`,
  howTo: "在新设备上",
  phoneSteps: [
    "在手机上装好 World，打开它，扫这个码；扫不了就粘贴邀请码。",
    "它以你的身份进来：能看、能说话、能管，但不在它上面跑任何东西。",
    "要它在后台也连着，按它的提示在手机的设置里放行后台运行。",
  ],
  howToSteps: [
    "装好 World，打开它，选「加入已有的世界」，扫这个码或粘贴邀请码；",
    "或者在它的终端里敲 fastctx join <邀请码>。",
    "加入时会请一次管理员确认，装上开机自启的服务，之后它就一直连着。",
  ],
  again: "再生成一个",
  revoke: "吊销",
  revokeTitle: (name) => `吊销 ${name}`,
  revokeWhat: (name) => `从此每台机器都不再认 ${name} 的钥匙：它不能再说话、收信，也不能再回来，除非重新邀请。它身上的东西不会被删掉。丢了的设备就这样处理。`,
  reason: "原因",
  reasonDefault: "revoked",
  revokeGo: "吊销它",
  leave: "离开世界",
  leaveTitle: "让这台离开世界",
  leaveWhat: "这台机器吊销自己的钥匙，回到独自一台。它身上的居民与记录都留着；别的机器从此不再认它。要回来，得重新被邀请。",
  leaveGo: "离开",
  working: "正在办…",
  close: "关闭",
  failed: (why) => `没办成：${why}`,
  specimen: "这是样例，没有连着世界",
};

export const enDevices: DeviceWords = {
  invite: "Invite a device",
  inviteTitle: "Invite a device",
  inviteWhat: "It is",
  aComputer: "A computer",
  aPhone: "A phone",
  computerHint: "Agents can live on it, and the World works on it.",
  phoneHint: "It comes in as you, to look and speak; nothing runs on it.",
  inviteName: "Its name",
  inviteNameHint: "lowercase letters, digits and hyphens; left empty, it names itself",
  valid: "Valid for",
  qr: "The invitation as a QR code, for the new device to scan",
  hours: (n) => (n < 24 ? `${n} hour${n === 1 ? "" : "s"}` : `${n / 24} day${n === 24 ? "" : "s"}`),
  make: "Make the code",
  making: "Making it…",
  code: "Invitation code",
  copy: "Copy",
  copied: "Copied",
  until: (at) => `valid until ${at}, once`,
  howTo: "On the new device",
  phoneSteps: [
    "Install World on the phone, open it, and scan this code; paste the code where it cannot scan.",
    "It comes in as you: it can look, speak and manage, and nothing runs on it.",
    "To stay connected in the background, allow it in the phone's settings as it asks.",
  ],
  howToSteps: [
    "Install World, open it, choose “Join a World”, and scan this code or paste it;",
    "or run fastctx join <code> in its terminal.",
    "Joining asks for an administrator once to install the service that keeps it connected.",
  ],
  again: "Make another",
  revoke: "Revoke",
  revokeTitle: (name) => `Revoke ${name}`,
  revokeWhat: (name) => `No machine will trust ${name}'s key again: it can no longer speak, receive letters or come back unless invited anew. Nothing on it is deleted. This is what to do with a lost device.`,
  reason: "Reason",
  reasonDefault: "revoked",
  revokeGo: "Revoke it",
  leave: "Leave the World",
  leaveTitle: "Have this machine leave the World",
  leaveWhat: "This machine revokes its own key and becomes a World of one. Its residents and records stay; the other machines stop trusting it. To come back it must be invited again.",
  leaveGo: "Leave",
  working: "Working…",
  close: "Close",
  failed: (why) => `It did not go through: ${why}`,
  specimen: "This is a specimen, not connected to the World",
};

export interface PaletteWords {
  placeholder: string;
  nothing: string;
  actions: string;
  places: string;
  agents: string;
  machines: string;
  theme(name: string): string;
  fold: string;
  hint: string;
}

export const zhPalette: PaletteWords = {
  placeholder: "去哪里，做什么…",
  nothing: "没有对得上的",
  actions: "做",
  places: "地方",
  agents: "智能体",
  machines: "机器",
  theme: (name) => `主题：${name}`,
  fold: "收起或打开侧栏",
  hint: "↑↓ 选择 · Enter 前往 · Esc 关闭",
};

export const enPalette: PaletteWords = {
  placeholder: "Go somewhere, do something…",
  nothing: "Nothing matches",
  actions: "Do",
  places: "Places",
  agents: "Agents",
  machines: "Machines",
  theme: (name) => `Theme: ${name}`,
  fold: "Fold or open the sidebar",
  hint: "↑↓ to choose · Enter to go · Esc to close",
};

export interface WelcomeWords {
  lede: string;
  thisComputer: string;
  thisPhone: string;
  facts(name: string, world: string | null): string;
  placed: string;
  notPlaced: string;
  place: string;
  noCarried: string;
  serviceOn: string;
  serviceOff: string;
  join: string;
  joinWhat: string;
  joinPhoneWhat: string;
  phoneJoined: string;
  found: string;
  foundWhat: string;
  alone: string;
  aloneWhat: string;
  code: string;
  codeHint: string;
  /** On a phone: read the invitation's QR code with the camera. */
  scan: string;
  scanFailed(why: string): string;
  /** Above the pasted code, where scanning comes first. */
  codeOr: string;
  name: string;
  nameHint: string;
  joinGo: string;
  joining: string;
  hubWhere: string;
  hubHere: string;
  hubServer: string;
  address: string;
  addressHint: string;
  hubGo: string;
  founding: string;
  serverSteps: string[];
  foundingCode: string;
  joined(world: string, name: string): string;
  kept: Record<"already" | "installed" | "declined" | "unsupported", string>;
  keptCommand: string;
  hosts: string;
  hostsWhat: string;
  codex: string;
  claude: string;
  hostState: Record<"absent" | "out" | "in" | "old", string>;
  applyGo: string;
  applying: string;
  enter: string;
  failed(why: string): string;
  back: string;
}

export const zhWelcome: WelcomeWords = {
  lede: "让你的几台电脑，和住在上面的 Codex、Claude，在同一个世界里。",
  thisComputer: "这台电脑",
  thisPhone: "这部手机",
  facts: (name, world) => (world ? `叫 ${name}，在世界 ${world} 里` : `叫 ${name}，此刻独自一台`),
  placed: "fastctx 已就位",
  notPlaced: "fastctx 还没放好",
  place: "放好它",
  noCarried: "这一份 World 没有随身带 fastctx",
  serviceOn: "开机自启的服务在",
  serviceOff: "还没有开机自启的服务",
  join: "加入已有的世界",
  joinWhat: "另一台设备在 World 里「邀请一台设备」，会给你一个邀请码。",
  joinPhoneWhat: "在电脑上的 World 里「邀请一台设备」、选「一部手机」，扫它给出的二维码，或者把邀请码粘贴到这里。",
  phoneJoined: "这部手机现在带着自己的钥匙，以你的身份看着世界、和智能体说话；上面什么都不跑。",
  found: "建一个新世界",
  foundWhat: "要有一个中枢，别的设备都连向它：可以是这台电脑，也可以是一台服务器。",
  alone: "先独自用",
  aloneWhat: "这台电脑本身就是一个世界。以后随时可以加入或建一个。",
  code: "邀请码",
  codeHint: "以 fxw3. 开头的一长串，粘贴进来",
  scan: "扫电脑上的二维码",
  codeOr: "或者粘贴邀请码",
  scanFailed: (why) => `没扫成：${why}`,
  name: "这台电脑在世界里叫什么",
  nameHint: "小写字母、数字与连字符；不写就用它现在的名字",
  joinGo: "加入",
  joining: "正在加入，稍后会请一次管理员确认…",
  hubWhere: "中枢放在哪",
  hubHere: "这台电脑",
  hubServer: "一台服务器",
  address: "别的设备够得着这台电脑的地址",
  addressHint: "IP 加端口，比如 203.0.113.5:7443；它要一直开着，别的设备才连得上",
  hubGo: "在这台电脑上建中枢",
  founding: "正在建，稍后会请一次管理员确认…",
  serverSteps: [
    "在服务器上装好 fastctx（npm install -g fastctx，或从 GitHub 的发布页下载）；",
    "运行 fastctx hub init --public <服务器的公网 IP>:7443，它会打印一个建世界码；",
    "运行 sudo fastctx service hub install，让中枢开机自启、一直在；",
    "把那个建世界码粘贴到上面「加入已有的世界」里，这台电脑就是这个世界的第一台。",
  ],
  foundingCode: "建世界码",
  joined: (world, name) => `已经在世界 ${world} 里，这台叫 ${name}。`,
  kept: {
    already: "开机自启的服务早就在了。",
    installed: "开机自启的服务装好了：从此开机就连着，不管有没有人登录。",
    declined: "没有拿到管理员确认，所以没装开机服务：只有你登录着时它才连着。以后可以在设置里再装。",
    unsupported: "这个系统没有开机服务：只有你登录着时它才连着。",
  },
  keptCommand: "要它开机就连着，在终端里运行：",
  hosts: "把世界的工具接进这台电脑上的智能体",
  hostsWhat: "接进去之后，你在这台电脑上自己开的 Codex 与 Claude Code 也能用世界的手、看见世界。随时可以拿出来。",
  codex: "Codex",
  claude: "Claude Code",
  hostState: { absent: "没装", out: "没接入", in: "已接入", old: "里面是旧版 fastctx" },
  applyGo: "接入",
  applying: "正在接入…",
  enter: "进入世界",
  failed: (why) => `没办成：${why}`,
  back: "返回",
};

export const enWelcome: WelcomeWords = {
  lede: "Your computers, and the Codex and Claude living on them, in one World.",
  thisComputer: "This computer",
  thisPhone: "This phone",
  facts: (name, world) => (world ? `named ${name}, in the World ${world}` : `named ${name}, on its own for now`),
  placed: "fastctx is in place",
  notPlaced: "fastctx is not in place yet",
  place: "Put it in place",
  noCarried: "this build of World carries no fastctx",
  serviceOn: "a service keeps it running",
  serviceOff: "no service keeps it running yet",
  join: "Join a World",
  joinWhat: "Another device's World gives you an invitation code under “Invite a device”.",
  joinPhoneWhat: "In World on a computer, invite a device, choose a phone, then scan its QR code or paste the invitation here.",
  phoneJoined: "This phone now carries its own key: it watches the World and speaks to agents as you, and runs nothing.",
  found: "Found a new World",
  foundWhat: "It needs a hub every device connects to: this computer, or a server.",
  alone: "Stay on its own",
  aloneWhat: "This computer is a World by itself. You can join or found one any time.",
  code: "Invitation code",
  codeHint: "a long string starting with fxw3.; paste it here",
  scan: "Scan the QR code on the computer",
  codeOr: "Or paste the invitation",
  scanFailed: (why) => `Could not scan: ${why}`,
  name: "This computer's name in the World",
  nameHint: "lowercase letters, digits and hyphens; left empty, it keeps its name",
  joinGo: "Join",
  joining: "Joining; an administrator will be asked once…",
  hubWhere: "Where the hub is",
  hubHere: "This computer",
  hubServer: "A server",
  address: "An address other devices reach this computer at",
  addressHint: "IP and port, such as 203.0.113.5:7443; it must stay on for the others to connect",
  hubGo: "Found the hub here",
  founding: "Founding; an administrator will be asked once…",
  serverSteps: [
    "Install fastctx on the server (npm install -g fastctx, or from the GitHub releases);",
    "run fastctx hub init --public <the server's public IP>:7443; it prints a founding code;",
    "run sudo fastctx service hub install, so the hub starts at boot and stays up;",
    "paste the founding code into “Join a World” above; this computer is the World's first.",
  ],
  foundingCode: "Founding code",
  joined: (world, name) => `In the World ${world}, as ${name}.`,
  kept: {
    already: "A service already keeps it running.",
    installed: "A service now keeps it running: connected from boot, signed in or not.",
    declined: "Administrator rights were declined, so no service was installed: it is connected only while you are signed in. You can install it later in the settings.",
    unsupported: "This system has no service for it: it is connected only while you are signed in.",
  },
  keptCommand: "To keep it connected from boot, run in a terminal:",
  hosts: "Put the World's tools into the agents on this computer",
  hostsWhat: "Then the Codex and Claude Code you open on this computer yourself use the World's hands and see the World. You can take them out any time.",
  codex: "Codex",
  claude: "Claude Code",
  hostState: { absent: "not installed", out: "not in", in: "in", old: "an old fastctx is in it" },
  applyGo: "Put them in",
  applying: "Putting them in…",
  enter: "Enter the World",
  failed: (why) => `It did not go through: ${why}`,
  back: "Back",
};

export interface ComposerWords {
  asks: AskWords;
  attach: AttachWords;
  files: FileWords;
  commands: {
    world: string;
    compact: string;
    model: string;
    rename: string;
    fork: string;
    stop: string;
    noCommands: string;
    effort: string;
    current: string;
  };
  modelChip: string;
  noModels: string;
  /** Rewinding to the user's words: what it drops and what it does not. */
  rewindAsk: string;
  rewindGo: string;
  forkAsk: string;
  forkGo: string;
  never: string;
  unknown(name: string): string;
  running(name: string): string;
  done(name: string): string;
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
    hint: "敲文件名的几个字",
    searching: "正在找…",
    none: "那台机器上、这个项目的地方里没有这样的文件",
  },
  attach: {
    attach: "附上文件",
    drop: (agent) => `放下，就附给 ${agent}`,
    remove: "拿掉",
    sending: "正在送过去…",
    failed: (why) => `没送过去：${why}`,
    retry: "再试一次",
    size: (bytes) => sizeOf(bytes, ["B", "KB", "MB", "GB"]),
    wait: "附上的文件还在送，送到了再发",
  },
  asks: {
    waiting: "在等你回答",
    command: (machine) => `想在 ${machine} 上跑一条命令`,
    stdin: (machine) => `想往 ${machine} 上一个在跑的程序里送输入`,
    network: (host, protocol) => `想连上 ${host}${protocol ? `（${protocol}）` : ""}`,
    files: "想改文件",
    filesNamed: (count) => `想改 ${count} 个文件`,
    grantRoot: (root) => `并请你允许它在这段对话里随意写 ${root} 下的东西`,
    reason: (why) => `它说：${why}`,
    inDir: (cwd) => `在 ${cwd}`,
    accept: "允许",
    acceptForSession: "这段对话里都允许",
    acceptPrefix: (prefix) => `以后 ${prefix} 开头的都允许`,
    networkRule: (allow, host) => (allow ? `以后都允许连 ${host}` : `以后都不许连 ${host}`),
    decline: "不允许，让它接着做",
    cancel: "不允许，停下这一轮",
    question: "它有一个问题",
    questions: (count) => `它有 ${count} 个问题`,
    several: "可以选几个",
    otherAnswer: "或者写下你自己的回答",
    secret: "写下的字不会显示出来",
    submit: "回答",
    fill: "提交",
    skip: "跳过",
    form: (server) => `${server} 想请你填一张表`,
    link: (server) => `${server} 请你打开一个网址`,
    openLink: "打开网址，接着做",
    refuse: "拒绝",
    dismiss: "先不管",
    required: "这一项要填",
    requiredOf: (name) => `这一项要填：${name}`,
    formUnreadable: "这张表在这里画不出来，只能拒绝或先不管",
    sending: "正在送出…",
    failed: (why) => `没送出去：${why}`,
    cannot: (request) => `它在等一个这里还答不了的回答（${request}）`,
    permissions: "它在请你给它更多权限，这里还答不了",
    verification: "它在请你做一次身份验证，这里还答不了",
    tool: (name) => `想用 ${name}`,
    rule: (allow, rules, where) =>
      `${allow === "allow" ? "以后都允许" : allow === "deny" ? "以后都不许" : "以后都先问我"} ${rules}（${where}）`,
    unrule: (rules, where) => `去掉 ${rules} 这条规矩（${where}）`,
    mode: (mode, where) => `切到 ${mode} 模式（${where}）`,
    directories: (add, directories, where) => `${add ? "允许它用" : "不再让它用"} ${directories}（${where}）`,
    kept: {
      session: "只在这段对话里",
      localSettings: "记进这个项目的本地设置",
      projectSettings: "记进这个项目的设置，会随项目共享",
      userSettings: "记进你所有项目的设置",
      cliArg: "只在这次启动里",
    },
    answered: (said) => said,
    gone: "已在别处答过，或不再需要了",
    settledBy: {
      accept: "你允许了",
      session: "你允许了，这段对话里不再问",
      rule: "你定下了一条以后照办的规矩",
      decline: "你没有允许，它接着做别的",
      cancel: "你没有允许，这一轮停下了",
      answered: "你回答了",
      skipped: "你跳过了",
      submitted: "你提交了",
      refused: "你拒绝了",
      dismissed: "你没有理会",
      opened: "你打开了网址",
    },
  },
  commands: {
    world: "世界",
    compact: "现在压缩这段对话",
    model: "换模型与推理强度",
    rename: "给这段对话起名",
    fork: "从这里分叉出一段新的对话",
    stop: "停下这一轮",
    noCommands: "没有对得上的命令",
    effort: "推理强度",
    current: "现在用的",
  },
  modelChip: "模型",
  noModels: "它没有报出可选的模型",
  rewindAsk: "从这一句起重来：这一句和它之后的对话会从对话里拿掉，它改过的文件不会回到那时。这一句会放回输入框，改好再发。",
  rewindGo: "从这里重来",
  forkAsk: "从这一轮分出一段新的对话：它带着到这里为止的全部，在同一台机器、同一个项目里，之后各走各的。",
  forkGo: "分出去",
  never: "算了",
  unknown: (name) => `没有 /${name} 这个命令`,
  running: (name) => `正在执行 /${name}…`,
  done: (name) => `/${name} 做完了`,
  failed: (why) => `没做成：${why}`,
  context: (percent, used, window) => `上下文用了 ${percent}%（${used.toLocaleString()} / ${window.toLocaleString()}）`,
  usage: {
    title: "用量",
    context: "上下文",
    contextUsed: (used, window) => (window ? `${used.toLocaleString()} / ${window.toLocaleString()}` : used.toLocaleString()),
    limits: "额度",
    window: (name) => {
      const minutes = /(\d+)m$/.exec(name);
      if (minutes) {
        const m = Number(minutes[1]);
        return m % 1440 === 0 ? `${m / 1440} 天的窗口` : m % 60 === 0 ? `${m / 60} 小时的窗口` : `${m} 分钟的窗口`;
      }
      return ({ five_hour: "5 小时的窗口", seven_day: "7 天的窗口" } as Record<string, string>)[name] ?? name;
    },
    used: (percent) => `用了 ${percent}%`,
    resets: (at) => `${at.getMonth() + 1}月${at.getDate()}日 ${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")} 重置`,
    cost: (usd) => `这段对话花了 $${usd}`,
    none: "它没有报出用量",
  },
};

export const enComposer: ComposerWords = {
  files: {
    hint: "Type a few letters of a file's name",
    searching: "Looking…",
    none: "No such file in this project's places on that machine",
  },
  attach: {
    attach: "Attach files",
    drop: (agent) => `Drop to attach for ${agent}`,
    remove: "Remove",
    sending: "Sending…",
    failed: (why) => `Not sent: ${why}`,
    retry: "Try again",
    size: (bytes) => sizeOf(bytes, ["B", "KB", "MB", "GB"]),
    wait: "Attachments are still on their way; send when they arrive",
  },
  asks: {
    waiting: "Waiting for you",
    command: (machine) => `Wants to run a command on ${machine}`,
    stdin: (machine) => `Wants to send input to a program running on ${machine}`,
    network: (host, protocol) => `Wants to reach ${host}${protocol ? ` (${protocol})` : ""}`,
    files: "Wants to change files",
    filesNamed: (count) => `Wants to change ${count} ${count === 1 ? "file" : "files"}`,
    grantRoot: (root) => `and asks to write anything under ${root} for the rest of this conversation`,
    reason: (why) => `It says: ${why}`,
    inDir: (cwd) => `In ${cwd}`,
    accept: "Allow",
    acceptForSession: "Allow for this conversation",
    acceptPrefix: (prefix) => `Always allow commands starting ${prefix}`,
    networkRule: (allow, host) => (allow ? `Always allow ${host}` : `Never allow ${host}`),
    decline: "Don't allow, carry on",
    cancel: "Don't allow, stop this turn",
    question: "It has a question",
    questions: (count) => `It has ${count} questions`,
    several: "choose any",
    otherAnswer: "Or write your own answer",
    secret: "What you type is hidden",
    submit: "Answer",
    fill: "Submit",
    skip: "Skip",
    form: (server) => `${server} asks you to fill in a form`,
    link: (server) => `${server} asks you to open a link`,
    openLink: "Open it and carry on",
    refuse: "Refuse",
    dismiss: "Not now",
    required: "This one is needed",
    requiredOf: (name) => `Required: ${name}`,
    formUnreadable: "This form can't be drawn here; it can only be refused or left",
    sending: "Sending…",
    failed: (why) => `Not sent: ${why}`,
    cannot: (request) => `It waits for an answer that can't be given here (${request})`,
    permissions: "It asks for more permissions, which can't be granted here",
    verification: "It asks you to verify who you are, which can't be done here",
    tool: (name) => `Wants to use ${name}`,
    rule: (allow, rules, where) =>
      `${allow === "allow" ? "Always allow" : allow === "deny" ? "Never allow" : "Always ask first for"} ${rules} (${where})`,
    unrule: (rules, where) => `Drop the rule ${rules} (${where})`,
    mode: (mode, where) => `Switch to ${mode} mode (${where})`,
    directories: (add, directories, where) => `${add ? "Let it use" : "Stop letting it use"} ${directories} (${where})`,
    kept: {
      session: "this conversation only",
      localSettings: "kept in this project's local settings",
      projectSettings: "kept in this project's shared settings",
      userSettings: "kept in your settings for every project",
      cliArg: "this launch only",
    },
    answered: (said) => said,
    gone: "Answered elsewhere, or no longer needed",
    settledBy: {
      accept: "You allowed it",
      session: "You allowed it for this conversation",
      rule: "You set a rule it keeps from now on",
      decline: "You didn't allow it; it carried on",
      cancel: "You didn't allow it; the turn stopped",
      answered: "You answered",
      skipped: "You skipped it",
      submitted: "You submitted it",
      refused: "You refused",
      dismissed: "You left it",
      opened: "You opened the link",
    },
  },
  commands: {
    world: "World",
    compact: "Compact this conversation now",
    model: "Change the model and effort",
    rename: "Name this conversation",
    fork: "Fork a new conversation from here",
    stop: "Stop this turn",
    noCommands: "No command matches",
    effort: "effort",
    current: "in use",
  },
  modelChip: "Model",
  rewindAsk: "Start again from these words: they and everything after them leave the conversation, and files it changed are not put back. The words return to the box to edit and send.",
  rewindGo: "Start again here",
  forkAsk: "Branch a new conversation from this turn: it carries everything up to here, on the same machine and in the same project, and goes its own way.",
  forkGo: "Branch it",
  never: "Never mind",
  noModels: "It lists no models to choose from",
  unknown: (name) => `There is no /${name}`,
  running: (name) => `Running /${name}…`,
  done: (name) => `/${name} done`,
  failed: (why) => `It did not go through: ${why}`,
  context: (percent, used, window) => `${percent}% of the context used (${used.toLocaleString()} of ${window.toLocaleString()})`,
  usage: {
    title: "Usage",
    context: "Context",
    contextUsed: (used, window) => (window ? `${used.toLocaleString()} / ${window.toLocaleString()}` : used.toLocaleString()),
    limits: "Limits",
    window: (name) => {
      const minutes = /(\d+)m$/.exec(name);
      if (minutes) {
        const m = Number(minutes[1]);
        return m % 1440 === 0 ? `${m / 1440}-day window` : m % 60 === 0 ? `${m / 60}-hour window` : `${m}-minute window`;
      }
      return ({ five_hour: "5-hour window", seven_day: "7-day window" } as Record<string, string>)[name] ?? name;
    },
    used: (percent) => `${percent}% used`,
    resets: (at) => `resets ${at.toLocaleString("en", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}`,
    cost: (usd) => `This conversation cost $${usd}`,
    none: "It reported no usage",
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
  outside: "不在项目里",
  machines: "机器",
  settings: "设置",
  newTalk: "新对话",
  newProject: "新项目",
  edit: "修改",
  openSidebar: "打开侧栏",
  foldSidebar: "收起侧栏",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueFor: (asker) => `受 ${asker} 之托`,
  busy: (lasted) => `在干活 · ${lasted}`,
  idle: "空闲",
  waiting: "等你回答",
  asleep: "睡着了",
  offline: "离线",
  noProjects: "还没有项目",
  nobodyHere: "此刻没有谁在",
  unreachable: "问不到世界",
  unanswered: (seconds) => `世界已经 ${seconds} 秒没回答`,
  thisOne: "这台",
  phone: "手机",
  joinOrFound: "加入或建一个世界",
  more: (n) => `还有 ${n} 个`,
  span: zhSpan,
  agentsIn: (project) => `${project} 里的智能体`,
  frame: { minimize: "最小化", maximize: "最大化", restore: "还原", close: "关闭" },
};

export const enShell: ShellWords = {
  app: "World",
  world: "The World",
  projects: "Projects",
  outside: "In no project",
  machines: "Machines",
  settings: "Settings",
  newTalk: "New conversation",
  newProject: "New project",
  edit: "Edit",
  openSidebar: "Open the sidebar",
  foldSidebar: "Fold the sidebar",
  someone: (agent, machine) => `${agent} · ${machine}`,
  colleagueFor: (asker) => `for ${asker}`,
  busy: (lasted) => `working · ${lasted}`,
  idle: "idle",
  waiting: "waiting for you",
  asleep: "asleep",
  offline: "offline",
  noProjects: "No projects yet",
  nobodyHere: "Nobody here right now",
  unreachable: "The World cannot be asked",
  unanswered: (seconds) => `The World has not answered for ${seconds} s`,
  thisOne: "this one",
  phone: "phone",
  joinOrFound: "Join or found a World",
  more: (n) => `${n} more`,
  span: enSpan,
  agentsIn: (project) => `Agents in ${project}`,
  frame: { minimize: "Minimize", maximize: "Maximize", restore: "Restore", close: "Close" },
};
